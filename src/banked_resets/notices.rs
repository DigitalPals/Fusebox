//! What the owner hears about banked resets: one used automatically, one whose outcome
//! needs review, and one that expired unused. Each is sent once, when it happens.
use super::*;

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
