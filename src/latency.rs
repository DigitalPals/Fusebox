//! Optional tie-breaking for comparable cold, successful first turns only.
use serde_json::Value;
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
        let len = body.to_string().len();
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
