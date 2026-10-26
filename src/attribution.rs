//! Quota attribution: which clients, sessions and models used each subscription
//! window.
//!
//! Subscriptions are limited in percent of a 5-hour or weekly window, not in
//! dollars, and the providers report only the total. Fusebox reads that total
//! after every Claude and ChatGPT response and from the usage endpoints, and it
//! knows every attempt that finished on the account in between. When a reading
//! shows the window's use rise, the rise is split over the attempts that finished
//! since the previous rise, weighted by their estimated API list price (or by
//! weighted tokens when a model has no price). Readings are often whole percents,
//! so attempts wait until the reported use actually moves and then share it. A
//! rise with no Fusebox attempt to explain it is counted as use outside Fusebox
//! (claude.ai, the desktop app, another tool). Use already on the window when
//! Fusebox began following it is kept apart as `before`.
//!
//! These are estimates, labelled so: the provider's own accounting is per token
//! and model, and Fusebox only sees the sum. They live in each account's state and
//! survive restarts through `<auth-dir>/.quota-attribution.state`.
use std::collections::{BTreeMap, VecDeque};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::quota::Window;
use crate::usage::types::Observation;

/// Attempts kept for attribution, per account: at most this many, at most this long.
const KEEP_USES: usize = 20_000;
const KEEP_FOR_DAYS: i64 = 8;
/// Attempts written to disk: the ones still waiting for a reading.
const SAVE_USES: usize = 2_000;
/// Reset times reported a little differently (Codex reports seconds-until) are one window.
const SAME_RESET_SECONDS: i64 = 600;
/// The usage endpoint and response headers round one window differently, so its
/// readings disagree by a point or two. Only a bigger drop, or one to nothing, is a reset.
const NOISE_POINTS: f64 = 5.0;
/// Sessions kept per window; the smallest go first.
const KEEP_SESSIONS: usize = 200;
/// Finished windows kept per account, newest first.
const KEEP_PREVIOUS: usize = 8;

/// One finished attempt on the account.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Use {
    pub at: DateTime<Utc>,
    /// Estimated API list price in nanodollars, when the model has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// Input, cache writes, cache reads and output weighted like typical prices.
    pub tokens: f64,
    /// Named client id; `app:<program>` without one; `key:<prefix>` for a shared key
    /// with no known program; `outside` (in history only) for use outside Fusebox.
    pub client: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Use {
    /// From a finished attempt's usage record; `program` names the client app when
    /// no named client key was used.
    pub fn from_observation(o: &Observation, cost: Option<i64>, program: Option<&str>) -> Option<Self> {
        let t = &o.tokens;
        let tokens = t.input.unwrap_or(0) as f64
            + 1.25 * t.cache_write.unwrap_or(0) as f64
            + 0.1 * t.cache_read.unwrap_or(0) as f64
            + 5.0 * t.output.unwrap_or(0) as f64;
        if tokens <= 0.0 && cost.unwrap_or(0) <= 0 {
            return None;
        }
        // The key names the client, as in the usage records: a named client's id, or
        // a shared key's hash. The program only names a request without a key.
        let client = match (&o.client_id, program) {
            (Some(id), _) => id.clone(),
            (None, Some(p)) => format!("app:{p}"),
            (None, None) => String::new(),
        };
        Some(Self {
            at: Utc::now(),
            cost: cost.filter(|c| *c > 0).map(|c| c as f64),
            tokens,
            client,
            session: o.session_id.clone(),
            model: o.actual_model.clone().or_else(|| o.requested_model.clone()),
        })
    }
}

/// What one window instance was used by, in percentage points of the window.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Share {
    /// "5h", "week", ... as in the quota windows.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub resets_at: Option<DateTime<Utc>>,
    /// When Fusebox began following this window.
    pub since: DateTime<Utc>,
    /// Already used then, by whatever used it.
    pub before: f64,
    /// The latest reading.
    pub used: f64,
    pub clients: BTreeMap<String, f64>,
    pub sessions: BTreeMap<String, f64>,
    pub models: BTreeMap<String, f64>,
    /// Rises no Fusebox attempt explains.
    pub outside: f64,
    /// Points split by weighted tokens because a model had no price.
    pub by_tokens: f64,
    /// Attempts finished after this wait for the next rise.
    cursor: DateTime<Utc>,
}

impl Share {
    fn start(w: &Window, now: DateTime<Utc>, before: f64) -> Self {
        Self {
            name: w.name.clone(),
            model: w.model.clone(),
            resets_at: w.resets_at,
            since: now,
            before,
            used: w.used,
            cursor: now,
            ..Default::default()
        }
    }

    /// Points attributed to Fusebox attempts.
    pub fn attributed(&self) -> f64 {
        self.clients.values().sum()
    }

    fn same_window(&self, w: &Window) -> bool {
        self.name == w.name && self.model == w.model
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Attribution {
    #[serde(default)]
    pub history: Vec<crate::outlook::History>,
    #[serde(default)]
    uses: VecDeque<Use>,
    #[serde(default)]
    pub current: Vec<Share>,
    /// The last finished instances, newest first.
    #[serde(default)]
    pub previous: Vec<Share>,
    /// Changed since the state was last written to disk.
    #[serde(skip)]
    pub dirty: bool,
}

/// How long a named window lasts, when the name says.
pub fn length(name: &str) -> Option<Duration> {
    // Model windows ("week opus") are as long as the first word says.
    match name.split_whitespace().next().unwrap_or_default() {
        "week" => Some(Duration::days(7)),
        "day" => Some(Duration::days(1)),
        n => n.strip_suffix('h').and_then(|h| h.parse::<i64>().ok()).map(Duration::hours),
    }
}

/// Whether a model counts toward a window scoped to models containing `scope`.
fn in_scope(scope: Option<&str>, model: Option<&str>) -> bool {
    match scope {
        None => true,
        Some(scope) => model.is_some_and(|m| m.to_ascii_lowercase().contains(&scope.to_ascii_lowercase())),
    }
}

impl Attribution {
    pub fn record(&mut self, u: Use) {
        let horizon = u.at - Duration::days(KEEP_FOR_DAYS);
        self.uses.push_back(u);
        while self.uses.len() > KEEP_USES || self.uses.front().is_some_and(|u| u.at < horizon) {
            self.uses.pop_front();
        }
        self.dirty = true;
    }

    /// A quota reading. Windows the reading leaves out keep their state.
    pub fn reading(&mut self, windows: &[Window], now: DateTime<Utc>) {
        crate::outlook::History::observe(&mut self.history, windows, now);
        for w in windows {
            let Some(i) = self.current.iter().position(|s| s.same_window(w)) else {
                // First sight: what is used already happened before Fusebox looked.
                self.current.push(Share::start(w, now, w.used));
                self.dirty = true;
                continue;
            };
            let s = &self.current[i];
            let same = match (s.resets_at, w.resets_at) {
                (Some(a), Some(b)) => (a - b).num_seconds().abs() < SAME_RESET_SECONDS,
                _ => true,
            };
            let dropped = w.used + NOISE_POINTS < s.used || (w.used < 0.5 && s.used >= 2.0);
            if same && !dropped {
                let rise = w.used - s.used;
                if rise > 1e-9 {
                    self.attribute(i, rise, now);
                }
                // The first reset time stays: Codex reports seconds-until, so later
                // readings drift by a second or two and would split the history.
                let s = &mut self.current[i];
                s.used = s.used.max(w.used);
                if s.resets_at.is_none() {
                    s.resets_at = w.resets_at;
                }
                self.dirty = true;
                continue;
            }
            // A new window: it renewed, or a banked reset cleared it. What it shows
            // already was used since it opened.
            let opened = w.resets_at.zip(length(&w.name)).map(|(r, d)| r - d).filter(|o| *o <= now);
            let mut next = Share::start(w, now, 0.0);
            match opened {
                Some(opened) => next.cursor = opened.max(self.current[i].cursor),
                None => next.before = w.used,
            }
            let finished = std::mem::replace(&mut self.current[i], next);
            self.previous.insert(0, finished);
            self.previous.truncate(KEEP_PREVIOUS);
            if opened.is_some() && w.used > 0.0 {
                self.attribute(i, w.used, now);
            }
            self.dirty = true;
        }
    }

    /// Splits a rise over the attempts that finished since the window's last rise.
    fn attribute(&mut self, i: usize, rise: f64, now: DateTime<Utc>) {
        let (cursor, scope) = (self.current[i].cursor, self.current[i].model.clone());
        let pending: Vec<&Use> = self
            .uses
            .iter()
            .filter(|u| u.at > cursor && u.at <= now && in_scope(scope.as_deref(), u.model.as_deref()))
            .collect();
        let share = &mut self.current[i];
        share.cursor = now;
        if pending.is_empty() {
            share.outside += rise;
            return;
        }
        // One unit for the whole split: prices when every attempt has one.
        let by_cost = pending.iter().all(|u| u.cost.is_some());
        let weight = |u: &Use| if by_cost { u.cost.unwrap_or(0.0) } else { u.tokens };
        let total: f64 = pending.iter().map(|u| weight(u)).sum();
        for u in &pending {
            let part = if total > 0.0 { rise * weight(u) / total } else { rise / pending.len() as f64 };
            *share.clients.entry(u.client.clone()).or_default() += part;
            if let Some(model) = &u.model {
                *share.models.entry(model.clone()).or_default() += part;
            }
            if let Some(session) = &u.session {
                *share.sessions.entry(session.clone()).or_default() += part;
            }
        }
        if !by_cost {
            share.by_tokens += rise;
        }
        while share.sessions.len() > KEEP_SESSIONS {
            let smallest = share.sessions.iter().min_by(|a, b| a.1.total_cmp(b.1)).map(|(k, _)| k.clone());
            match smallest {
                Some(k) => share.sessions.remove(&k),
                None => break,
            };
        }
    }

    /// What one session used of each current window, in points.
    pub fn session(&self, session: &str) -> BTreeMap<String, f64> {
        self.current.iter().filter_map(|s| s.sessions.get(session).map(|p| (window_key(s), *p))).collect()
    }

    /// The state worth keeping across a restart: windows, and only the attempts
    /// still waiting for a reading.
    pub fn for_disk(&self) -> Self {
        let oldest = self.current.iter().map(|s| s.cursor).min();
        let pending = self.uses.iter().filter(|u| oldest.is_none_or(|c| u.at > c));
        let skip = pending.clone().count().saturating_sub(SAVE_USES);
        Self {
            uses: pending.skip(skip).cloned().collect(),
            history: self.history.clone(),
            current: self.current.clone(),
            previous: self.previous.clone(),
            dirty: false,
        }
    }
}

/// A window's name with its model scope, as the dashboard keys it. Claude already
/// names its model windows by model ("week opus").
pub fn window_key(s: &Share) -> String {
    match &s.model {
        Some(m) if !s.name.to_ascii_lowercase().contains(&m.to_ascii_lowercase()) => format!("{} · {m}", s.name),
        _ => s.name.clone(),
    }
}

pub const FILE: &str = ".quota-attribution.state";

/// Every account's attribution, for the state file.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Saved(pub BTreeMap<String, SavedAccount>);

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SavedAccount {
    /// Credential identity, independent of the filename and token refreshes.
    /// Older state files have no identity and are bound on their next save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(flatten)]
    pub attribution: Attribution,
}

impl Saved {
    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        crate::files::write_private(path, &serde_json::to_vec(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minutes: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_790_000_000, 0).unwrap() + Duration::minutes(minutes)
    }

    fn window(name: &str, used: f64, resets: i64) -> Window {
        Window { name: name.into(), used, resets_at: Some(at(resets)), model: None }
    }

    fn used(client: &str, minute: i64, cost: Option<f64>, tokens: f64, model: &str) -> Use {
        Use {
            at: at(minute),
            cost,
            tokens,
            client: client.into(),
            session: Some(format!("{client}-session")),
            model: Some(model.into()),
        }
    }

    #[test]
    fn a_rise_is_split_by_price_over_the_attempts_since_the_last_rise() {
        let mut a = Attribution::default();
        a.reading(&[window("week", 10.0, 5000)], at(0));
        a.record(used("laptop", 1, Some(300.0), 10.0, "claude-opus-5-5"));
        a.record(used("agent", 2, Some(100.0), 90.0, "claude-sonnet-5-5"));
        // Whole-percent readings: no change yet, so both attempts keep waiting.
        a.reading(&[window("week", 10.0, 5000)], at(3));
        a.reading(&[window("week", 12.0, 5000)], at(4));
        let s = &a.current[0];
        assert_eq!(s.before, 10.0);
        assert!((s.clients["laptop"] - 1.5).abs() < 1e-9 && (s.clients["agent"] - 0.5).abs() < 1e-9);
        assert!((s.sessions["laptop-session"] - 1.5).abs() < 1e-9);
        assert_eq!(s.by_tokens, 0.0);
        // A rise with nothing to explain it happened outside Fusebox.
        a.reading(&[window("week", 13.0, 5000)], at(10));
        assert!((a.current[0].outside - 1.0).abs() < 1e-9);
    }

    #[test]
    fn model_windows_are_keyed_once_and_requests_by_their_key() {
        let share = |name: &str, model: Option<&str>| Share {
            name: name.into(),
            model: model.map(String::from),
            ..Default::default()
        };
        assert_eq!(window_key(&share("week opus", Some("opus"))), "week opus");
        assert_eq!(window_key(&share("week", Some("opus"))), "week · opus");
        let mut o = Observation::new("proxy", "x".into(), "openai", 0);
        o.tokens.output = Some(10);
        o.client_id = Some("0123456789abcdef".into());
        assert_eq!(Use::from_observation(&o, None, Some("Codex")).unwrap().client, "0123456789abcdef");
        o.client_id = None;
        assert_eq!(Use::from_observation(&o, None, Some("Codex")).unwrap().client, "app:Codex");
        assert_eq!(Use::from_observation(&o, None, None).unwrap().client, "");
    }

    #[test]
    fn unpriced_models_split_by_tokens_and_scoped_windows_count_their_models() {
        let mut a = Attribution::default();
        let opus = Window { name: "week".into(), used: 0.0, resets_at: Some(at(5000)), model: Some("opus".into()) };
        a.reading(&[window("5h", 0.0, 300), opus.clone()], at(0));
        a.record(used("laptop", 1, Some(500.0), 100.0, "claude-opus-5-5"));
        a.record(used("agent", 2, None, 300.0, "claude-sonnet-5-5"));
        a.reading(&[window("5h", 4.0, 300), Window { used: 2.0, ..opus }], at(3));
        let five = &a.current[0];
        assert!((five.clients["laptop"] - 1.0).abs() < 1e-9, "{five:?}");
        assert!((five.clients["agent"] - 3.0).abs() < 1e-9);
        assert_eq!(five.by_tokens, 4.0);
        let weekly_opus = &a.current[1];
        assert_eq!(weekly_opus.clients.len(), 1, "only Opus counts toward the Opus window");
        assert!((weekly_opus.clients["laptop"] - 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_renewed_window_starts_over_and_keeps_the_finished_one() {
        let mut a = Attribution::default();
        a.reading(&[window("5h", 40.0, 300)], at(0));
        a.record(used("laptop", 10, Some(1.0), 1.0, "m"));
        a.reading(&[window("5h", 50.0, 300)], at(20));
        // Renewed at minute 300; a request after that shows in the new window's first reading.
        a.record(used("agent", 310, Some(1.0), 1.0, "m"));
        a.reading(&[window("5h", 3.0, 600)], at(320));
        assert_eq!(a.previous.len(), 1);
        assert_eq!(a.previous[0].clients["laptop"], 10.0);
        let s = &a.current[0];
        assert_eq!((s.before, s.used), (0.0, 3.0));
        assert_eq!(s.clients.get("agent"), Some(&3.0));
        // A Codex-style reset time that moves by seconds is the same window.
        a.record(used("agent", 330, Some(1.0), 1.0, "m"));
        let mut jitter = window("5h", 4.0, 600);
        jitter.resets_at = jitter.resets_at.map(|t| t + Duration::seconds(3));
        a.reading(&[jitter], at(340));
        assert_eq!(a.previous.len(), 1);
        assert_eq!(a.current[0].clients.get("agent"), Some(&4.0));
        // A drop without a new reset time (a banked reset) also starts over.
        a.reading(&[window("5h", 0.0, 600)], at(350));
        assert_eq!(a.previous.len(), 2);
        assert_eq!(a.current[0].used, 0.0);
    }

    #[test]
    fn readings_from_two_sources_that_disagree_by_a_point_are_one_window() {
        // As production saw it: the usage endpoint says 87 with a fractional reset
        // time, a response header seconds later says 86 with a whole-second one.
        let mut a = Attribution::default();
        let mut endpoint = window("5h", 87.0, 300);
        endpoint.resets_at = endpoint.resets_at.map(|t| t - Duration::milliseconds(392));
        a.reading(&[endpoint], at(0));
        a.reading(&[window("5h", 86.0, 300)], at(1));
        a.record(used("laptop", 2, Some(1.0), 1.0, "m"));
        a.reading(&[window("5h", 88.0, 300)], at(3));
        assert!(a.previous.is_empty(), "{:?}", a.previous);
        let s = &a.current[0];
        assert_eq!((s.before, s.used, s.outside), (87.0, 88.0, 0.0));
        assert_eq!(s.clients.get("laptop"), Some(&1.0));
        // A real drop is still a reset.
        a.reading(&[window("5h", 70.0, 300)], at(4));
        assert_eq!(a.previous.len(), 1);
    }

    #[test]
    fn the_saved_state_keeps_windows_and_only_waiting_attempts() {
        let mut a = Attribution::default();
        a.reading(&[window("week", 5.0, 5000)], at(0));
        a.record(used("laptop", 1, Some(1.0), 1.0, "m"));
        a.reading(&[window("week", 6.0, 5000)], at(2));
        a.record(used("agent", 3, Some(1.0), 1.0, "m"));
        let saved = a.for_disk();
        assert_eq!(saved.uses.len(), 1, "only the attempt after the last rise");
        assert_eq!(saved.current, a.current);
        assert_eq!(saved.history, a.history);
        let saved = SavedAccount { identity: Some("identity".into()), attribution: saved };
        let text = serde_json::to_string(&Saved(BTreeMap::from([("acct".to_string(), saved)]))).unwrap();
        let back: Saved = serde_json::from_str(&text).unwrap();
        assert_eq!(back.0["acct"].identity.as_deref(), Some("identity"));
        let mut restored = back.0["acct"].attribution.clone();
        assert_eq!(restored.history, a.history);
        restored.reading(&[window("week", 7.0, 5000)], at(4));
        assert_eq!(restored.current[0].clients.get("agent"), Some(&1.0));
        let mut legacy: serde_json::Value = serde_json::from_str(&text).unwrap();
        legacy["acct"].as_object_mut().unwrap().remove("history");
        let legacy: Saved = serde_json::from_value(legacy).unwrap();
        assert!(legacy.0["acct"].attribution.history.is_empty());
    }
}
