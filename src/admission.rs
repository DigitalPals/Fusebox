//! Optional admission for HTTP POST requests, through response completion.
//! Counts decoded request bytes; parsed JSON and responses need extra memory.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub const MAX_BODY_SIZE: usize = 256 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    pub text_body_bytes: usize,
    pub media_body_bytes: usize,
    /// Zero means unlimited. Includes collection, provider work and streaming.
    pub max_in_flight: usize,
    /// Zero means unlimited. Includes bodies received with compression.
    pub max_in_flight_body_bytes: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            text_body_bytes: MAX_BODY_SIZE,
            media_body_bytes: MAX_BODY_SIZE,
            max_in_flight: 0,
            max_in_flight_body_bytes: 0,
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if [self.text_body_bytes, self.media_body_bytes].iter().any(|n| *n == 0 || *n > MAX_BODY_SIZE) {
            return Err("request-limits body sizes must be between 1 and 268435456 bytes".into());
        }
        Ok(())
    }

    pub fn body_limit(&self, path: &str) -> usize {
        if path.starts_with("/v1/images/") || path.starts_with("/v1/videos/") {
            self.media_body_bytes
        } else {
            self.text_body_bytes
        }
    }
}

#[derive(Default)]
struct Usage {
    requests: usize,
    bytes: usize,
}

#[derive(Default)]
pub struct Admission {
    usage: Mutex<Usage>,
}

#[derive(Debug, Clone, Copy)]
pub struct Full;
impl std::fmt::Display for Full {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("request capacity exhausted; retry later")
    }
}
impl std::error::Error for Full {}

pub struct Permit {
    admission: Arc<Admission>,
    bytes: AtomicUsize,
    limit: usize,
}

impl Admission {
    pub fn acquire(self: &Arc<Self>, cfg: &Config) -> Result<Arc<Permit>, Full> {
        let mut usage = self.usage.lock();
        if (cfg.max_in_flight > 0 && usage.requests >= cfg.max_in_flight)
            || (cfg.max_in_flight_body_bytes > 0 && usage.bytes >= cfg.max_in_flight_body_bytes)
        {
            return Err(Full);
        }
        usage.requests += 1;
        Ok(Arc::new(Permit {
            admission: self.clone(),
            bytes: AtomicUsize::new(0),
            limit: cfg.max_in_flight_body_bytes,
        }))
    }
}

impl Permit {
    pub fn reserve(&self, bytes: usize) -> Result<(), Full> {
        let mut usage = self.admission.usage.lock();
        if self.limit > 0 && bytes > self.limit.saturating_sub(usage.bytes) {
            return Err(Full);
        }
        usage.bytes += bytes;
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut usage = self.admission.usage.lock();
        usage.requests -= 1;
        usage.bytes -= self.bytes.load(Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_is_fail_fast_and_releases_bytes_on_cancellation() {
        let admission = Arc::new(Admission::default());
        let cfg = Config { max_in_flight: 2, max_in_flight_body_bytes: 10, ..Default::default() };
        let first = admission.acquire(&cfg).unwrap();
        let second = admission.acquire(&cfg).unwrap();
        assert!(admission.acquire(&cfg).is_err());
        first.reserve(6).unwrap();
        second.reserve(4).unwrap();
        assert!(second.reserve(1).is_err());
        drop(first);
        second.reserve(6).unwrap();
        assert!(admission.acquire(&cfg).is_err());
        drop(second);
        let next = admission.acquire(&cfg).unwrap();
        next.reserve(10).unwrap();
    }
}
