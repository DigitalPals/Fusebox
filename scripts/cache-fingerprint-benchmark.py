#!/usr/bin/env python3
"""Compare embeddings-cache hits and Claude fingerprints with a git revision.

Pure component microbenchmarks: no network, providers, containers or services.
Requires a stable release build's sha2/hex dependencies in target/release/deps.
"""

import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import tempfile


CACHE_HARNESS = r"""
use std::{hint::black_box, sync::Arc, time::Instant};
type Value = usize;
pub struct Config { cache_seconds: u64, cache_bytes: usize }
fn main() {
    let cfg = Config { cache_seconds: 86400, cache_bytes: 1 << 20 };
    for count in [1, 100, 1000] {
        let keys: Vec<_> = (0..count).map(|i| i.to_string()).collect();
        let mut cache = Cache::default();
        for (i, key) in keys.iter().enumerate() {
            cache.insert(key.clone(), Arc::new(i), 16, &cfg);
        }
        let mut samples = Vec::new();
        for _ in 0..9 {
            let start = Instant::now();
            for i in 0..100_000 {
                black_box(cache.get(black_box(&keys[i % count]), black_box(&cfg))).unwrap();
            }
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(f64::total_cmp);
        assert_eq!(cache.stats(&cfg), (900_000, 0, count, count * 16));
        println!("{{\"entries\":{count},\"hits_per_sample\":100000,\"median_ms\":{}}}", samples[4]);
    }
}
"""

FINGERPRINT_HARNESS = r"""
use sha2::{Digest, Sha256};
use std::{alloc::{GlobalAlloc, Layout, System}, hint::black_box,
    sync::atomic::{AtomicUsize, Ordering}, time::Instant};
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
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(size, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator] static ALLOCATOR: Counting = Counting;
fn main() {
    for bytes in [0, 10 * 1024, 1024 * 1024] {
        let message = "a".repeat(bytes);
        assert_eq!(before(&message), after(&message));
        for (algorithm, function) in [("before", before as fn(&str) -> String), ("after", after)] {
            let mut samples = Vec::new();
            let (mut allocations, mut allocated_bytes) = (0, 0);
            for _ in 0..9 {
                let old_allocations = ALLOCATIONS.load(Ordering::Relaxed);
                let old_bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
                let start = Instant::now();
                for _ in 0..200 {
                    black_box(function(black_box(&message)));
                }
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                allocations = ALLOCATIONS.load(Ordering::Relaxed) - old_allocations;
                allocated_bytes = ALLOCATED_BYTES.load(Ordering::Relaxed) - old_bytes;
                samples.push(elapsed);
            }
            samples.sort_by(f64::total_cmp);
            println!("{{\"algorithm\":\"{algorithm}\",\"prompt_bytes\":{bytes},\"calls_per_sample\":200,\"median_ms\":{},\"allocations\":{allocations},\"allocated_bytes\":{allocated_bytes}}}", samples[4]);
        }
    }
}
"""


def command(*args, cwd):
    return subprocess.check_output(args, cwd=cwd, text=True)


def function(source, marker):
    start = source.index(marker)
    end = source.index("\n}", start) + 2
    return source[start:end]


def cache_source(source):
    start = source.index("struct Cached {")
    end = source.index("\n#[cfg(test)]", start)
    cache = source[start:end]
    # Key construction is outside the cache mutex and outside this measurement.
    start = cache.index("    fn key(")
    end = cache.index("    fn get(", start)
    return cache[:start] + cache[end:]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-ref", default="HEAD")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--deps-dir", type=Path, default=Path("target/release/deps"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    deps = (root / args.deps_dir).resolve()
    baseline = command("git", "rev-parse", "--verify", args.baseline_ref + "^{commit}", cwd=root).strip()
    old = {path: command("git", "show", baseline + ":" + path, cwd=root)
           for path in ["src/embeddings.rs", "src/upstream.rs"]}
    new = {path: (root / path).read_text() for path in old}
    result = {
        "synthetic_only": True,
        "network_or_provider_latency_measured": False,
        "baseline_commit": baseline,
        "compiler": command("rustc", "+stable", "--version", cwd=root).strip(),
        "system": platform.platform(),
        "methodology": [
            "Optimized standalone Rust, medians of 9 samples per case.",
            "Cache: exact production cache methods, keys prebuilt, Arc<usize> payload instead of JSON.",
            "Cache: only hits measured, no serialization, key hashing, misses, expiry or mutex contention.",
            "Fingerprint: production function with the client version fixed to omit global lookup.",
            "Fingerprint allocations counted with a System allocator wrapper; cumulative allocated bytes, not peak memory; harness excluded.",
            "Neither measurement estimates end-to-end request latency or provider speed.",
        ],
        "working_sources_sha256": {path: hashlib.sha256(source.encode()).hexdigest()
                                   for path, source in new.items()},
        "cache": [],
    }
    with tempfile.TemporaryDirectory(prefix="fusebox-cache-fingerprint-") as directory:
        workspace = Path(directory)
        for label, sources in [("before", old), ("after", new)]:
            source = workspace / f"cache_{label}.rs"
            source.write_text(cache_source(sources["src/embeddings.rs"]) + "\n" + CACHE_HARNESS)
            binary = workspace / f"cache_{label}"
            command("rustc", "+stable", "-O", "--edition=2024", str(source), "-o", str(binary), cwd=root)
            result["cache"].extend({"algorithm": label, **json.loads(line)}
                                   for line in command(str(binary), cwd=root).splitlines())
        source = workspace / "fingerprint.rs"
        content = FINGERPRINT_HARNESS
        for label, sources in [("before", old), ("after", new)]:
            code = function(sources["src/upstream.rs"], "fn cc_fingerprint(")
            code = code.replace("fn cc_fingerprint(", f"fn {label}(", 1)
            code = code.replace("crate::clients::version(crate::clients::CLAUDE_CODE)", '"2.1.310".to_string()')
            content += "\n" + code
        salt = next(line for line in new["src/upstream.rs"].splitlines()
                    if line.startswith("const CC_FINGERPRINT_SALT:"))
        source.write_text(content + "\n" + salt + "\n")
        binary = workspace / "fingerprint"
        externs = []
        for crate in ["sha2", "hex"]:
            candidates = sorted(deps.glob(f"lib{crate}-*.rlib"), key=lambda path: path.stat().st_mtime, reverse=True)
            if not candidates:
                parser.error(f"{crate} release dependency missing; run cargo build --release --locked first")
            externs.extend(["--extern", f"{crate}={candidates[0]}"])
        command("rustc", "+stable", "-O", "--edition=2024", "-L", f"dependency={deps}",
                *externs, str(source), "-o", str(binary), cwd=root)
        result["fingerprint"] = [json.loads(line) for line in command(str(binary), cwd=root).splitlines()]
    report = json.dumps(result, indent=2) + "\n"
    if args.output:
        args.output.write_text(report)
    print(report, end="")


if __name__ == "__main__":
    main()
