#!/usr/bin/env python3
"""Local mock-only runtime/worker benchmark. No provider credentials or inference.
Run inside agent-resource on thebeast. JSON output contains only measurements.
"""
import argparse
import concurrent.futures
import contextlib
import hashlib
import http.client
import http.server
import json
import itertools
import math
import os
from pathlib import Path
import socket
import statistics
import subprocess
import tempfile
import threading
import time


def call(port, path, body=None):
    connection=http.client.HTTPConnection("127.0.0.1",port,timeout=20)
    start=time.perf_counter()
    try:
        connection.connect()
        connection.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        connection.request("POST" if body is not None else "GET",path,body=body,
                           headers={"Content-Type":"application/json"} if body is not None else {})
        response=connection.getresponse()
        value=response.read()
        if response.status!=200:
            raise RuntimeError("unexpected_mock_status")
        return (time.perf_counter()-start)*1000, value
    finally:
        connection.close()


def resources(process):
    memory = {}
    for line in Path(f"/proc/{process.pid}/status").read_text().splitlines():
        key, _, value = line.partition(":")
        if key in ("VmRSS", "VmHWM", "Threads"):
            memory[key] = int(value.strip().split()[0])
    fields = Path(f"/proc/{process.pid}/stat").read_text().rsplit(")", 1)[1].split()
    return memory, (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")


class Provider(http.server.BaseHTTPRequestHandler):
    protocol_version="HTTP/1.1"
    sequence=itertools.count()
    def setup(self):
        super().setup()
        self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    def do_POST(self):
        request=json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        type(self).last_request=request
        response={"id":f"mock-{next(self.sequence)}", "object":"response", "status":"completed", "model":request["model"],
                  "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"OK"}]}],
                  "usage":{"input_tokens":10,"output_tokens":1,"input_tokens_details":{"cached_tokens":0,"cache_write_tokens":0}}}
        wire=json.dumps(response).encode()
        self.send_response(200)
        self.send_header("Content-Type","application/json")
        self.send_header("Content-Length",str(len(wire)))
        self.end_headers()
        self.wfile.write(wire)
    def log_message(self,*_): pass


class ProviderServer(http.server.ThreadingHTTPServer):
    request_queue_size=128
    daemon_threads=True


@contextlib.contextmanager
def service(binary, directory, upstream_port, workers):
    with socket.socket() as port_socket:
        port_socket.bind(("127.0.0.1",0)); port=port_socket.getsockname()[1]
    config=Path(directory)/"config.json"
    config.write_text(json.dumps({"host":"127.0.0.1","port":port,"auth-dir":str(Path(directory)/"auth"),
        "usage":{"enabled":False},"codex-api-key":[{"api-key":"mock-only","base-url":f"http://127.0.0.1:{upstream_port}/v1"}]}))
    env=os.environ.copy();env.pop("TOKIO_WORKER_THREADS",None)
    if workers!="default":env["TOKIO_WORKER_THREADS"]=str(workers)
    with (Path(directory)/"service.log").open("w") as log:
        process=subprocess.Popen([str(binary),"--config",str(config)],env=env,stdout=log,stderr=log)
        try:
            for _ in range(200):
                if process.poll() is not None:raise RuntimeError("mock_service_start_failed")
                try:
                    call(port,"/healthz");break
                except (OSError,http.client.HTTPException):time.sleep(.05)
            else:raise RuntimeError("mock_service_not_ready")
            yield process,port
        finally:
            process.terminate()
            try:process.wait(timeout=10)
            except subprocess.TimeoutExpired:process.kill();process.wait(timeout=5)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary",type=Path,default=Path("target/release/fusebox"))
    parser.add_argument("--requests",type=int,default=24)
    parser.add_argument("--label",default="working-tree")
    parser.add_argument("--output",type=Path,help="Also save the metadata-only JSON report")
    parser.add_argument("--workers",nargs="+",default=["2","4","8","default"])
    args=parser.parse_args()
    if not 1<=args.requests<=1000 or any(w not in ("2","4","8","default") for w in args.workers):parser.error("invalid limits")
    upstream=ProviderServer(("127.0.0.1",0),Provider)
    threading.Thread(target=upstream.serve_forever,daemon=True).start()
    result={"mock_only":True,"provider_savings_measured":False,"host_cpus":os.cpu_count(),
            "label": args.label, "binary_version": subprocess.check_output([str(args.binary.resolve()), "--version"], text=True).strip(),
            "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
            "measurement_notes": ["Single local synthetic run; fresh client connection for each request; warmup precedes each payload.",
                "TCP_NODELAY on client and mock; mock backlog 128.",
                "RSS includes retained store:false continuation histories; memory is cumulative within each worker run.",
                "Proxy CPU uses Linux scheduler tick resolution; small cases may round to zero.",
                "This compares worker counts on the current implementation, not old and new revisions."],
            "allocator":"application default: 4 glibc arenas, 256 KiB mmap threshold", "runs":[]}
    try:
        for workers in args.workers:
            with tempfile.TemporaryDirectory(prefix="fusebox-runtime-",dir=os.environ.get("AGENT_RESOURCE_WORKSPACE")) as directory:
                with service(args.binary.resolve(),directory,upstream.server_port,workers) as (process,port):
                    for kind, size in (("text",10<<10),("text",200<<10),("text",1<<20),("multimodal",1<<20)):
                        request_input = "x"*size if kind == "text" else [{"role":"user","content":[
                            {"type":"input_text","text":"Describe this image"},
                            {"type":"input_image","image_url":"data:image/png;base64,"+"A"*size}]}]
                        body=json.dumps({"model":"gpt-6.1-sol","input":request_input,"stream":False,"store":False}).encode()
                        call(port,"/v1/responses",body)
                        for concurrency in (1,8):
                            _, cpu_start = resources(process)
                            start=time.perf_counter()
                            with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as pool:
                                rows=list(pool.map(lambda _:call(port,"/v1/responses",body),range(args.requests)))
                            elapsed=time.perf_counter()-start
                            ms=sorted(row[0] for row in rows)
                            memory, cpu_end = resources(process)
                            result["runs"].append({"workers":workers,"payload_kind":kind,"payload_bytes":len(body),"concurrency":concurrency,
                                "requests":len(rows),"requests_per_second":len(rows)/elapsed,
                                "median_ms":statistics.median(ms),"p95_ms":ms[max(0,math.ceil(len(ms)*.95)-1)],
                                "proxy_cpu_ms_per_request": (cpu_end-cpu_start)*1000/len(rows), **memory})
    finally:
        upstream.shutdown();upstream.server_close()
    report=json.dumps(result,indent=2)
    if args.output:args.output.write_text(report+"\n")
    print(report)


if __name__=="__main__":main()
