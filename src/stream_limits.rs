//! Configurable bounds for upstream streaming responses.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Limits {
    pub event_bytes: usize,
    pub media_event_bytes: usize,
    pub bootstrap_bytes: usize,
    /// Absolute time for headers and the first event together. 0 preserves legacy timing.
    pub pre_output_timeout_seconds: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            event_bytes: 16 << 20,
            media_event_bytes: 64 << 20,
            bootstrap_bytes: 2 << 20,
            pre_output_timeout_seconds: 0,
        }
    }
}
impl Limits {
    pub fn validate(&self) -> Result<(), String> {
        if !(1024..=256 << 20).contains(&self.event_bytes)
            || !(self.event_bytes..=256 << 20).contains(&self.media_event_bytes)
            || !(1024..=64 << 20).contains(&self.bootstrap_bytes)
            || self.pre_output_timeout_seconds > 3600
        {
            return Err(
                "invalid stream-limits (event/media: 1 KiB–256 MiB; bootstrap: 1 KiB–64 MiB; timeout: 0–3600s)".into(),
            );
        }
        Ok(())
    }
    pub fn for_model(&self, model: &str) -> usize {
        if model.contains("image") || model.contains("audio") || model.contains("video") {
            self.media_event_bytes
        } else {
            self.event_bytes
        }
    }
}
