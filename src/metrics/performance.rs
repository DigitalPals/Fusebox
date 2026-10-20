//! Bounded process-local measurement; labels never contain session or prompt ids.
use crate::{state::RequestLog, usage::types::Observation};
use std::{collections::BTreeMap, fmt::Write};

const MAX_SERIES: usize = 128;
const BOUNDS: [f64; 12] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1., 2.5, 5., 30., 120.];
#[derive(Default)]
struct Histogram {
    buckets: [u64; 12],
    count: u64,
    sum: f64,
}
impl Histogram {
    fn add(&mut self, ms: u64) {
        let seconds = ms as f64 / 1000.;
        self.count += 1;
        self.sum += seconds;
        for (i, limit) in BOUNDS.iter().enumerate() {
            if seconds <= *limit {
                self.buckets[i] += 1;
            }
        }
    }
    fn render(&self, out: &mut String, name: &str, labels: &str) {
        for (limit, count) in BOUNDS.iter().zip(self.buckets) {
            let _ = writeln!(out, "{name}_bucket{{{labels},le=\"{limit}\"}} {count}");
        }
        let _ = writeln!(
            out,
            "{name}_bucket{{{labels},le=\"+Inf\"}} {}\n{name}_count{{{labels}}} {}\n{name}_sum{{{labels}}} {}",
            self.count, self.count, self.sum
        );
    }
}
#[derive(Default)]
struct Series {
    tokens: [u64; 5],
    known: [u64; 5],
    unknown: [u64; 5],
    requests: u64,
    retries: u64,
    total: Histogram,
    ttft: Histogram,
    prepare: Histogram,
    headers: Histogram,
}
#[derive(Default)]
pub struct Performance {
    series: BTreeMap<(String, String, &'static str), Series>,
    ws: BTreeMap<&'static str, u64>,
}
impl Performance {
    fn series(&mut self, provider: &str, model: &str, transport: &'static str) -> &mut Series {
        let provider = match provider {
            "codex" => "openai",
            "claude" => "anthropic",
            other => other,
        };
        let model = crate::diagnostics::model_label(model);
        let mut key = (provider.to_string(), model.to_string(), if transport == "ws" { "ws" } else { "http" });
        if !self.series.contains_key(&key) && self.series.len() >= MAX_SERIES {
            key = ("other".into(), "other".into(), "other");
        }
        self.series.entry(key).or_default()
    }
    pub fn attempt(&mut self, observation: &Observation, transport: &'static str) {
        let row = self.series(
            &observation.provider,
            observation.actual_model.as_deref().or(observation.requested_model.as_deref()).unwrap_or("unknown"),
            transport,
        );
        let t = &observation.tokens;
        for (i, value) in [t.input, t.cache_read, t.cache_write, t.output, t.reasoning].into_iter().enumerate() {
            if let Some(value) = value {
                row.tokens[i] = row.tokens[i].saturating_add(value);
                row.known[i] += 1;
            } else {
                row.unknown[i] += 1;
            }
        }
    }
    pub fn request(
        &mut self,
        log: &RequestLog,
        observation: Option<&Observation>,
        prepare: Option<u64>,
        headers: Option<u64>,
    ) {
        let provider = observation.map(|o| o.provider.as_str()).unwrap_or(&log.provider);
        let model =
            observation.and_then(|o| o.actual_model.as_deref().or(o.requested_model.as_deref())).unwrap_or(&log.model);
        let row = self.series(provider, model, log.transport);
        row.requests += 1;
        row.retries += u64::from(log.attempts.saturating_sub(1));
        row.total.add(log.latency_ms);
        if let Some(ms) = log.ttft_ms {
            row.ttft.add(ms);
        }
        if let Some(ms) = prepare {
            row.prepare.add(ms);
        }
        if let Some(ms) = headers {
            row.headers.add(ms);
        }
    }
    pub fn websocket(&mut self, reason: &'static str) {
        *self.ws.entry(reason).or_default() += 1;
    }
    pub fn render(&self, out: &mut String) {
        for (name, help) in [
            ("total", "Request duration"),
            ("ttft", "Time to first content"),
            ("prepare", "Final attempt preparation"),
            ("headers", "Final attempt upstream headers"),
        ] {
            let _ = writeln!(
                out,
                "# HELP fusebox_{name}_seconds {help} in seconds.\n# TYPE fusebox_{name}_seconds histogram"
            );
        }
        let _ = writeln!(
            out,
            "# HELP fusebox_usage_tokens_total Reported disjoint tokens across all provider attempts; reasoning is an output subset.\n# TYPE fusebox_usage_tokens_total counter\n# TYPE fusebox_usage_fields_total counter\n# TYPE fusebox_routed_requests_total counter\n# TYPE fusebox_retries_total counter\n# TYPE fusebox_websocket_turns_total counter"
        );
        for ((provider, model, transport), row) in &self.series {
            let labels = format!(
                "provider=\"{}\",model=\"{}\",transport=\"{transport}\"",
                super::label(provider),
                super::label(model)
            );
            for (i, kind) in ["input", "cache_read", "cache_write", "output", "reasoning"].iter().enumerate() {
                let _ = writeln!(
                    out,
                    "fusebox_usage_tokens_total{{{labels},kind=\"{kind}\"}} {}\nfusebox_usage_fields_total{{{labels},kind=\"{kind}\",state=\"reported\"}} {}\nfusebox_usage_fields_total{{{labels},kind=\"{kind}\",state=\"unknown\"}} {}",
                    row.tokens[i], row.known[i], row.unknown[i]
                );
            }
            let _ = writeln!(
                out,
                "fusebox_routed_requests_total{{{labels}}} {}\nfusebox_retries_total{{{labels}}} {}",
                row.requests, row.retries
            );
            for (name, h) in
                [("total", &row.total), ("ttft", &row.ttft), ("prepare", &row.prepare), ("headers", &row.headers)]
            {
                h.render(out, &format!("fusebox_{name}_seconds"), &labels);
            }
        }
        for (reason, count) in &self.ws {
            let _ = writeln!(out, "fusebox_websocket_turns_total{{reason=\"{reason}\"}} {count}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn series_are_bounded_and_missing_usage_stays_unknown() {
        let mut p = Performance::default();
        let mut o = Observation::new("proxy", "event".into(), "codex", 0);
        o.tokens.cache_write = Some(0);
        for n in 0..500 {
            o.actual_model = Some(format!("model-{n}"));
            p.attempt(&o, "http");
        }
        assert!(p.series.len() <= MAX_SERIES + 1);
        let mut out = String::new();
        p.render(&mut out);
        assert!(out.contains("kind=\"input\",state=\"unknown\""));
        assert!(out.contains("kind=\"cache_write\",state=\"reported\""));
        assert!(!out.contains("event"));
    }
}
