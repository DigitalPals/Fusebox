//! The organization reset lock is held by the caller while this journal is open.
use super::*;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    Pending,
    Unknown,
    Refused,
    Started,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Attempt {
    pub request_id: String,
    pub at: DateTime<Utc>,
    pub automatic: bool,
    pub outcome: Outcome,
    pub retry_after: DateTime<Utc>,
    pub resets_at: Option<DateTime<Utc>>,
}

impl Attempt {
    pub fn view(&self, now: DateTime<Utc>) -> View {
        let (status, message) = match self.outcome {
            Outcome::Pending | Outcome::Unknown => (
                "unknown",
                "Waiting for provider confirmation. Further starters are held until the retry time or a confirmed active window.",
            ),
            Outcome::Refused => ("refused", "Claude refused the starter request. Waiting before another attempt."),
            Outcome::Started => ("started", "The provider confirmed an active five-hour window."),
        };
        View {
            attempted_at: Some(self.at),
            resets_at: self.resets_at,
            retry_after: Some(self.retry_after),
            automatic: self.automatic,
            ..View::new(status, message, now)
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    version: u8,
    identity: String,
    pub attempt: Option<Attempt>,
    #[serde(skip)]
    path: PathBuf,
}

impl Journal {
    pub fn open(root: &std::path::Path, account: &str, organization: &str) -> Result<Self> {
        let identity = hex::encode(Sha256::digest(format!("claude:{organization}:{account}")));
        let dir = root.join(".window-starters");
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let path = dir.join(format!("{identity}.json"));
        let mut value: Self = match std::fs::File::open(&path) {
            Ok(file) => {
                use std::io::Read;
                ensure!(file.metadata()?.len() <= 16 * 1024, "Window starter journal is too large");
                let mut bytes = vec![];
                file.take(16 * 1024 + 1).read_to_end(&mut bytes)?;
                serde_json::from_slice(&bytes)
                    .context("Window starter journal is unreadable; automatic starting is blocked")?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Self { version: 1, identity: identity.clone(), attempt: None, path: PathBuf::new() }
            }
            Err(_) => return Err(anyhow!("Could not read window starter journal")),
        };
        ensure!(value.version == 1 && value.identity == identity, "Invalid window starter journal identity or version");
        if let Some(a) = &value.attempt {
            ensure!(
                uuid::Uuid::parse_str(&a.request_id).is_ok() && a.retry_after >= a.at,
                "Invalid window starter attempt; automatic starting is blocked"
            );
        }
        value.path = path;
        Ok(value)
    }

    pub fn save(&self) -> Result<()> {
        crate::files::write_private(&self.path, &serde_json::to_vec(self)?).map_err(|_| {
            anyhow!("Could not save window starter journal; further starters are blocked until reconciled")
        })
    }
}
