//! What a restart must not forget about quota: which accounts the provider confirmed
//! to have no 5-hour limit. Only a usage-endpoint reading can confirm that, and that
//! endpoint may be rate limited for a while after a restart; meanwhile the dashboard
//! would show a 5-hour meter the account doesn't have.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::accounts::Pool;

/// Kept in the auth directory.
pub const FILE: &str = ".quota.state";

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    /// Account ID → the credential identity the flag was confirmed for.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    no_five_hour_limit: BTreeMap<String, String>,
}

impl Saved {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        crate::files::write_private(path, &serde_json::to_vec(self)?)
    }

    /// The pool's current flags.
    pub fn of(pool: &Pool) -> Self {
        let no_five_hour_limit = pool
            .all()
            .into_iter()
            .filter_map(|a| {
                let st = a.state.lock();
                st.quota.no_five_hour_limit.then(|| (a.id.clone(), st.credential_identity.clone()))
            })
            .collect();
        Self { no_five_hour_limit }
    }

    /// Puts the flags back on accounts still signed in as the same subscription. Any
    /// later reading with a 5-hour window clears a flag again, headers included.
    pub fn restore(&self, pool: &Pool) {
        for a in pool.all() {
            if let Some(identity) = self.no_five_hour_limit.get(&a.id) {
                let mut st = a.state.lock();
                if *identity == st.credential_identity
                    && !st.quota.windows.iter().any(|w| w.name == "5h" && w.model.is_none())
                {
                    st.quota.no_five_hour_limit = true;
                }
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
}
