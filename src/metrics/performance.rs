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
    /// One contiguous group per metric family, as the exposition format requires.
    pub fn render(&self, out: &mut String) {
        const KINDS: [&str; 5] = ["input", "cache_read", "cache_write", "output", "reasoning"];
        let rows: Vec<(String, &Series)> = self
            .series
            .iter()
            .map(|((provider, model, transport), row)| {
                let labels = format!(
                    "provider=\"{}\",model=\"{}\",transport=\"{transport}\"",
                    super::label(provider),
                    super::label(model)
                );
                (labels, row)
            })
            .collect();
        let family = |out: &mut String, name: &str, kind: &str, help: &str| {
            let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
        };

        family(
            out,
            "fusebox_usage_tokens_total",
            "counter",
            "Reported disjoint tokens across all provider attempts; reasoning is an output subset.",
        );
        for (labels, row) in &rows {
            for (i, kind) in KINDS.iter().enumerate() {
                let _ = writeln!(out, "fusebox_usage_tokens_total{{{labels},kind=\"{kind}\"}} {}", row.tokens[i]);
            }
        }
        family(
            out,
            "fusebox_usage_fields_total",
            "counter",
            "Provider attempts whose usage reported or omitted each token kind.",
        );
        for (labels, row) in &rows {
            for (i, kind) in KINDS.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "fusebox_usage_fields_total{{{labels},kind=\"{kind}\",state=\"reported\"}} {}\nfusebox_usage_fields_total{{{labels},kind=\"{kind}\",state=\"unknown\"}} {}",
                    row.known[i], row.unknown[i]
                );
            }
        }
        family(out, "fusebox_routed_requests_total", "counter", "Finished routed requests.");
        for (labels, row) in &rows {
            let _ = writeln!(out, "fusebox_routed_requests_total{{{labels}}} {}", row.requests);
        }
        family(out, "fusebox_retries_total", "counter", "Provider attempts beyond the first, per finished request.");
        for (labels, row) in &rows {
            let _ = writeln!(out, "fusebox_retries_total{{{labels}}} {}", row.retries);
        }
        type Pick = fn(&Series) -> &Histogram;
        let histograms: [(&str, &str, Pick); 4] = [
            ("total", "Request duration", |r| &r.total),
            ("ttft", "Time to first content", |r| &r.ttft),
            ("prepare", "Final attempt preparation", |r| &r.prepare),
            ("headers", "Final attempt upstream headers", |r| &r.headers),
        ];
        for (name, help, histogram) in histograms {
            let name = format!("fusebox_{name}_seconds");
            family(out, &name, "histogram", &format!("{help} in seconds."));
            for (labels, row) in &rows {
                histogram(row).render(out, &name, labels);
            }
        }
        family(
            out,
            "fusebox_websocket_turns_total",
            "counter",
            "Codex WebSocket turns by native completion or fallback reason.",
        );
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

    #[test]
    fn every_family_is_one_described_group() {
        let mut p = Performance::default();
        let mut o = Observation::new("proxy", "attempt".into(), "codex", 0);
        for model in ["a", "b"] {
            o.actual_model = Some(model.into());
            p.attempt(&o, "http");
        }
        p.websocket("native");
        let mut out = String::new();
        p.render(&mut out);
        let family = |line: &str| {
            let name = line.split(['{', ' ']).next().unwrap();
            ["_bucket", "_count", "_sum"]
                .iter()
                .find_map(|suffix| name.strip_suffix(suffix).filter(|n| n.ends_with("_seconds")))
                .unwrap_or(name)
                .to_string()
        };
        let mut finished = std::collections::HashSet::new();
        let (mut current, mut described) = (String::new(), std::collections::HashSet::new());
        for line in out.lines() {
            if let Some(rest) = line.strip_prefix("# HELP ").or_else(|| line.strip_prefix("# TYPE ")) {
                described.insert(rest.split(' ').next().unwrap().to_string());
                continue;
            }
            let name = family(line);
            assert!(described.contains(&name), "{name} has no HELP/TYPE");
            if name != current {
                assert!(finished.insert(current.clone()), "{current} is split");
                assert!(!finished.contains(&name), "{name} appears in two groups");
                current = name;
            }
        }
    }
}
