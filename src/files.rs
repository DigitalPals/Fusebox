//! Small state files that hold secrets or must never be seen half-written.

use std::io::Write;
use std::path::Path;

/// Replaces `path` with `bytes` atomically: a private (0600) temporary file in the same
/// directory is written, synced and renamed over the old one, then the directory is synced.
pub fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    write_private_via(path, &path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4())), bytes)
}

/// [`write_private`] through a fixed temporary file, for files one writer owns at a time
/// (a credential under its refresh lock). A temporary file left by a crash is reused,
/// and made private before anything is written to it.
pub fn write_private_via(path: &Path, temporary: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> anyhow::Result<()> {
        let mut file = options.open(temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(temporary, path)?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
