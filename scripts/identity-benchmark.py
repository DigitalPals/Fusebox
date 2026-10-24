#!/usr/bin/env python3
"""Compare loopback provider latency using fake OAuth credentials only.

Run on thebeast through agent-resource run; its private workspace owns all
fixture files. This measures proxy overhead, not real provider generation/TLS.
"""
import argparse
import http.client
import http.server
import json
import os
from pathlib import Path
import signal
import socket
import statistics
import subprocess
import threading
import time


class Provider(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def setup(self):
        super().setup()
        self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def log_message(self, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if self.path.startswith('/v1/messages'):
            result = {'id': 'msg_local', 'type': 'message', 'role': 'assistant',
                      'model': body['model'], 'content': [{'type': 'text', 'text': 'OK'}],
                      'stop_reason': 'end_turn', 'usage': {'input_tokens': 1, 'output_tokens': 1}}
            payload, content_type = json.dumps(result).encode(), 'application/json'
        else:
            result = {'id': 'resp_local', 'object': 'response', 'status': 'completed',
                      'model': body['model'], 'output': [{'type': 'message', 'role': 'assistant',
                      'content': [{'type': 'output_text', 'text': 'OK'}]}],
                      'usage': {'input_tokens': 1, 'output_tokens': 1}}
            payload = ''.join('data: ' + json.dumps(event) + '\n\n' for event in [
                {'type': 'response.created', 'response': {'id': 'resp_local', 'model': body['model']}},
                {'type': 'response.output_text.delta', 'delta': 'OK', 'output_index': 0, 'content_index': 0},
                {'type': 'response.completed', 'response': result}]).encode()
            content_type = 'text/event-stream'
        self.send_response(200)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)
        self.wfile.flush()


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def measure(binary, directory, provider_url, kind, count, warmup):
    directory.mkdir(parents=True, exist_ok=True)
    auth = directory / 'auth'
    auth.mkdir(exist_ok=True)
    (auth / 'account.json').write_text(json.dumps({
        'type': kind, 'access_token': 'FAKE_LOOPBACK_ACCESS', 'account_id': 'local-account',
        'base_url': provider_url}))
    port = free_port()
    config = directory / 'config.yaml'
    config.write_text(f'host: 127.0.0.1\nport: {port}\nauth-dir: {auth}\n'
                      'api-keys: [FAKE_LOOPBACK_CLIENT]\nproxy-url: direct\nusage:\n  enabled: false\n')
    with (directory / 'service.log').open('a') as log:
        process = subprocess.Popen([str(binary), '--config', str(config)], stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 15
            while True:
                if process.poll() is not None:
                    raise RuntimeError(f'fixture exited: {directory / "service.log"}')
                try:
                    conn = http.client.HTTPConnection('127.0.0.1', port, timeout=2)
                    conn.request('GET', '/readyz')
                    response = conn.getresponse()
                    response.read()
                    conn.close()
                    if response.status == 200:
                        break
                except OSError:
                    pass
                if time.monotonic() >= deadline:
                    raise TimeoutError('local fixture readiness timed out')
                time.sleep(0.02)
            body = ({'model': 'claude-sonnet-4-6', 'max_tokens': 8,
                     'messages': [{'role': 'user', 'content': 'loopback benchmark'}]}
                    if kind == 'claude' else {'model': 'gpt-6.1-sol', 'input': 'loopback benchmark'})
            path = '/v1/messages' if kind == 'claude' else '/v1/responses'
            headers = {'Authorization': 'Bearer FAKE_LOOPBACK_CLIENT', 'Content-Type': 'application/json',
                       'x-fusebox-session-id': 'benchmark-conversation'}
            payload = json.dumps(body)
            conn = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
            samples = []
            try:
                for index in range(count + warmup):
                    started = time.perf_counter_ns()
                    conn.request('POST', path, payload, headers)
                    response = conn.getresponse()
                    data = response.read()
                    elapsed = (time.perf_counter_ns() - started) / 1_000_000
                    if response.status != 200:
                        raise RuntimeError(f'loopback request failed: {response.status}: {data[:300]!r}')
                    if index >= warmup:
                        samples.append(elapsed)
            finally:
                conn.close()
            return samples
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGINT)
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)


def summarize(samples):
    ordered = sorted(samples)
    return {'requests': len(samples), 'median_ms': round(statistics.median(samples), 3),
            'p95_ms': round(ordered[min(len(ordered)-1, int(len(ordered)*0.95))], 3)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', type=Path)
    parser.add_argument('--after', type=Path, required=True)
    parser.add_argument('--requests', type=int, default=100)
    parser.add_argument('--warmup', type=int, default=20)
    parser.add_argument('--rounds', type=int, default=3)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root = Path(os.environ['AGENT_RESOURCE_WORKSPACE'])
    if min(args.requests, args.rounds) < 1 or args.warmup < 0:
        parser.error('requests and rounds must be positive, warmup nonnegative')
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Provider)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    results = {'scope': 'loopback, debug binaries, fake credentials, reused connections; excludes real provider generation and TLS', 'providers': {}}
    try:
        for kind in ['claude', 'codex']:
            samples = {'before': [], 'after': []}
            for iteration in range(args.rounds):
                names = ['before', 'after'] if iteration % 2 == 0 else ['after', 'before']
                for name in names:
                    binary = getattr(args, name)
                    if binary:
                        samples[name].extend(measure(binary.resolve(), root / kind / name,
                            f'http://127.0.0.1:{server.server_port}', kind, args.requests, args.warmup))
            results['providers'][kind] = {name: summarize(values) for name, values in samples.items() if values}
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(results, indent=2) + '\n')
    print(json.dumps(results, indent=2))


if __name__ == '__main__':
    main()
