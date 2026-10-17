//! Plan against expiring grants; spending always rechecks authoritative usage under the journal lock.
use super::*;
use crate::config::{Config, Routing};
use crate::quota::{Quota, Window};
use provider::Grant;

const WEEKLY_USED: f64 = 95.0;
const EXPIRY_USED: f64 = 20.0;
const EXPIRY_SAFETY_HOURS: i64 = 6;
const INVENTORY_MAX_AGE_MINUTES: i64 = 35;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Policy {
    pub expiry_priority_hours: u32,
    pub auto_redeem: bool,
}
impl Default for Policy {
    fn default() -> Self {
        Self { expiry_priority_hours: 7 * 24, auto_redeem: false }
    }
}
impl Policy {
    pub fn validate(&self) -> Result<(), String> {
        if self.expiry_priority_hours > 8760 {
            return Err("Reset expiry preference must be between 0 and 8760 hours".into());
        }
        if self.auto_redeem && self.expiry_priority_hours == 0 {
            return Err("Automatic resets need a nonzero reset expiry preference".into());
        }
        Ok(())
    }
}

pub(super) fn enabled(cfg: &Config) -> bool {
    cfg.banked_resets
        && cfg.routing == Routing::SmartQuota
        && cfg.banked_reset_policy.auto_redeem
        && cfg.banked_reset_policy.expiry_priority_hours > 0
}

fn valid(grant: &Grant, now: DateTime<Utc>) -> bool {
    grant.remaining > 0
        && grant.expires_at.is_some_and(|e| e > now)
        && grant.starts_at.is_none_or(|s| s <= now)
        && (grant.usable || grant.reason.as_deref() == Some("Account must reach its limit first"))
}

fn within_horizon(cfg: &Config, grant: &Grant, now: DateTime<Utc>) -> bool {
    valid(grant, now)
        && grant.expires_at.is_some_and(|e| {
            e <= now + chrono::Duration::hours(i64::from(cfg.banked_reset_policy.expiry_priority_hours))
        })
}

fn clears(grant: &Grant, window: &Window) -> bool {
    grant.clears.contains(&window.name)
        || (window.model.is_none() && grant.clears.iter().any(|s| s == "subscription limits"))
}

fn inventory(view: &View, now: DateTime<Utc>) -> Option<&Inventory> {
    if view.error.is_some()
        || view.checked_at > now + chrono::Duration::minutes(1)
        || view.checked_at < now - chrono::Duration::minutes(INVENTORY_MAX_AGE_MINUTES)
        || view.operation.as_ref().is_some_and(|o| matches!(o.status.as_str(), "pending" | "unknown"))
    {
        return None;
    }
    view.inventory.as_ref().filter(|i| i.available.is_some_and(|n| n > 0))
}

/// Separate from weekly renewal: a reset needs time to consume allowance before it can restore it.
pub fn priority_expiry(acct: &Account, model: &str, cfg: &Config) -> Option<DateTime<Utc>> {
    if !cfg.banked_resets
        || cfg.routing != Routing::SmartQuota
        || cfg.banked_reset_policy.expiry_priority_hours == 0
        || account_id(acct).is_err()
    {
        return None;
    }
    let now = Utc::now();
    let st = acct.state.lock();
    let inv = inventory(st.banked_resets.as_ref()?, now)?;
    inv.grants
        .iter()
        .filter(|g| within_horizon(cfg, g, now))
        .filter(|g| {
            st.quota
                .windows
                .iter()
                .any(|w| clears(g, w) && w.model.as_ref().is_none_or(|m| model.to_ascii_lowercase().contains(m)))
        })
        .filter_map(|g| g.expires_at)
        .min()
}

pub(super) struct Decision {
    pub grant_id: String,
    pub credit_key: String,
}

pub(super) fn decision(cfg: &Config, quota: &Quota, inv: &Inventory, now: DateTime<Utc>) -> Option<Decision> {
    if !enabled(cfg) || !inv.eligible || inv.available.is_none_or(|n| n == 0) {
        return None;
    }
    inv.grants
        .iter()
        .filter(|g| g.usable && within_horizon(cfg, g, now))
        .filter(|g| {
            let expiry = g.expires_at.unwrap();
            if expiry <= now + chrono::Duration::seconds(30) {
                return false;
            }
            let affected: Vec<_> = quota
                .windows
                .iter()
                .filter(|w| clears(g, w) && w.used.is_finite() && w.resets_at.is_none_or(|r| r > now))
                .collect();
            let weekly = g.clears.iter().any(|s| s.starts_with("week") || s == "subscription limits");
            let drained = affected.iter().any(|w| {
                (!weekly || w.name.starts_with("week"))
                    && w.used >= WEEKLY_USED
                    && w.resets_at.is_none_or(|r| r > now + chrono::Duration::minutes(10))
            });
            let expiring = expiry <= now + chrono::Duration::hours(EXPIRY_SAFETY_HOURS)
                && affected.iter().any(|w| w.used >= EXPIRY_USED && w.resets_at.is_none_or(|r| r > expiry));
            drained || expiring
        })
        .min_by_key(|g| (g.expires_at, &g.id))
        .map(|g| Decision {
            grant_id: g.id.clone(),
            // Another grant arriving or expiring must not release this marker.
            // Only this grant's identity, expiry and remaining count identify
            // the next credit. Anonymous Codex credits conservatively share it.
            credit_key: format!("{}:{}:{}", g.id, g.expires_at.unwrap().timestamp(), g.remaining),
        })
}

pub(super) fn check_journal(journal: &ledger::Journal, decision: &Decision, now: DateTime<Utc>) -> Result<()> {
    ensure!(journal.unresolved().is_none(), "A previous reset needs review");
    for operation in journal.operations.values() {
        let success = matches!(operation.status.as_str(), "applied" | "already_used" | "reconciled_used");
        ensure!(
            !(success && now - operation.updated_at < chrono::Duration::minutes(10)),
            "A reset was recently applied"
        );
        if operation.auto_credit.as_deref() == Some(&decision.credit_key) {
            ensure!(!success, "Provider inventory has not confirmed the previous automatic spend");
            ensure!(
                now - operation.updated_at >= chrono::Duration::hours(1),
                "Automatic reset is backing off after refusal"
            );
        }
    }
    Ok(())
}

/// Called by the bounded quota poller, never from a user's inference request.
pub async fn automatic_check(app: &Arc<App>, acct: &Arc<Account>) -> bool {
    let cfg = app.cfg();
    if !enabled(&cfg) || account_id(acct).is_err() {
        return false;
    }
    {
        let st = acct.state.lock();
        if st.disabled || st.quota_refreshing {
            return false;
        }
        let Some(view) = st.banked_resets.as_ref() else { return false };
        let Some(inv) = inventory(view, Utc::now()) else { return false };
        if let Some(o) = &view.operation {
            let delay = if o.automatic && o.status == "refused" { 3600 } else { 600 };
            if (Utc::now() - o.created_at).num_seconds() < delay {
                return false;
            }
        }
        // A grant waiting for a limit may become usable on the authoritative preflight.
        let mut planned = inv.clone();
        for g in &mut planned.grants {
            if g.reason.as_deref() == Some("Account must reach its limit first") {
                g.usable = true;
                planned.eligible = true;
            }
        }
        if decision(&cfg, &st.quota, &planned, Utc::now()).is_none() {
            return false;
        }
    }
    let result = async {
        let api = api(app, acct).await?;
        let organization = api.identity().await?;
        let mut ledger = Ledger::open(&app.startup_config.auth_dir(), &scope(acct.provider, &organization))?;
        execute(app, acct, &api, &organization, &api.account_id, &mut ledger, Request::Automatic).await
    }
    .await;
    match result {
        Ok(_) => true,
        Err(e) => {
            tracing::debug!(account = %acct.label, "automatic reset deferred: {e}");
            false
        }
    }
}

#[derive(Serialize)]
pub struct Plan {
    pub expires_at: DateTime<Utc>,
    pub prioritized: bool,
    pub auto_redeem: bool,
    pub due: bool,
    pub message: &'static str,
}

pub fn plan(acct: &Account, cfg: &Config) -> Option<Plan> {
    if !cfg.banked_resets || account_id(acct).is_err() {
        return None;
    }
    let now = Utc::now();
    let prioritized = priority_expiry(acct, "", cfg).is_some();
    let st = acct.state.lock();
    let inv = inventory(st.banked_resets.as_ref()?, now)?;
    let expires_at = inv.grants.iter().filter(|g| valid(g, now)).filter_map(|g| g.expires_at).min()?;
    let auto = enabled(cfg) && !st.disabled;
    Some(Plan {
        expires_at,
        prioritized: prioritized && !st.disabled,
        auto_redeem: auto,
        due: auto && decision(cfg, &st.quota, inv, now).is_some(),
        message: if auto {
            "Automatic reset at 95% weekly use, or in the final 6 hours with at least 20% quota to restore. Short limits normally wait for their own renewal."
        } else {
            "Automatic redemption is off. Use a reset manually before it expires."
        },
    })
}
