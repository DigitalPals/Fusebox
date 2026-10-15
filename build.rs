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
fn build_assets() {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let mut declarations = String::from("static ASSETS: [(&str, Asset); 10] = [\n");
    for (path, file, content_type) in [
        ("/", "index.html", "text/html; charset=utf-8"),
        ("app.js", "app.js", "text/javascript; charset=utf-8"),
        ("usage.js", "usage.js", "text/javascript; charset=utf-8"),
        ("config.js", "config.js", "text/javascript; charset=utf-8"),
        ("i18n.js", "i18n.js", "text/javascript; charset=utf-8"),
        ("i18n-zh.js", "i18n-zh.js", "text/javascript; charset=utf-8"),
        ("style.css", "style.css", "text/css; charset=utf-8"),
        ("icon.svg", "icon.svg", "image/svg+xml"),
        ("manifest.webmanifest", "manifest.webmanifest", "application/manifest+json"),
        ("sw.js", "sw.js", "text/javascript; charset=utf-8"),
    ] {
        println!("cargo:rerun-if-changed=ui/{file}");
        let mut source = fs::read_to_string(format!("ui/{file}")).expect("read dashboard asset");
        if path == "/" {
            println!("cargo:rerun-if-changed=ui/logos.svg");
            let logos = fs::read_to_string("ui/logos.svg").expect("read logo sprite");
            source = source.replace("<!-- logos -->", &logos);
        }
        let mut encoder = GzBuilder::new().mtime(0).write(Vec::new(), Compression::best());
        encoder.write_all(source.as_bytes()).expect("compress dashboard asset");
        let compressed = encoder.finish().expect("finish compressed dashboard asset");
        fs::write(out.join(format!("{file}.gz")), &compressed).expect("write compressed dashboard asset");
        let etag = format!("\"{:x}\"", Sha256::digest(source.as_bytes()));
        let gzip_etag = format!("\"{:x}\"", Sha256::digest(&compressed));
        declarations.push_str(&format!(
            "({path:?}, Asset::new(include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{file}.gz\")), {content_type:?}, {etag:?}, {gzip_etag:?})),\n"
        ));
    }
    declarations.push_str("];\n");
    fs::write(out.join("ui-assets.rs"), declarations).expect("write dashboard asset table");
}
