//! Names the commit a binary was built from, for `--version`, the dashboard and metrics.
//! `FUSEBOX_COMMIT` in the build environment wins (Docker builds have no .git); a build
//! outside a git checkout says `unknown`.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=FUSEBOX_COMMIT");
    println!("cargo:rerun-if-changed=.git/HEAD");
    if let Ok(head) = std::fs::read_to_string(".git/HEAD")
        && let Some(reference) = head.trim().strip_prefix("ref: ")
    {
        println!("cargo:rerun-if-changed=.git/{reference}");
    }
    let commit = std::env::var("FUSEBOX_COMMIT")
        .ok()
        .filter(|c| !c.trim().is_empty())
        .or_else(|| {
            let out = Command::new("git").args(["rev-parse", "--short=7", "HEAD"]).output().ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        })
        .filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_alphanumeric()))
        // A full SHA (from CI) is shown short, as git would.
        .map(|c| if c.len() > 12 { c[..7].to_string() } else { c });
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    match commit {
        Some(c) => {
            println!("cargo:rustc-env=FUSEBOX_COMMIT={c}");
            println!("cargo:rustc-env=FUSEBOX_VERSION={version} ({c})");
        }
        None => {
            println!("cargo:rustc-env=FUSEBOX_COMMIT=unknown");
            println!("cargo:rustc-env=FUSEBOX_VERSION={version}");
        }
    }
}
