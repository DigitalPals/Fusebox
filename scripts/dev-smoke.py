#!/usr/bin/env python3
"""Check a rebuilt Fusebox with a synthetic loopback provider and current UI assets.

Reuses runtime-benchmark fixtures. No personal credentials or history are used.
On thebeast run inside agent-resource; see docs/optimization-report.md.
"""
import argparse
import gzip
import hashlib
import http.client
import json
import os
import re
import runpy
import socket
import tempfile
import threading
from pathlib import Path


def main():
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=root / 'target/release/fusebox')
    args = parser.parse_args()
    binary = args.binary.resolve()
    workspace = os.environ.get('AGENT_RESOURCE_WORKSPACE')
    if socket.gethostname() == 'thebeast' and not workspace:
        parser.error('run inside agent-resource on thebeast (see docs/optimization-report.md)')
    helpers = runpy.run_path(str(root / 'scripts/runtime-benchmark.py'))
    upstream = helpers['ProviderServer'](('127.0.0.1', 0), helpers['Provider'])
    thread = threading.Thread(target=upstream.serve_forever, daemon=True)
    thread.start()
    checks = []

    def request(port, path, body=None, headers=None):
        conn = http.client.HTTPConnection('127.0.0.1', port, timeout=30)
        try:
            conn.request('POST' if body is not None else 'GET', path, body,
                         headers or {'Content-Type': 'application/json'})
            response = conn.getresponse()
            return response.status, {name.lower(): value for name, value in response.getheaders()}, response.read()
        finally:
            conn.close()

    try:
        with tempfile.TemporaryDirectory(prefix='fusebox-dev-smoke-', dir=workspace) as directory:
            with helpers['service'](binary, directory, upstream.server_port, '4') as (process, port):
                for path in ['/healthz', '/readyz']:
                    status, _, _ = request(port, path)
                    assert status == 200, (path, status)
                checks.append('rebuilt service is live and ready with a loopback-only provider')
                for path, payload, kind in [
                    ('/v1/responses', {'model': 'gpt-6.1-sol', 'input': 'synthetic check', 'stream': False}, 'native'),
                    ('/v1/chat/completions', {'model': 'gpt-6.1-sol', 'messages': [{'role': 'user', 'content': 'synthetic check'}], 'stream': False}, 'translated'),
                ]:
                    status, _, body = request(port, path, json.dumps(payload).encode())
                    result = json.loads(body)
                    assert status == 200, (path, status, result)
                    if kind == 'native':
                        assert result['output'][0]['content'][0]['text'] == 'OK'
                        assert result['usage']['input_tokens'] == 10
                        assert result['usage']['output_tokens'] == 1
                    else:
                        assert result['choices'][0]['message']['content'] == 'OK'
                        assert result['usage']['prompt_tokens'] == 10
                        assert result['usage']['completion_tokens'] == 1
                    checks.append(kind + ' request returns expected response and token accounting')
                payload = gzip.compress(json.dumps({'model': 'gpt-6.1-sol', 'input': 'compressed synthetic check'}).encode())
                status, _, body = request(port, '/v1/responses', payload,
                                         {'Content-Type': 'application/json', 'Content-Encoding': 'gzip'})
                assert status == 200 and json.loads(body)['output'][0]['content'][0]['text'] == 'OK'
                checks.append('gzip request body decompresses and completes')
                status, _, page = request(port, '/')
                assert status == 200
                page = page.decode()
                urls = {}
                for file in ['usage.js', 'app.js']:
                    match = re.search(r'/ui/' + re.escape(file) + r'\?v=[a-f0-9]+', page)
                    assert match, file
                    urls[file] = match.group()
                    status, headers, body = request(port, match.group())
                    assert status == 200 and body == (root / 'ui' / file).read_bytes(), file
                    status, gz_headers, compressed = request(port, match.group(), headers={'Accept-Encoding': 'gzip'})
                    assert status == 200 and gz_headers.get('content-encoding') == 'gzip'
                    assert gzip.decompress(compressed) == body
                    status, _, empty = request(port, match.group(), headers={'If-None-Match': headers['etag']})
                    assert status == 304 and empty == b''
                assert page.index(urls['usage.js']) < page.index(urls['app.js'])
                checks.append('current app/usage source is embedded, versioned, gzip-negotiated and conditionally cached')
                checks.append('shared UsageTools loads before app consumers')
                status, _, body = request(port, '/metrics')
                assert status == 200 and b'fusebox_build_info' in body
                checks.append('rebuilt service exposes management metrics')
            assert process.poll() == 0, process.returncode
            checks.append('mock-backed service exits gracefully')
    finally:
        upstream.shutdown()
        upstream.server_close()
        thread.join(timeout=5)
    print(json.dumps({'passed': len(checks), 'checks': checks,
                      'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                      'mock_only': True, 'tokio_worker_threads': 4}, indent=2))


if __name__ == '__main__':
    main()
