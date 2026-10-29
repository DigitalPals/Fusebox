//! Names the commit a binary was built from, for `--version`, the dashboard and metrics.
//! `FUSEBOX_COMMIT` in the build environment wins (Docker builds have no .git); a build
//! outside a git checkout says `unknown`.

use std::process::Command;
use std::{fs, io::Write, path::PathBuf};

use flate2::{Compression, GzBuilder};
use sha2::{Digest, Sha256};

fn main() {
    build_assets();
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

/// Keep only compressed text in the executable. Hashes identify each negotiated
/// representation, and the index already contains the SVG sprite when compressed.
///
/// Each asset also gets a short version from its content. Where one asset names another
/// under `/ui/` (the index its scripts and stylesheet, i18n.js its dictionary), the
/// reference carries that version (`/ui/app.js?v=…`), so the browser may keep the file
/// until it changes. Assets are listed after every asset they name.
fn build_assets() {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let mut declarations = String::from("static ASSETS: [(&str, Asset); 10] = [\n");
    let mut versions: Vec<(String, String)> = Vec::new();
    for (path, file, content_type) in [
        ("app.js", "app.js", "text/javascript; charset=utf-8"),
        ("usage.js", "usage.js", "text/javascript; charset=utf-8"),
        ("config.js", "config.js", "text/javascript; charset=utf-8"),
        ("i18n-zh.js", "i18n-zh.js", "text/javascript; charset=utf-8"),
        ("i18n.js", "i18n.js", "text/javascript; charset=utf-8"),
        ("style.css", "style.css", "text/css; charset=utf-8"),
        ("icon.svg", "icon.svg", "image/svg+xml"),
        ("manifest.webmanifest", "manifest.webmanifest", "application/manifest+json"),
        ("sw.js", "sw.js", "text/javascript; charset=utf-8"),
        ("/", "index.html", "text/html; charset=utf-8"),
    ] {
        println!("cargo:rerun-if-changed=ui/{file}");
        let mut source = fs::read_to_string(format!("ui/{file}")).expect("read dashboard asset");
        if path == "/" {
            println!("cargo:rerun-if-changed=ui/logos.svg");
            let logos = fs::read_to_string("ui/logos.svg").expect("read logo sprite");
            source = source.replace("<!-- logos -->", &logos);
        }
        for (url, version) in &versions {
            for quote in ['"', '\''] {
                source = source.replace(&format!("{url}{quote}"), &format!("{url}?v={version}{quote}"));
            }
        }
        let digest = format!("{:x}", Sha256::digest(source.as_bytes()));
        let compressed = compressed_asset(&out, file, source.as_bytes(), &digest);
        let version = digest[..12].to_string();
        let etag = format!("\"{digest}\"");
        let gzip_etag = format!("\"{:x}\"", Sha256::digest(&compressed));
        declarations.push_str(&format!(
            "({path:?}, Asset::new(include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{file}.gz\")), {content_type:?}, {etag:?}, {gzip_etag:?}, {version:?})),\n"
        ));
        if path != "/" {
            versions.push((format!("/ui/{file}"), version));
        }
    }
    declarations.push_str("];\n");
    write_changed(&out.join("ui-assets.rs"), declarations.as_bytes());
}

/// Cargo reruns this script for commit changes and individual UI edits. Reuse
/// unchanged assets after reference rewriting, so dependency changes still
/// invalidate their parents. Bump the cache format when compression options change.
fn compressed_asset(out: &std::path::Path, file: &str, source: &[u8], digest: &str) -> Vec<u8> {
    let path = out.join(format!("{file}.gz"));
    let fingerprint_path = out.join(format!("{file}.gz.fingerprint"));
    let fingerprint = format!("gzip-v1:{digest}");
    if fs::read(&fingerprint_path).is_ok_and(|old| old == fingerprint.as_bytes())
        && let Ok(compressed) = fs::read(&path)
    {
        return compressed;
    }
    let mut encoder = GzBuilder::new().mtime(0).write(Vec::new(), Compression::best());
    encoder.write_all(source).expect("compress dashboard asset");
    let compressed = encoder.finish().expect("finish compressed dashboard asset");
    // A cancelled write followed by a source revert must not validate the old
    // fingerprint against a partially replaced gzip file.
    if let Err(error) = fs::remove_file(&fingerprint_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        panic!("invalidate dashboard asset fingerprint: {error}");
    }
    fs::write(path, &compressed).expect("write compressed dashboard asset");
    // Record validity only after the corresponding compressed file is written.
    fs::write(fingerprint_path, fingerprint).expect("write dashboard asset fingerprint");
    compressed
}

fn write_changed(path: &std::path::Path, bytes: &[u8]) {
    if !fs::read(path).is_ok_and(|old| old == bytes) {
        fs::write(path, bytes).expect("write dashboard asset table");
    }
}
