//! A rolling week of quota supply and demand. Observed consumption is cumulative across
//! resets; a reset replenishes a forecast account only at its reported time.
use std::collections::VecDeque;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::quota::Window;

const LOOKBACK_HOURS: i64 = 24;
const LEARN_HOURS: i64 = 6;
const MAX_GAP_MINUTES: i64 = 60;
const SAMPLE_MINUTES: i64 = 5;
const RESET_JITTER_SECONDS: i64 = 600;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Sample {
    at: DateTime<Utc>,
    spent: f64,
}

/// Five-minute samples of observed quota consumption, never token-price estimates.
/// Kept in the existing identity-bound attribution journal, including through restarts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct History {
    pub name: String,
    pub model: Option<String>,
    resets_at: Option<DateTime<Utc>>,
    high: f64,
    at: DateTime<Utc>,
    spent: f64,
    samples: VecDeque<Sample>,
}

impl History {
    pub fn observe(histories: &mut Vec<Self>, windows: &[Window], now: DateTime<Utc>) {
        for w in windows.iter().filter(|w| w.name == "week" && w.model.is_none()) {
            if !w.used.is_finite() || !(0.0..=100.0).contains(&w.used) || w.resets_at.is_some_and(|r| r <= now) {
                continue;
            }
            let Some(h) = histories.iter_mut().find(|h| h.name == w.name && h.model == w.model) else {
                histories.push(Self {
                    name: w.name.clone(),
                    model: w.model.clone(),
                    resets_at: w.resets_at,
                    high: w.used,
                    at: now,
                    spent: 0.0,
                    samples: VecDeque::from([Sample { at: now, spent: 0.0 }]),
                });
                continue;
            };
            // A late response from an old window cannot undo an observed reset.
            if now < h.at
                || h.resets_at.zip(w.resets_at).is_some_and(|(a, b)| (a - b).num_seconds() >= RESET_JITTER_SECONDS)
            {
                continue;
            }
            if now - h.at > Duration::minutes(MAX_GAP_MINUTES) {
                // Unknown activity during an outage is not evidence of zero consumption.
                h.samples.clear();
                h.high = w.used;
                h.resets_at = w.resets_at;
            } else {
                let moved =
                    h.resets_at.zip(w.resets_at).is_some_and(|(a, b)| (b - a).num_seconds() >= RESET_JITTER_SECONDS);
                let dropped = w.used + 5.0 < h.high || (w.used < 0.5 && h.high >= 2.0);
                if moved || dropped {
                    // The already observed use stays in history. Only new-window usage
                    // is added; never count the refill itself as negative consumption.
                    h.spent += w.used;
                    h.high = w.used;
                    h.resets_at = w.resets_at;
                } else {
                    h.spent += (w.used - h.high).max(0.0);
                    h.high = h.high.max(w.used);
                    h.resets_at = h.resets_at.or(w.resets_at);
                }
            }
            h.at = now;
            if h.samples.back().is_none_or(|s| now - s.at >= Duration::minutes(SAMPLE_MINUTES)) {
                h.samples.push_back(Sample { at: now, spent: h.spent });
            }
            let start = now - Duration::hours(LOOKBACK_HOURS);
            // Keep an anchor before the cutoff for interpolation, with bounded storage.
            while h.samples.len() > 2 && h.samples[1].at <= start {
                h.samples.pop_front();
            }
        }
    }

    fn rate(&self, now: DateTime<Utc>) -> Result<(f64, f64), &'static str> {
        if now < self.at || now - self.at > Duration::minutes(MAX_GAP_MINUTES) {
            return Err("stale_readings");
        }
        let first = self.samples.front().ok_or("collecting_history")?;
        let start = (self.at - Duration::hours(LOOKBACK_HOURS)).max(first.at);
        let seconds = (self.at - start).num_milliseconds() as f64 / 1000.0;
        if seconds < LEARN_HOURS as f64 * 3600.0 {
            return Err("collecting_history");
        }
        let mut baseline = first.spent;
        if start > first.at {
            let next = self.samples.get(1).ok_or("collecting_history")?;
            let share = (start - first.at).num_milliseconds() as f64 / (next.at - first.at).num_milliseconds() as f64;
            baseline += share * (next.spent - first.spent);
        }
        let rate = (self.spent - baseline) / seconds;
        if !rate.is_finite() || rate < 0.0 {
            return Err("invalid_history");
        }
        Ok((rate, seconds / 3600.0))
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Outlook {
    pub state: &'static str,
    pub reason: Option<&'static str>,
    pub horizon_at: DateTime<Utc>,
    pub next_reset_at: Option<DateTime<Utc>>,
    pub min_remaining: Option<f64>,
    pub min_at: Option<DateTime<Utc>>,
    pub exhausts_at: Option<DateTime<Utc>>,
    pub recovers_at: Option<DateTime<Utc>>,
    pub rate_per_day: Option<f64>,
    pub observed_hours: Option<f64>,
    pub learning_until: Option<DateTime<Utc>>,
}

impl Outlook {
    pub fn calculate(members: &[crate::pools::Member<'_>], priced: bool, now: DateTime<Utc>) -> Self {
        let horizon = now + Duration::days(7);
        let weekly =
            |m: &crate::pools::Member<'_>| m.windows.iter().find(|w| w.name == "week" && w.model.is_none()).cloned();
        let mut out = Self {
            state: "unavailable",
            reason: None,
            horizon_at: horizon,
            next_reset_at: members.iter().filter_map(weekly).filter_map(|w| w.resets_at).filter(|r| *r > now).min(),
            min_remaining: None,
            min_at: None,
            exhausts_at: None,
            recovers_at: None,
            rate_per_day: None,
            observed_hours: None,
            learning_until: None,
        };
        let mut stock = Vec::new();
        let (mut total, mut rate, mut hours) = (0.0, 0.0, 24.0_f64);
        for m in members {
            let reason = if !m.forecast_available {
                Some("unavailable_account")
            } else if let Some(w) = weekly(m) {
                if !w.used.is_finite() || !(0.0..=100.0).contains(&w.used) {
                    Some("missing_quota")
                } else if let Some(reset) = w.resets_at.filter(|r| *r > now && *r <= horizon) {
                    let weight =
                        if priced { crate::pools::plan_weight(m.provider, m.plan).unwrap_or(1.0) } else { 1.0 };
                    total += weight;
                    stock.push(Stock { capacity: 100.0 * weight, left: (100.0 - w.used) * weight, reset });
                    match m.history.iter().find(|h| h.name == "week" && h.model.is_none()) {
                        Some(h)
                            if h.resets_at
                                .zip(w.resets_at)
                                .is_some_and(|(a, b)| (a - b).num_seconds().abs() >= RESET_JITTER_SECONDS) =>
                        {
                            Some("awaiting_reset")
                        }
                        Some(h) => match h.rate(now) {
                            Ok((r, observed)) => {
                                rate += r * weight;
                                hours = hours.min(observed);
                                None
                            }
                            Err(reason) => {
                                if reason == "collecting_history" {
                                    let ready = h.samples.front().map(|s| s.at + Duration::hours(LEARN_HOURS));
                                    out.learning_until = out.learning_until.max(ready);
                                }
                                Some(reason)
                            }
                        },
                        None => {
                            out.learning_until = Some(now + Duration::hours(LEARN_HOURS));
                            Some("collecting_history")
                        }
                    }
                } else {
                    Some(if w.resets_at.is_some_and(|r| r <= now) { "awaiting_reset" } else { "missing_reset" })
                }
            } else {
                Some("missing_quota")
            };
            // Missing/stale data takes precedence over a newly connected account learning.
            if let Some(reason) = reason
                && (out.reason.is_none() || reason != "collecting_history")
            {
                out.reason = Some(reason);
            }
        }
        if let Some(reason) = out.reason {
            if reason == "collecting_history" {
                out.state = "learning";
            }
            return out;
        }
        if total <= 0.0 {
            out.reason = Some("missing_quota");
            return out;
        }
        out.rate_per_day = Some(rate * 86_400.0 / total);
        out.observed_hours = Some(hours);
        simulate(&mut out, stock, rate, total, now);
        out.state = if out.exhausts_at.is_some() {
            "run_out"
        } else if out.min_remaining.is_some_and(|r| r < 10.0) {
            "tight"
        } else {
            "on_track"
        };
        out
    }
}

struct Stock {
    capacity: f64,
    left: f64,
    reset: DateTime<Utc>,
}

fn simulate(out: &mut Outlook, mut stock: Vec<Stock>, rate: f64, total: f64, now: DateTime<Utc>) {
    let balance = |stock: &[Stock]| stock.iter().map(|s| s.left).sum::<f64>();
    let mut minimum = balance(&stock);
    out.min_at = Some(now);
    let mut at = now;
    while at < out.horizon_at {
        stock.sort_by_key(|s| s.reset);
        let next = stock[0].reset.min(out.horizon_at);
        let available = balance(&stock);
        let mut demand = rate * (next - at).num_milliseconds() as f64 / 1000.0;
        if demand > available + 1e-9 && out.exhausts_at.is_none() {
            let empty = at + Duration::milliseconds((available / rate * 1000.0).round() as i64);
            out.exhausts_at = Some(empty);
            out.recovers_at = Some(stock[0].reset);
            out.min_at = Some(empty);
        }
        for s in &mut stock {
            let take = demand.min(s.left);
            s.left -= take;
            demand -= take;
        }
        let left = balance(&stock);
        if left < minimum {
            minimum = left;
            if out.exhausts_at.is_none() {
                out.min_at = Some(next);
            }
        }
        // All simultaneous resets refill together. Unused allowance never rolls over.
        for s in stock.iter_mut().filter(|s| s.reset == next) {
            s.left = s.capacity;
            s.reset += Duration::days(7);
        }
        at = next;
    }
    out.min_remaining = Some((minimum / total).clamp(0.0, 100.0));
}

#[cfg(test)]
pub(crate) fn test_history(w: &Window, now: DateTime<Utc>, per_day: f64) -> History {
    History {
        name: w.name.clone(),
        model: w.model.clone(),
        resets_at: w.resets_at,
        high: w.used,
        at: now,
        spent: per_day / 4.0,
        samples: VecDeque::from([
            Sample { at: now - Duration::hours(6), spent: 0.0 },
            Sample { at: now, spent: per_day / 4.0 },
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pools::Member;

    fn now() -> DateTime<Utc> {
        "2026-10-26T10:42:00Z".parse().unwrap()
    }
    fn week(used: f64, hours: f64) -> Window {
        Window {
            name: "week".into(),
            model: None,
            used,
            resets_at: Some(now() + Duration::seconds((hours * 3600.0) as i64)),
        }
    }
    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-7, "{a} != {b}");
    }
    fn forecast(windows: &[Window], rates: &[f64]) -> Outlook {
        let histories: Vec<_> = windows.iter().zip(rates).map(|(w, r)| vec![test_history(w, now(), *r)]).collect();
        let members: Vec<_> = windows
            .iter()
            .zip(&histories)
            .map(|(w, h)| Member {
                id: "fixture",
                provider: "claude",
                plan: Some("max_20x"),
                subscription: true,
                disabled: false,
                windows: std::slice::from_ref(w),
                history: h,
                forecast_available: true,
                no_five_hour: false,
            })
            .collect();
        Outlook::calculate(&members, true, now())
    }

    #[test]
    fn screenshot_resets_are_applied_in_time_and_the_lowest_reserve_is_before_a_later_reset() {
        let out = forecast(&[week(100.0, 54.0), week(0.0, 101.0), week(59.0, 76.0), week(98.0, 3.3)], &[14.0; 4]);
        assert_eq!(out.state, "on_track");
        assert_eq!(out.next_reset_at, Some(now() + Duration::minutes(198)));
        assert_eq!(out.min_at, Some(now() + Duration::hours(54)));
        near(out.min_remaining.unwrap(), 29.25);
        near(out.rate_per_day.unwrap(), 14.0);
        assert!(out.exhausts_at.is_none());
    }

    #[test]
    fn a_reset_later_today_cannot_hide_an_outage_before_it() {
        let out = forecast(&[week(98.0, 4.0), week(100.0, 48.0)], &[24.0; 2]);
        assert_eq!(out.state, "run_out");
        near(out.min_remaining.unwrap(), 0.0);
        assert_eq!(out.exhausts_at, Some(now() + Duration::hours(1)));
        assert_eq!(out.recovers_at, Some(now() + Duration::hours(4)));
        assert_eq!(out.min_at, out.exhausts_at);
    }

    #[test]
    fn forecasts_continue_after_the_last_reset_and_discard_unspent_allowance() {
        let out = forecast(&[week(0.0, 1.0), week(0.0, 1.0)], &[20.0; 2]);
        assert_eq!(out.state, "run_out");
        // Plenty now, but both renew in an hour. Their old unused quota expires.
        assert_eq!(out.exhausts_at, Some(now() + Duration::hours(121)));
        assert_eq!(out.recovers_at, Some(now() + Duration::hours(169)));
        // Reaching zero exactly at a simultaneous reset creates no period of exhaustion.
        let exact = forecast(&[week(99.0, 1.0), week(99.0, 1.0)], &[24.0 / 7.0; 2]);
        assert_ne!(exact.state, "run_out");
        let exact = forecast(&[week(100.0 / 7.0, 24.0), week(100.0 / 7.0, 24.0)], &[600.0 / 7.0; 2]);
        assert_eq!(exact.exhausts_at, Some(now() + Duration::hours(52)));
    }

    #[test]
    fn zero_consumption_is_supported_only_with_observed_history() {
        let out = forecast(&[week(30.0, 24.0), week(50.0, 48.0)], &[0.0; 2]);
        assert_eq!(out.state, "on_track");
        near(out.min_remaining.unwrap(), 60.0);
        assert_eq!(out.min_at, Some(now()));
        let ws = [week(0.0, 24.0)];
        let m = Member {
            id: "a",
            provider: "claude",
            plan: None,
            subscription: true,
            disabled: false,
            windows: &ws,
            history: &[],
            forecast_available: true,
            no_five_hour: false,
        };
        let out = Outlook::calculate(&[m], false, now());
        assert_eq!(out.state, "learning");
        assert!(out.min_remaining.is_none() && out.rate_per_day.is_none());
        assert_eq!(out.learning_until, Some(now() + Duration::hours(6)));
        let out = Outlook::calculate(&[Member { windows: &[], ..m }], false, now());
        assert_eq!(out.reason, Some("missing_quota"));
        let out = Outlook::calculate(&[Member { forecast_available: false, ..m }], false, now());
        assert_eq!(out.reason, Some("unavailable_account"));
    }

    #[test]
    fn unknown_past_or_invalid_resets_and_stale_history_never_produce_a_healthy_forecast() {
        for reset in [None, Some(now()), Some(now() + Duration::days(8))] {
            let out = forecast(&[Window { resets_at: reset, ..week(10.0, 24.0) }], &[10.0]);
            assert_eq!(out.state, "unavailable");
            assert!(out.min_remaining.is_none());
        }
        let ws = [week(10.0, 24.0)];
        let hs = [test_history(&ws[0], now() - Duration::hours(2), 10.0)];
        let m = Member {
            id: "a",
            provider: "claude",
            plan: None,
            subscription: true,
            disabled: false,
            windows: &ws,
            history: &hs,
            forecast_available: true,
            no_five_hour: false,
        };
        let out = Outlook::calculate(&[m], false, now());
        assert_eq!(out.reason, Some("stale_readings"));
    }

    #[test]
    fn mixed_plans_weight_both_consumption_and_capacity() {
        let ws = [week(20.0, 24.0), week(80.0, 48.0)];
        let hs = [test_history(&ws[0], now(), 0.0), test_history(&ws[1], now(), 0.0)];
        let members: Vec<_> = (0..2)
            .map(|i| Member {
                id: "a",
                provider: "claude",
                plan: Some(if i == 0 { "max_20x" } else { "pro" }),
                subscription: true,
                disabled: false,
                windows: &ws[i..i + 1],
                history: &hs[i..i + 1],
                forecast_available: true,
                no_five_hour: false,
            })
            .collect();
        near(Outlook::calculate(&members, true, now()).min_remaining.unwrap(), 820.0 / 11.0);
        near(Outlook::calculate(&members, false, now()).min_remaining.unwrap(), 50.0);
    }

    #[test]
    fn consumption_history_survives_resets_serialization_and_noisy_readings() {
        let mut hs = Vec::new();
        let start = now() - Duration::hours(6);
        for i in 0..=12 {
            History::observe(&mut hs, &[week(80.0 + i as f64, 1.0)], start + Duration::minutes(i * 30));
        }
        near(hs[0].rate(now()).unwrap().0 * 86_400.0, 48.0);
        let saved = serde_json::to_string(&hs).unwrap();
        let mut restored: Vec<History> = serde_json::from_str(&saved).unwrap();
        // A banked reset moves the weekly reset; all 12 observed points remain.
        History::observe(&mut restored, &[week(0.0, 168.0)], now() + Duration::minutes(1));
        near(restored[0].spent, 12.0);
        History::observe(&mut restored, &[week(94.0, 1.0)], now() + Duration::minutes(2));
        near(restored[0].spent, 12.0); // stale response from before reset
        History::observe(&mut restored, &[week(2.0, 168.0)], now() + Duration::minutes(30));
        History::observe(&mut restored, &[week(1.0, 168.0)], now() + Duration::minutes(31));
        History::observe(&mut restored, &[week(2.0, 168.0)], now() + Duration::minutes(32));
        near(restored[0].spent, 14.0); // downward rounding does not count the same point twice
        assert!(restored[0].rate(now() + Duration::minutes(32)).is_ok());
    }

    #[test]
    fn history_has_a_bounded_rolling_day_and_relearns_after_gaps() {
        let mut hs = Vec::new();
        for minute in 0..=48 * 60 {
            History::observe(&mut hs, &[week(minute as f64 / 60.0, 168.0)], now() + Duration::minutes(minute));
        }
        assert!(hs[0].samples.len() <= 290);
        let end = now() + Duration::hours(48);
        near(hs[0].rate(end).unwrap().0 * 86_400.0, 24.0);
        assert_eq!(hs[0].rate(end + Duration::minutes(61)), Err("stale_readings"));
        History::observe(&mut hs, &[week(80.0, 168.0)], end + Duration::hours(2));
        assert_eq!(hs[0].rate(end + Duration::hours(2)), Err("collecting_history"));
        assert_eq!(hs[0].samples.len(), 1);
    }
}
