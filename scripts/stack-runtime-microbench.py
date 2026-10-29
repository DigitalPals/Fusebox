#!/usr/bin/env python3
"""Benchmark production SSE/routing primitives against a git revision.

No servers, credentials, containers or external providers are used. Requires
Rust stable; run `python3 scripts/stack-runtime-microbench.py --baseline-ref REF`.
The routing harness extracts the production wildcard function so it can measure
allocator calls without the rest of the application or its test runner.
"""
import argparse
import csv
import hashlib
import io
import json
from pathlib import Path
import platform
import subprocess
import tempfile


ROUTING_HARNESS = r"""
use std::{alloc::{GlobalAlloc, Layout, System}, hint::black_box, sync::atomic::{AtomicUsize, Ordering}, time::Instant};
struct Counting;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) { unsafe { System.dealloc(ptr, layout) } }
    unsafe fn realloc(&self, ptr:*mut u8, layout:Layout, size:usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(size, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator] static ALLOCATOR: Counting = Counting;
fn main() {
    let cases = [("claude-opus-5-5", "CLAUDE-OPUS-5-5"), ("gpt-*", "gpt-6.1-sol"), ("*-preview", "gemini-4-flash-preview"), ("*flash*", "gemini-4-flash-preview"), ("gpt-*-astra*", "gpt-6.1-sol"), ("*🦀B", "Aé🦀b")];
    for (pattern, model) in &cases {
        assert_eq!(reference(pattern, model), wildcard(pattern, model));
    }
    for (algorithm, function) in [("before", reference as fn(&str,&str)->bool), ("after",wildcard)] {
        let mut samples=Vec::new();
        let mut allocs=0;
        let mut bytes=0;
        for _ in 0..9 {
            let alloc_start=ALLOCATIONS.load(Ordering::Relaxed);
            let bytes_start=ALLOCATED_BYTES.load(Ordering::Relaxed);
            let start=Instant::now();
            for _ in 0..100_000 {
                for (pattern,model) in &cases {
                    black_box(function(black_box(pattern),black_box(model)));
                }
            }
            let elapsed=start.elapsed().as_secs_f64()*1000.0;
            allocs=ALLOCATIONS.load(Ordering::Relaxed)-alloc_start;
            bytes=ALLOCATED_BYTES.load(Ordering::Relaxed)-bytes_start;
            samples.push(elapsed);
        }
        samples.sort_by(f64::total_cmp);
        println!("{{\"algorithm\":\"{algorithm}\",\"comparisons\":600000,\"median_ms\":{},\"allocations\":{allocs},\"allocated_bytes\":{bytes}}}",samples[4]);
    }
}
"""


def command(*args, cwd):
    return subprocess.check_output(args, cwd=cwd, text=True)


def wildcard_function(source, name):
    start = source.index("pub fn wildcard(")
    end = source.index("\n}", start) + 2
    return source[start:end].replace("pub fn wildcard(", f"fn {name}(", 1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-ref", default="HEAD")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    baseline = command("git", "rev-parse", "--verify", args.baseline_ref + "^{commit}", cwd=root).strip()
    old_accounts = command("git", "show", baseline + ":src/accounts.rs", cwd=root)
    new_accounts = (root / "src/accounts.rs").read_text()
    old_sse = command("git", "show", baseline + ":src/sse.rs", cwd=root)
    new_sse = (root / "src/sse.rs").read_text()
    compiler = command("rustc", "+stable", "--version", cwd=root).strip()
    result = {
        "synthetic_only": True,
        "network_or_provider_latency_measured": False,
        "baseline_commit": baseline,
        "compiler": compiler,
        "system": platform.platform(),
        "methodology": [
            "Optimized standalone Rust binaries, medians of 9 samples per case.",
            "Routing: 600,000 comparisons per sample over exact/prefix/suffix/infix/multi-wildcard/Unicode cases.",
            "Routing allocations counted by a System global allocator wrapper; harness bookkeeping excluded.",
            "SSE: existing scripts/sse-benchmark.rs, complete production decoder from each revision.",
            "Microbenchmarks measure primitives only; compiler/host noise and production workload differences apply.",
        ],
        "working_sources_sha256": {
            "src/accounts.rs": hashlib.sha256(new_accounts.encode()).hexdigest(),
            "src/sse.rs": hashlib.sha256(new_sse.encode()).hexdigest(),
        },
    }
    with tempfile.TemporaryDirectory(prefix="fusebox-runtime-primitives-") as directory:
        workspace = Path(directory)
        routing = workspace / "routing.rs"
        routing.write_text(wildcard_function(old_accounts, "reference") + "\n" +
                           wildcard_function(new_accounts, "wildcard") + "\n" + ROUTING_HARNESS)
        binary = workspace / "routing"
        command("rustc", "+stable", "-O", "--edition=2024", str(routing), "-o", str(binary), cwd=root)
        result["routing"] = [json.loads(line) for line in command(str(binary), cwd=root).splitlines()]
        result["sse"] = []
        harness = (root / "scripts/sse-benchmark.rs").read_text()
        for label, source in [("before", old_sse), ("after", new_sse)]:
            decoder = workspace / f"sse-{label}.rs"
            decoder.write_text(source)
            bench = workspace / f"sse-bench-{label}.rs"
            bench.write_text(harness.replace('"../src/sse.rs"', json.dumps(str(decoder))))
            binary = workspace / f"sse-bench-{label}"
            command("rustc", "+stable", "-O", "--edition=2024", str(bench), "-o", str(binary), cwd=root)
            rows = csv.DictReader(io.StringIO(command(str(binary), cwd=root)))
            for row in rows:
                result["sse"].append({"algorithm": label, "case": row["case"],
                                      "bytes": int(row["bytes"]), "chunk_bytes": int(row["chunk_bytes"]),
                                      "median_ms": float(row["median_ms"])})
    report = json.dumps(result, indent=2) + "\n"
    if args.output:
        args.output.write_text(report)
    print(report, end="")


if __name__ == "__main__":
    main()
