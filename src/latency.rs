//! Optional tie-breaking for comparable cold, successful first turns only.
use serde_json::Value;
use std::collections::HashMap;
use std::time::Instant;
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Context {
    bytes: u8,
    effort: String,
    cache_intent: &'static str,
    ws: bool,
}
impl Context {
    pub fn new(body: &Value, transport: &str) -> Self {
        // Only the bucket matters: count serialized bytes without allocating, and
        // stop once the largest bucket boundary is passed.
        let len = serialized_len(body, 1 << 20);
        let (_, suffix) = crate::ir::split_model_suffix(body["model"].as_str().unwrap_or_default());
        let suffix_effort = suffix.and_then(|r| r.effort_level());
        let effort = suffix_effort
            .as_deref()
            .or(body["reasoning"]["effort"].as_str())
            .or(body["reasoning_effort"].as_str())
            .or(body["output_config"]["effort"].as_str())
            .unwrap_or("default")
            .to_string();
        Self {
            bytes: if len <= 16 << 10 {
                0
            } else if len <= 128 << 10 {
                1
            } else if len <= 1 << 20 {
                2
            } else {
                3
            },
            effort,
            cache_intent: crate::cache::Requested::of(body).intent,
            ws: transport == "ws",
        }
    }
}
/// Serialized JSON length, or a value above `cap` once it is exceeded.
fn serialized_len(value: &Value, cap: usize) -> usize {
    struct Counter {
        bytes: usize,
        cap: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.bytes = self.bytes.saturating_add(buf.len());
            if self.bytes > self.cap {
                return Err(std::io::Error::other("size bucket decided"));
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, cap };
    let _ = serde_json::to_writer(&mut counter, value);
    counter.bytes
}
#[derive(Clone, Debug)]
pub struct Sample {
    pub count: u32,
    pub ms: f64,
    updated: Instant,
}
impl Sample {
    pub fn add(&mut self, ms: u64) {
        self.ms = 0.8 * self.ms + 0.2 * ms as f64;
        self.count = self.count.saturating_add(1);
        self.updated = Instant::now();
    }
    pub fn new(ms: u64) -> Self {
        Self { count: 1, ms: ms as f64, updated: Instant::now() }
    }
    pub fn estimate(&self) -> Option<f64> {
        (self.count >= 3 && self.updated.elapsed().as_secs() < 300).then_some(self.ms)
    }
}

/// Comparison contexts retained per account.
const MAX_SAMPLES: usize = 64;

/// Records a cold first-response time. Bounded per account: a new comparison
/// context replaces the stalest one instead of discarding every sample.
pub fn record(samples: &mut HashMap<(String, Context), Sample>, key: (String, Context), ms: u64) {
    if !samples.contains_key(&key)
        && samples.len() >= MAX_SAMPLES
        && let Some(stalest) = samples.iter().min_by_key(|(_, s)| s.updated).map(|(k, _)| k.clone())
    {
        samples.remove(&stalest);
    }
    samples.entry(key).and_modify(|s| s.add(ms)).or_insert_with(|| Sample::new(ms));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn size_buckets_stop_counting_past_the_largest_boundary() {
        let small = json!({"input":"x".repeat(100)});
        assert_eq!(serialized_len(&small, 1 << 20), small.to_string().len());
        let large = json!({"input":"x".repeat(3 << 20)});
        assert!(serialized_len(&large, 1 << 20) > 1 << 20);
        assert_eq!(Context::new(&large, "http").bytes, 3);
        assert_eq!(Context::new(&json!({"input":"x".repeat(200 << 10)}), "http").bytes, 2);
    }

    #[test]
    fn a_full_account_replaces_only_its_stalest_context() {
        let context = |effort: usize| Context::new(&json!({"reasoning":{"effort":format!("e{effort}")}}), "http");
        let mut samples = HashMap::new();
        for i in 0..MAX_SAMPLES {
            record(&mut samples, ("m".into(), context(i)), 100);
        }
        samples.get_mut(&("m".into(), context(7))).unwrap().updated -= std::time::Duration::from_secs(60);
        // Updating a known context never evicts.
        record(&mut samples, ("m".into(), context(3)), 50);
        assert_eq!(samples.len(), MAX_SAMPLES);
        assert!(samples.contains_key(&("m".into(), context(7))));
        record(&mut samples, ("m".into(), context(MAX_SAMPLES)), 100);
        assert_eq!(samples.len(), MAX_SAMPLES);
        assert!(!samples.contains_key(&("m".into(), context(7))));
        assert_eq!(samples[&("m".into(), context(3))].count, 2);
    }
}
