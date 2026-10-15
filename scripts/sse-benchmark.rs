//! Decoder-only synthetic benchmark; does not measure network/provider latency.
//! rustc -O --edition=2024 scripts/sse-benchmark.rs -o /tmp/fusebox-sse-benchmark
//! /tmp/fusebox-sse-benchmark

#[allow(dead_code)]
#[path = "../src/sse.rs"]
mod sse;

use std::{hint::black_box, time::Instant};

fn measure(body: &[u8], chunk: usize, expected: usize) -> f64 {
    let mut samples = Vec::new();
    for _ in 0..9 {
        let start = Instant::now();
        let mut decoder = sse::SseDecoder::default();
        let mut count = 0;
        for part in body.chunks(chunk) {
            count += black_box(decoder.push(black_box(part))).len();
        }
        count += black_box(decoder.finish()).len();
        assert_eq!(count, expected);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn main() {
    println!("case,bytes,chunk_bytes,median_ms");
    for size in [1 << 20, 4 << 20] {
        let body = format!("data: {}\n\n", "x".repeat(size));
        for chunk in [1024, 16384, 65536] {
            println!("long_line,{},{chunk},{:.6}", body.len(), measure(body.as_bytes(), chunk, 1));
        }
    }
    let body: String = (0..2000).map(|i| format!("event: response.output_text.delta\ndata: {{\"type\":\"response.output_text.delta\",\"delta\":\"token {i} some words here\",\"sequence_number\":{i}}}\n\n")).collect();
    println!("small_events,{},16384,{:.6}", body.len(), measure(body.as_bytes(), 16384, 2000));
}
