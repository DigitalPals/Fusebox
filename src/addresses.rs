//! Where client keys are used from, and what is connected right now.
//!
//! Each named client's addresses are kept in `<auth-dir>/.client-addresses.state` (0600):
//! when each was first and last seen, how many requests came from it, and how. A named key
//! turning up somewhere new can send a notification (`notifications.client-new-address`).
//! What is connected now, requests in flight and open WebSockets for every key, is kept
//! in memory only.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::source::{Source, Via};

pub const FILE: &str = ".client-addresses.state";
/// Addresses kept per client; the least recently seen goes first.
const KEEP: usize = 32;
/// Addresses not seen for this long are forgotten.
const FORGET_AFTER_DAYS: i64 = 180;
/// Saved at most this often while requests keep coming.
const SAVE_EVERY: Duration = Duration::from_secs(30);
/// How long a key and address stay on the connected list after their last request.
const RECENT: chrono::TimeDelta = chrono::TimeDelta::minutes(5);
/// Notifications about addresses in an hour, at most, whatever happens.
const ALERTS_PER_HOUR: usize = 10;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    pub first: DateTime<Utc>,
    pub last: DateTime<Utc>,
    pub requests: u64,
    pub via: Via,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Book {
    /// Client id, then address.
    #[serde(default)]
    clients: BTreeMap<String, BTreeMap<String, Seen>>,
}

/// What a request's address meant for its client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Noted {
    Known,
    /// The client's first address ever: nothing to compare it with.
    First,
    /// An address the client wasn't used from before.
    New,
}

#[derive(Clone, Debug)]
struct Live {
    via: Via,
    login: Option<String>,
    in_flight: u32,
    sockets: u32,
    last: DateTime<Utc>,
}

/// A key and address on the connected list, as `/api/clients/usage` shows it.
#[derive(Clone, Debug, Serialize)]
pub struct Connected {
    pub key: String,
    pub ip: IpAddr,
    pub via: Via,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
    pub in_flight: u32,
    pub sockets: u32,
    pub last: DateTime<Utc>,
}

/// Notifications sent in the last hour, and what each was about: a client, an address,
/// and whether it was a refusal.
type Alerts = (VecDeque<Instant>, HashSet<(String, IpAddr, bool)>);

pub struct Addresses {
    path: PathBuf,
    book: Mutex<Book>,
    dirty: AtomicBool,
    saved: Mutex<Option<Instant>>,
    live: Mutex<HashMap<(String, IpAddr), Live>>,
    alerts: Mutex<Alerts>,
}

impl Addresses {
    pub fn load(auth_dir: &Path) -> Self {
        let path = auth_dir.join(FILE);
        let book = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!("{} is unreadable ({e}); client addresses start over", path.display());
                Book::default()
            }),
            Err(_) => Book::default(),
        };
        Self {
            path,
            book: Mutex::new(book),
            dirty: AtomicBool::new(false),
            saved: Mutex::new(None),
            live: Mutex::default(),
            alerts: Mutex::default(),
        }
    }

    /// Records an admitted request of a named client.
    pub fn note(&self, client: &str, source: &Source) -> Noted {
        let now = Utc::now();
        let mut book = self.book.lock();
        let known = book.clients.entry(client.to_string()).or_default();
        let had = !known.is_empty();
        let noted = match known.get_mut(&source.ip.to_string()) {
            Some(seen) => {
                seen.last = now;
                seen.requests = seen.requests.saturating_add(1);
                seen.via = source.via;
                if source.login.is_some() {
                    seen.login.clone_from(&source.login);
                }
                Noted::Known
            }
            None => {
                let seen = Seen { first: now, last: now, requests: 1, via: source.via, login: source.login.clone() };
                known.insert(source.ip.to_string(), seen);
                while known.len() > KEEP {
                    let Some(oldest) = known.iter().min_by_key(|(_, s)| s.last).map(|(ip, _)| ip.clone()) else {
                        break;
                    };
                    known.remove(&oldest);
                }
                if had { Noted::New } else { Noted::First }
            }
        };
        self.dirty.store(true, Ordering::Relaxed);
        noted
    }

    /// A client's addresses, most recently seen first.
    pub fn history(&self, client: &str) -> Vec<(String, Seen)> {
        let book = self.book.lock();
        let mut list: Vec<_> =
            book.clients.get(client).into_iter().flatten().map(|(ip, s)| (ip.clone(), s.clone())).collect();
        list.sort_by(|a, b| b.1.last.cmp(&a.1.last).then_with(|| a.0.cmp(&b.0)));
        list
    }

    /// Writes the book when it changed: at most every 30 seconds, or now with `force`.
    /// Clients no longer configured and addresses not seen for half a year are dropped.
    pub fn save(&self, clients: &[crate::config::NamedClient], force: bool) {
        if !self.dirty.load(Ordering::Relaxed) {
            return;
        }
        {
            let mut saved = self.saved.lock();
            if !force && saved.is_some_and(|at| at.elapsed() < SAVE_EVERY) {
                return;
            }
            *saved = Some(Instant::now());
        }
        self.dirty.store(false, Ordering::Relaxed);
        let bytes = {
            let mut book = self.book.lock();
            let cutoff = Utc::now() - chrono::TimeDelta::days(FORGET_AFTER_DAYS);
            let ids: HashSet<&str> = clients.iter().map(|c| c.id.as_str()).collect();
            book.clients.retain(|id, known| {
                known.retain(|_, s| s.last > cutoff);
                ids.contains(id.as_str()) && !known.is_empty()
            });
            serde_json::to_vec(&*book)
        };
        let result = bytes.map_err(anyhow::Error::from).and_then(|b| crate::files::write_private(&self.path, &b));
        if let Err(e) = result {
            self.dirty.store(true, Ordering::Relaxed);
            tracing::warn!("could not save {}: {e:#}", self.path.display());
        }
    }

    /// Counts a request (or, with `socket`, an open WebSocket) until the guard is dropped.
    pub fn begin(self: &Arc<Self>, key: &str, source: &Source, socket: bool) -> Guard {
        let slot = (key.to_string(), source.ip);
        {
            let mut live = self.live.lock();
            let entry = live.entry(slot.clone()).or_insert_with(|| Live {
                via: source.via,
                login: None,
                in_flight: 0,
                sockets: 0,
                last: Utc::now(),
            });
            entry.via = source.via;
            if source.login.is_some() {
                entry.login.clone_from(&source.login);
            }
            entry.last = Utc::now();
            if socket {
                entry.sockets += 1;
            } else {
                entry.in_flight += 1;
            }
        }
        Guard { book: self.clone(), slot, socket }
    }

    /// Keys and addresses with requests in flight or open sockets, or seen in the last
    /// five minutes; busy ones first.
    pub fn connected(&self) -> Vec<Connected> {
        let now = Utc::now();
        let mut live = self.live.lock();
        live.retain(|_, l| l.in_flight > 0 || l.sockets > 0 || now - l.last < RECENT);
        let mut list: Vec<Connected> = live
            .iter()
            .map(|((key, ip), l)| Connected {
                key: key.clone(),
                ip: *ip,
                via: l.via,
                login: l.login.clone(),
                in_flight: l.in_flight,
                sockets: l.sockets,
                last: l.last,
            })
            .collect();
        list.sort_by(|a, b| {
            let busy = |c: &Connected| c.in_flight > 0 || c.sockets > 0;
            busy(b).cmp(&busy(a)).then(b.last.cmp(&a.last)).then_with(|| a.key.cmp(&b.key))
        });
        list
    }

    /// Whether to notify about a client and address: once each while Fusebox runs, and
    /// ten in an hour at most.
    pub fn may_alert(&self, client: &str, ip: IpAddr, refused: bool) -> bool {
        let mut alerts = self.alerts.lock();
        let (sent, about) = &mut *alerts;
        while sent.front().is_some_and(|at| at.elapsed() > Duration::from_secs(3600)) {
            sent.pop_front();
        }
        if sent.len() >= ALERTS_PER_HOUR || !about.insert((client.to_string(), ip, refused)) {
            return false;
        }
        sent.push_back(Instant::now());
        true
    }
}

/// Keeps a request or socket on the connected list.
pub struct Guard {
    book: Arc<Addresses>,
    slot: (String, IpAddr),
    socket: bool,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let mut live = self.book.live.lock();
        if let Some(l) = live.get_mut(&self.slot) {
            if self.socket {
                l.sockets = l.sockets.saturating_sub(1);
            } else {
                l.in_flight = l.in_flight.saturating_sub(1);
            }
            l.last = Utc::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NamedClient;

    fn source(ip: &str, via: Via) -> Source {
        Source { ip: ip.parse().unwrap(), via, login: None }
    }

    fn dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fusebox-addresses-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn client(id: &str) -> NamedClient {
        NamedClient { id: id.into(), label: id.into(), key: format!("fbx_{id}"), ..Default::default() }
    }

    #[test]
    fn the_first_address_is_quiet_and_later_ones_are_new() {
        let dir = dir();
        let book = Addresses::load(&dir);
        assert_eq!(book.note("beast", &source("10.10.0.7", Via::ProxyProtocol)), Noted::First);
        assert_eq!(book.note("beast", &source("10.10.0.7", Via::ProxyProtocol)), Noted::Known);
        assert_eq!(book.note("beast", &source("100.64.0.3", Via::Forwarded)), Noted::New);
        assert_eq!(book.note("dragon", &source("10.10.0.7", Via::ProxyProtocol)), Noted::First, "per client");
        let history = book.history("beast");
        assert_eq!(history.iter().map(|(ip, _)| ip.as_str()).collect::<Vec<_>>(), ["100.64.0.3", "10.10.0.7"]);
        assert_eq!(history[1].1.requests, 2);
        assert_eq!(history[1].1.via, Via::ProxyProtocol);
        assert!(book.history("nobody").is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_book_survives_a_restart_without_configured_clients_or_stale_addresses() {
        let dir = dir();
        let book = Addresses::load(&dir);
        book.note("beast", &source("10.10.0.7", Via::Direct));
        book.note("gone", &source("10.10.0.8", Via::Direct));
        book.note("beast", &source("10.10.0.9", Via::Direct));
        book.book.lock().clients.get_mut("beast").unwrap().get_mut("10.10.0.9").unwrap().last =
            Utc::now() - chrono::TimeDelta::days(FORGET_AFTER_DAYS + 1);
        book.save(&[client("beast")], true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.join(FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let again = Addresses::load(&dir);
        assert_eq!(again.history("beast").len(), 1);
        assert!(again.history("gone").is_empty(), "removed clients are forgotten");
        assert_eq!(again.note("beast", &source("10.10.0.7", Via::Direct)), Noted::Known);
        // Nothing new, nothing written.
        std::fs::remove_file(dir.join(FILE)).unwrap();
        let quiet = Addresses::load(&dir);
        quiet.save(&[client("beast")], true);
        assert!(!dir.join(FILE).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_client_keeps_its_most_recent_addresses() {
        let dir = dir();
        let book = Addresses::load(&dir);
        for i in 0..(KEEP + 5) {
            book.note("busy", &source(&format!("10.0.0.{i}"), Via::Direct));
        }
        let history = book.history("busy");
        assert_eq!(history.len(), KEEP);
        assert!(history.iter().all(|(ip, _)| ip != "10.0.0.0"), "the oldest went first");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn connected_counts_requests_and_sockets_until_they_end() {
        let dir = dir();
        let book = Arc::new(Addresses::load(&dir));
        let a = source("10.10.0.7", Via::ProxyProtocol);
        let request = book.begin("beast", &a, false);
        let socket = book.begin("beast", &a, true);
        let other = book.begin("", &source("127.0.0.1", Via::Direct), false);
        let now = book.connected();
        assert_eq!(now.len(), 2);
        let beast = now.iter().find(|c| c.key == "beast").unwrap();
        assert_eq!((beast.in_flight, beast.sockets), (1, 1));
        drop((request, socket, other));
        let now = book.connected();
        assert!(now.iter().all(|c| c.in_flight == 0 && c.sockets == 0), "still listed as recent");
        assert_eq!(now.len(), 2);
        book.live.lock().values_mut().for_each(|l| l.last = Utc::now() - RECENT - chrono::TimeDelta::seconds(1));
        assert!(book.connected().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn alerts_come_once_each_and_ten_an_hour_at_most() {
        let dir = dir();
        let book = Addresses::load(&dir);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(book.may_alert("beast", ip, false));
        assert!(!book.may_alert("beast", ip, false));
        assert!(book.may_alert("beast", ip, true), "a refusal is its own news");
        for i in 2..=20 {
            book.may_alert("beast", format!("10.0.0.{i}").parse().unwrap(), false);
        }
        assert_eq!(book.alerts.lock().0.len(), ALERTS_PER_HOUR);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
