//! What the owner hears about banked resets: newly received, used automatically,
//! needing review, or expired unused.
use super::*;
use std::collections::BTreeMap;

/// Receipt history lives under the spending journal's identity lock. Keep allocation
/// high-water marks even when grants disappear: partial reads and stale balances must
/// not turn an old grant into a new one. Unknown identities are credits already counted
/// in a count-only/partial response, so later detail rows don't announce them again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Observation {
    checked_at: Option<DateTime<Utc>>,
    available: Option<u64>,
    grants: BTreeMap<String, u64>,
    unidentified: u64,
}

#[derive(Debug, Default)]
pub(super) struct Received {
    pub count: u64,
    pub expires_at: Option<DateTime<Utc>>,
}

impl Observation {
    pub(super) fn update(&mut self, inv: &Inventory, now: DateTime<Utc>) -> Received {
        if self.checked_at.is_some_and(|checked| now < checked) {
            return Received::default();
        }
        let held = |g: &&provider::Grant| g.remaining > 0 && g.expires_at.is_none_or(|expiry| expiry > now);
        let available = inv.available.unwrap_or_else(|| inv.grants.iter().filter(held).map(|g| g.remaining).sum());
        let identified: u64 = inv.grants.iter().filter(held).filter(|g| !g.id.is_empty()).map(|g| g.remaining).sum();
        let complete = identified == available;
        let initial = self.checked_at.is_none();
        let had_unidentified = self.unidentified > 0;
        self.unidentified = self.unidentified.min(available);
        let mut received = Received::default();
        for grant in inv.grants.iter().filter(|g| !g.id.is_empty()) {
            let previous = self.grants.get(&grant.id).copied();
            let total = grant.total.max(grant.remaining);
            self.grants.insert(grant.id.clone(), previous.unwrap_or(0).max(total));
            if grant.remaining == 0 || grant.expires_at.is_some_and(|expiry| expiry <= now) {
                continue;
            }
            let mut added = previous.map_or(grant.remaining, |old| total.saturating_sub(old).min(grant.remaining));
            let freshly_granted =
                self.checked_at.zip(grant.granted_at).is_some_and(|(checked, granted)| granted > checked);
            if previous.is_none() && !freshly_granted {
                let accounted = added.min(self.unidentified);
                self.unidentified -= accounted;
                added -= accounted;
            }
            received.count += added;
            if added > 0
                && let Some(expiry) = grant.expires_at
            {
                received.expires_at = Some(received.expires_at.map_or(expiry, |old| old.min(expiry)));
            }
        }
        // With complete, previously identified inventory, a balance rebound without
        // any new allocation is not a receipt. Count-only providers can only report
        // a net gain; remember that gain until its credit identities become known.
        if !complete || had_unidentified {
            let gain = available.saturating_sub(self.available.unwrap_or(available));
            let unassigned = gain.saturating_sub(received.count);
            received.count += unassigned;
            self.unidentified = self.unidentified.saturating_add(unassigned).min(available);
        }
        if initial {
            self.unidentified = available.saturating_sub(identified);
            received = Received::default();
        } else if complete {
            self.unidentified = 0;
        }
        self.checked_at = Some(now);
        self.available = Some(available);
        received
    }
}

pub(super) fn received(
    app: &Arc<App>,
    acct: &Account,
    account: &str,
    ledger: &mut Ledger,
    inv: &Inventory,
    now: DateTime<Utc>,
) {
    if !app.cfg().banked_resets
        || account_id(acct).ok().as_deref() != Some(account)
        || (inv.available.is_none() && inv.grants.is_empty())
    {
        return;
    }
    let previous = ledger.journal.observations.get(account).cloned();
    let mut next = previous.clone().unwrap_or_default();
    let receipt = next.update(inv, now);
    if previous.as_ref() == Some(&next) {
        return;
    }
    if next.grants.len() > 10_000 {
        tracing::warn!(account = %acct.label, "Reset receipt history is full; notification skipped");
        return;
    }
    ledger.journal.observations.insert(account.into(), next);
    if let Err(e) = ledger.save() {
        if let Some(previous) = previous {
            ledger.journal.observations.insert(account.into(), previous);
        } else {
            ledger.journal.observations.remove(account);
        }
        tracing::warn!(account = %acct.label, "Cannot save reset receipt history; notification skipped: {e:#}");
        return;
    }
    if receipt.count == 0 {
        return;
    }
    let name = provider_name(acct.provider);
    let (title, what) = if receipt.count == 1 {
        (format!("New {name} banked reset"), "1 new banked reset".into())
    } else {
        (format!("{} new {name} banked resets", receipt.count), format!("{} new banked resets", receipt.count))
    };
    let total = inv.available.map_or(String::new(), |n| format!(" {n} banked in total."));
    let expiry = receipt.expires_at.map_or(String::new(), |expiry| {
        format!(" Earliest reported expiry for the new resets: {} UTC.", expiry.format("%d %b %Y %H:%M"))
    });
    crate::push::banked_reset(
        app,
        acct,
        format!("reset-received:{}:{}", acct.id, uuid::Uuid::new_v4()),
        "ok",
        title,
        format!("{}: {what}.{total}{expiry}", acct.label),
    );
}

/// One watched reset: grant ID, expiry and how many it holds.
type Watched = (String, DateTime<Utc>, u64);

pub fn provider_name(provider: Provider) -> &'static str {
    if provider == Provider::Claude { "Claude" } else { "Codex" }
}

/// The unexpired resets in a good inventory read.
pub(super) fn watch_list(inv: &Inventory, now: DateTime<Utc>) -> Vec<Watched> {
    inv.grants
        .iter()
        .filter(|g| g.remaining > 0)
        .filter_map(|g| Some((g.id.clone(), g.expires_at.filter(|e| *e > now)?, g.remaining)))
        .collect()
}

/// Resets watched at the last good read that have expired since without being used up.
/// One that is gone from the list counts with what it held then: spends made here are
/// forgotten as they happen (`forget_spent`).
pub(super) fn expired_unused(watch: &[Watched], inv: &Inventory, now: DateTime<Utc>) -> Vec<Watched> {
    watch
        .iter()
        .filter(|(_, expires, _)| *expires <= now)
        .filter_map(|(id, expires, held)| {
            match inv.grants.iter().find(|g| g.id == *id && g.expires_at == Some(*expires)) {
                Some(g) if g.remaining == 0 => None,
                Some(g) => Some((id.clone(), *expires, g.remaining)),
                None => Some((id.clone(), *expires, *held)),
            }
        })
        .collect()
}

/// A reset in this view expired after the view was read: read again so the expiry is
/// noticed within a minute, not at the next half-hourly refresh.
pub fn expiry_passed(view: &View, now: DateTime<Utc>) -> bool {
    view.inventory.as_ref().is_some_and(|inv| {
        inv.grants.iter().any(|g| g.remaining > 0 && g.expires_at.is_some_and(|e| view.checked_at < e && e <= now))
    })
}

pub(super) fn expired(app: &Arc<App>, acct: &Account, view: &View, gone: &[Watched]) {
    let name = provider_name(acct.provider);
    let cfg = app.cfg();
    // Why it wasn't used, when the latest operation says.
    let why = match &view.operation {
        Some(o)
            if o.automatic && o.status == "refused" && view.checked_at - o.created_at < chrono::Duration::hours(2) =>
        {
            format!(" The last automatic try was refused: {}.", o.message.trim_end_matches('.'))
        }
        _ if !automation::enabled(&cfg) => {
            " Turn on Automatically use expiring resets under Config, Routing to use them before they expire.".into()
        }
        _ => String::new(),
    };
    for (id, expires, count) in gone {
        let (title, what) = if *count == 1 {
            (format!("{name} reset expired unused"), "A banked reset".to_string())
        } else {
            (format!("{count} {name} resets expired unused"), format!("{count} banked resets"))
        };
        crate::push::banked_reset(
            app,
            acct,
            format!("reset-expired:{}:{id}:{}", acct.id, expires.timestamp()),
            "warn",
            title,
            format!("{}: {what} expired without being used.{why}", acct.label),
        );
    }
}

/// An automatic spend's settled outcome. Manual ones happen in front of the user.
pub(super) fn outcome(app: &Arc<App>, acct: &Account, operation: &Operation, expiry_due: bool) {
    let name = provider_name(acct.provider);
    let label = &acct.label;
    let (level, title, body) = match operation.status.as_str() {
        "applied" => (
            "ok",
            format!("{name} reset used"),
            if expiry_due {
                format!(
                    "{label}: a banked reset was about to expire, so Fusebox used it. The limits it clears start over."
                )
            } else {
                format!(
                    "{label}: a limit was 95% used, so Fusebox used a banked reset. The limits it clears start over."
                )
            },
        ),
        "unknown" => (
            "warn",
            format!("{name} reset needs review"),
            format!(
                "{label}: an automatic reset got no clear answer, so one may have been used. Fusebox won't use another until you check the outcome in Reset details."
            ),
        ),
        _ => return,
    };
    crate::push::banked_reset(app, acct, format!("reset:{}", operation.request_id), level, title, body);
}

/// A reset spent (or reported spent) here is not watched for expiry any more. Without
/// a grant ID (Codex chose), the one expiring first is assumed.
pub(super) fn forget_spent(app: &App, acct: &Account, account: &str, grant: &str) {
    for other in app.pool.all() {
        if other.provider != acct.provider || account_id(&other).ok().as_deref() != Some(account) {
            continue;
        }
        let mut st = other.state.lock();
        let watch = &mut st.reset_watch;
        let at = if grant.is_empty() {
            watch.iter().enumerate().min_by_key(|(_, w)| w.1).map(|(i, _)| i)
        } else {
            watch.iter().position(|w| w.0 == grant)
        };
        if let Some(i) = at {
            watch[i].2 -= 1;
            if watch[i].2 == 0 {
                watch.remove(i);
            }
        }
    }
}
