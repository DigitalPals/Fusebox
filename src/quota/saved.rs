//! What a restart must not forget about quota: which accounts the provider confirmed
//! to have no 5-hour limit, and Meta's last usage reading. Only a usage-endpoint
//! reading can confirm the first, and that endpoint may be rate limited for a while
//! after a restart; meanwhile the dashboard would show a 5-hour meter the account
//! doesn't have. Meta has no usage endpoint at all: its windows arrive with its
//! responses, so an idle Meta account would show no meters after every restart.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::Window;
use crate::accounts::{Pool, Provider};

/// Kept in the auth directory.
pub const FILE: &str = ".quota.state";

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    /// Account ID → the credential identity the flag was confirmed for.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    no_five_hour_limit: BTreeMap<String, String>,
    /// Account ID → the Meta account's last reported windows.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    meta: BTreeMap<String, Reading>,
}

/// A Meta account's windows as last reported, for the credential they belong to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Reading {
    identity: String,
    windows: Vec<Window>,
    updated_at: Option<DateTime<Utc>>,
}

impl Saved {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        crate::files::write_private(path, &serde_json::to_vec(self)?)
    }

    /// The pool's current flags and Meta readings.
    pub fn of(pool: &Pool) -> Self {
        let mut saved = Self::default();
        for a in pool.all() {
            let st = a.state.lock();
            if st.quota.no_five_hour_limit {
                saved.no_five_hour_limit.insert(a.id.clone(), st.credential_identity.clone());
            }
            if a.provider == Provider::Meta && !st.quota.windows.is_empty() {
                let reading = Reading {
                    identity: st.credential_identity.clone(),
                    windows: st.quota.windows.clone(),
                    updated_at: st.quota.updated_at,
                };
                saved.meta.insert(a.id.clone(), reading);
            }
        }
        saved
    }

    /// Puts the flags and readings back on accounts still signed in as the same
    /// subscription. Any later reading with a 5-hour window clears a flag again,
    /// headers included; a restored Meta reading keeps its age, and its next frame
    /// replaces it.
    pub fn restore(&self, pool: &Pool) {
        for a in pool.all() {
            let mut st = a.state.lock();
            if let Some(identity) = self.no_five_hour_limit.get(&a.id)
                && *identity == st.credential_identity
                && !st.quota.windows.iter().any(|w| w.name == "5h" && w.model.is_none())
            {
                st.quota.no_five_hour_limit = true;
            }
            if let Some(r) = self.meta.get(&a.id)
                && a.provider == Provider::Meta
                && r.identity == st.credential_identity
                && st.quota.windows.is_empty()
            {
                st.quota.windows = r.windows.clone();
                st.quota.updated_at = r.updated_at;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, account: &str) {
        std::fs::write(
            dir.join("claude-a.json"),
            serde_json::json!({"type":"claude","account_id":account,"access_token":"test-only","expired":"2099-01-01T00:00:00Z"})
                .to_string(),
        )
        .unwrap();
    }

    fn pool(dir: &Path) -> Pool {
        let pool = Pool::default();
        pool.reload(&crate::config::Config { auth_dir: dir.to_string_lossy().into(), ..Default::default() });
        pool
    }

    #[test]
    fn a_confirmed_absence_of_the_five_hour_limit_survives_a_restart_of_the_same_subscription_only() {
        let dir = std::env::temp_dir().join(format!("fusebox-quota-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir, "bc6d7b9b-441d-4b9b-a16b-38c1a38f9d7a");
        let before = pool(&dir);
        assert_eq!(Saved::of(&before), Saved::default());
        before.all()[0].state.lock().quota.no_five_hour_limit = true;
        let path = dir.join(FILE);
        Saved::of(&before).save(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }

        let restarted = pool(&dir);
        Saved::load(&path).restore(&restarted);
        assert!(restarted.all()[0].state.lock().quota.no_five_hour_limit);
        assert_eq!(Saved::of(&restarted), Saved::load(&path));

        // A 5-hour window from headers still clears it, as before.
        let acct = restarted.all()[0].clone();
        acct.state
            .lock()
            .set_quota(vec![crate::quota::Window { name: "5h".into(), used: 1.0, resets_at: None, model: None }], None);
        assert!(!acct.state.lock().quota.no_five_hour_limit);

        // Another subscription signed in to the same file starts without it.
        write(&dir, "0b0c7b9b-441d-4b9b-a16b-38c1a38f9d7b");
        let other = pool(&dir);
        Saved::load(&path).restore(&other);
        assert!(!other.all()[0].state.lock().quota.no_five_hour_limit);
        assert_eq!(Saved::of(&other), Saved::default());

        // A missing or damaged file restores nothing.
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(Saved::load(&path), Saved::default());
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn write_meta(dir: &Path, email: &str) {
        std::fs::write(
            dir.join("meta-a.json"),
            serde_json::json!({"type":"meta","auth_kind":"oauth","api_key":"test-only","base_url":"https://api.meta.ai/v1",
                "dca_token":"test-only","email":email})
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn a_meta_accounts_last_reading_survives_a_restart_of_the_same_sign_in_only() {
        let dir = std::env::temp_dir().join(format!("fusebox-quota-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        write_meta(&dir, "a@example.com");
        let before = pool(&dir);
        assert_eq!(before.all()[0].provider, Provider::Meta);
        assert_eq!(Saved::of(&before), Saved::default(), "no reading yet, nothing to keep");
        let resets = Some(Utc::now() + chrono::Duration::hours(3));
        let windows = vec![
            Window { name: "5h".into(), used: 3.0, resets_at: resets, model: None },
            Window { name: "week".into(), used: 1.0, resets_at: resets, model: None },
        ];
        before.all()[0].state.lock().set_quota(windows.clone(), None);
        let seen = before.all()[0].state.lock().quota.updated_at;
        let path = dir.join(FILE);
        Saved::of(&before).save(&path).unwrap();

        let restarted = pool(&dir);
        Saved::load(&path).restore(&restarted);
        {
            let acct = restarted.all()[0].clone();
            let st = acct.state.lock();
            assert_eq!(st.quota.windows, windows);
            assert_eq!(st.quota.updated_at, seen, "the reading keeps its age");
            assert!(st.quota.refreshed_at.is_none(), "a restored reading is not a fresh one");
            assert_eq!(st.quota.five_hour_remaining("muse-spark-1.3"), Some(97.0));
        }
        assert_eq!(Saved::of(&restarted), Saved::load(&path));

        // Another account signed in to the same file starts empty.
        write_meta(&dir, "b@example.com");
        let other = pool(&dir);
        Saved::load(&path).restore(&other);
        assert!(other.all()[0].state.lock().quota.windows.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
