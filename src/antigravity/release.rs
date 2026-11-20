//! Reads an Antigravity release's build number (`cl=`) from the release itself.
//!
//! The language server stamps its build into its binary (`changelist N with baseline
//! M`). The macOS release is a 200 MB ZIP; Fusebox reads only the ZIP's directory and
//! the language server's compressed bytes, inflating them as they arrive until the
//! stamp shows up (about 37 MB into 2.33.0's 76 MB). Nothing is written to disk.

use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use flate2::{Decompress, FlushDecompress, Status};
use reqwest::StatusCode;
use reqwest::header::{ACCEPT_ENCODING, CONTENT_RANGE, RANGE};

/// Google's release storage. A manifest naming anything else is not followed.
const STORAGE: &str = "https://storage.googleapis.com/antigravity-public/antigravity-hub/";
const LANGUAGE_SERVER: &str = "Antigravity.app/Contents/Resources/bin/language_server";
/// The end-of-directory record and the longest comment it can carry.
const TAIL: u64 = 22 + 65_535;
const MAX_DIRECTORY: u64 = 16 << 20;
/// Bytes kept between two inflated pieces, so a stamp split across them is found.
const CARRY: usize = 128;
const MARK: &[u8] = b"changelist ";
const BASELINE: &[u8] = b" with baseline ";

/// The macOS ZIP an update manifest names for `version`, when it is in Google's storage.
pub(super) fn release_url(manifest: &str, version: &str) -> Option<String> {
    let prefix = format!("{STORAGE}{version}-");
    manifest.lines().find_map(|line| {
        let line = line.trim_start();
        let url = line.strip_prefix("- ").unwrap_or(line).strip_prefix("url:")?.trim().trim_matches(['"', '\'']);
        (url.starts_with(&prefix) && url.ends_with("/Antigravity.zip") && !url.contains("..")).then(|| url.to_string())
    })
}

/// The build number stamped into the language server of the release ZIP at `url`.
pub(super) async fn read_build(http: &reqwest::Client, url: &str) -> Result<String> {
    let tail = body(ranged(http, url, None).await?, TAIL).await?;
    let (offset, size) = directory(&tail)?;
    ensure!(size > 0 && size <= MAX_DIRECTORY, "a {size}-byte ZIP directory");
    let directory = body(ranged(http, url, Some((offset, offset + size - 1))).await?, size).await?;
    let entry = find(&directory, LANGUAGE_SERVER)?;
    let header = body(ranged(http, url, Some((entry.offset, entry.offset + 29))).await?, 30).await?;
    let start = data_start(&header, entry.offset)?;
    ensure!(entry.compressed > 0, "an empty language server");
    let mut resp = ranged(http, url, Some((start, start + entry.compressed - 1))).await?;
    let mut reader = StampReader::new(entry.method)?;
    let mut read = 0;
    while let Some(chunk) = resp.chunk().await? {
        read += chunk.len() as u64;
        ensure!(read <= entry.compressed, "more language server than the ZIP directory says");
        if let Some(build) = reader.push(&chunk)? {
            return Ok(build);
        }
    }
    bail!("no build stamp in the language server")
}

/// `bytes=first-last` of `url`, or its last `TAIL` bytes; anything but that range fails.
async fn ranged(http: &reqwest::Client, url: &str, range: Option<(u64, u64)>) -> Result<reqwest::Response> {
    let header = match range {
        Some((first, last)) => format!("bytes={first}-{last}"),
        None => format!("bytes=-{TAIL}"),
    };
    let resp = http
        .get(url)
        .header(RANGE, header)
        .header(ACCEPT_ENCODING, "identity")
        .timeout(Duration::from_secs(300))
        .send()
        .await?;
    ensure!(resp.status() == StatusCode::PARTIAL_CONTENT, "the release answered {} to a range request", resp.status());
    if let Some((first, _)) = range {
        let start = resp
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok()?.strip_prefix("bytes ")?.split('-').next()?.parse::<u64>().ok());
        ensure!(start == Some(first), "the release sent another range than {first}");
    }
    Ok(resp)
}

/// A ranged response's bytes, at most `max` of them.
async fn body(mut resp: reqwest::Response, max: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        out.extend_from_slice(&chunk);
        ensure!(out.len() as u64 <= max, "the release sent more than {max} bytes");
    }
    Ok(out)
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The central directory's offset and size, from the end-of-directory record.
fn directory(tail: &[u8]) -> Result<(u64, u64)> {
    let at = tail.windows(4).rposition(|w| w == b"PK\x05\x06").context("no ZIP end record")?;
    ensure!(tail.len() >= at + 22, "a short ZIP end record");
    let (size, offset) = (u32_at(tail, at + 12), u32_at(tail, at + 16));
    ensure!(size != u32::MAX && offset != u32::MAX, "a ZIP64 release");
    Ok((offset.into(), size.into()))
}

#[derive(Debug, PartialEq)]
struct Entry {
    method: u16,
    compressed: u64,
    offset: u64,
}

/// The directory entry for `name`.
fn find(dir: &[u8], name: &str) -> Result<Entry> {
    let mut p = 0;
    while dir.len() >= p + 46 && dir[p..].starts_with(b"PK\x01\x02") {
        let end = p + 46 + usize::from(u16_at(dir, p + 28));
        ensure!(dir.len() >= end, "a short ZIP directory");
        if &dir[p + 46..end] == name.as_bytes() {
            let (compressed, offset) = (u32_at(dir, p + 20), u32_at(dir, p + 42));
            ensure!(compressed != u32::MAX && offset != u32::MAX, "a ZIP64 entry");
            return Ok(Entry { method: u16_at(dir, p + 10), compressed: compressed.into(), offset: offset.into() });
        }
        p = end + usize::from(u16_at(dir, p + 30)) + usize::from(u16_at(dir, p + 32));
    }
    bail!("no {name} in the release")
}

/// Where an entry's data starts, after its local header.
fn data_start(header: &[u8], offset: u64) -> Result<u64> {
    ensure!(header.len() >= 30 && header.starts_with(b"PK\x03\x04"), "no ZIP entry header");
    Ok(offset + 30 + u64::from(u16_at(header, 26)) + u64::from(u16_at(header, 28)))
}

/// Inflates the language server's bytes (or takes them as stored) and watches for the stamp.
struct StampReader {
    inflate: Option<Decompress>,
    out: Vec<u8>,
    carry: Vec<u8>,
}

impl StampReader {
    fn new(method: u16) -> Result<Self> {
        let inflate = match method {
            0 => None,
            8 => Some(Decompress::new(false)),
            other => bail!("ZIP method {other}"),
        };
        Ok(Self { inflate, out: vec![0; 1 << 18], carry: Vec::new() })
    }

    fn push(&mut self, mut input: &[u8]) -> Result<Option<String>> {
        let Some(inflate) = self.inflate.as_mut() else { return Ok(scan(&mut self.carry, input)) };
        loop {
            let (before_in, before_out) = (inflate.total_in(), inflate.total_out());
            let status = inflate.decompress(input, &mut self.out, FlushDecompress::None)?;
            let used = (inflate.total_in() - before_in) as usize;
            let made = (inflate.total_out() - before_out) as usize;
            input = &input[used..];
            if let Some(build) = scan(&mut self.carry, &self.out[..made]) {
                return Ok(Some(build));
            }
            // Done, or waiting for more input with nothing left to write out.
            if status == Status::StreamEnd || (used == 0 && made == 0) || (input.is_empty() && made < self.out.len()) {
                return Ok(None);
            }
        }
    }
}

/// Looks for the stamp in `data` and the few bytes before it, keeping the last few.
fn scan(carry: &mut Vec<u8>, data: &[u8]) -> Option<String> {
    carry.extend_from_slice(data);
    let found = stamp(carry);
    carry.drain(..carry.len().saturating_sub(CARRY));
    found
}

/// N in `changelist N with baseline M`.
fn stamp(hay: &[u8]) -> Option<String> {
    let mut from = 0;
    while let Some(at) = hay[from..].iter().position(|&b| b == MARK[0]) {
        let rest = &hay[from + at..];
        if let Some(rest) = rest.strip_prefix(MARK) {
            let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
            if (6..=15).contains(&digits)
                && rest[digits..].starts_with(BASELINE)
                && rest.get(digits + BASELINE.len()).is_some_and(u8::is_ascii_digit)
            {
                return Some(String::from_utf8_lossy(&rest[..digits]).into_owned());
            }
        }
        from += at + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::Arc;

    use axum::http::{HeaderMap, StatusCode as Code, header};
    use axum::response::IntoResponse;

    use super::*;

    const MANIFEST: &str = "version: 2.33.0\nfiles:\n  - url: https://storage.googleapis.com/antigravity-public/antigravity-hub/2.33.0-4731201358004224/darwin-arm/Antigravity.zip\n    sha512: akGKBoAWOxZmddrQyvSHDxmnCtaaLnz7jaBTDg+8g8vG/lytBldvIkHRPxXA+WWsm6Xq2FP+fSzWD/BKFmsBvQ==\n    size: 208556255\npath: Antigravity.zip\n";

    #[test]
    fn the_manifest_names_googles_zip() {
        let url = release_url(MANIFEST, "2.33.0").unwrap();
        assert!(url.ends_with("/2.33.0-4731201358004224/darwin-arm/Antigravity.zip"), "{url}");
        assert_eq!(release_url(MANIFEST, "2.34.0"), None, "another release's file");
        let elsewhere = MANIFEST.replace("storage.googleapis.com", "example.com");
        assert_eq!(release_url(&elsewhere, "2.33.0"), None);
        assert_eq!(release_url("version: 2.33.0\npath: Antigravity.zip\n", "2.33.0"), None);
    }

    #[test]
    fn the_stamp_is_the_changelist_before_its_baseline() {
        assert_eq!(
            stamp(b"..changelist 1023160026 with baseline 1022452901 in a mint client"),
            Some("1023160026".into())
        );
        assert_eq!(
            stamp(b"changelist abc changelist 12 with baseline 3 changelist 998057262 with baseline 9"),
            Some("998057262".into())
        );
        assert_eq!(stamp(b"changelist 1023160026 with baseline "), None, "cut before the baseline");
        assert_eq!(stamp(b"changelist 1023160026"), None);
    }

    /// Bytes that don't compress, with a stamp two thirds in and a near miss before it.
    fn language_server() -> Vec<u8> {
        let mut seed = 0x2545_f491_u32;
        let mut out: Vec<u8> = (0..600_000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed >> 24) as u8
            })
            .collect();
        for (at, text) in [(100_000, &b"changelist 77 with baseline 1"[..]), (400_000, STAMP)] {
            out[at..at + text.len()].copy_from_slice(text);
        }
        out
    }

    const STAMP: &[u8] = b"changelist 1023160026 with baseline 1022452901";

    fn cat(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    fn zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let (mut out, mut dir) = (Vec::new(), Vec::new());
        for (name, data, deflate) in entries {
            let body = if *deflate {
                let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
                e.write_all(data).unwrap();
                e.finish().unwrap()
            } else {
                data.to_vec()
            };
            let (method, offset, extra) = (if *deflate { 8u16 } else { 0 }, out.len() as u32, b"UT\x01\x00\x07");
            let sizes = [(body.len() as u32).to_le_bytes(), (data.len() as u32).to_le_bytes()].concat();
            let name_len = (name.len() as u16).to_le_bytes();
            out.extend(cat(&[b"PK\x03\x04\x14\x00\x00\x00", &method.to_le_bytes(), &[0; 8], &sizes, &name_len]));
            out.extend(cat(&[&(extra.len() as u16).to_le_bytes(), name.as_bytes(), extra, &body]));
            dir.extend(cat(&[b"PK\x01\x02\x14\x00\x14\x00\x00\x00", &method.to_le_bytes(), &[0; 8], &sizes]));
            dir.extend(cat(&[&name_len, &[0; 12], &offset.to_le_bytes(), name.as_bytes()]));
        }
        let (count, size, offset) = ((entries.len() as u16).to_le_bytes(), (dir.len() as u32).to_le_bytes(), out.len());
        out.extend(dir);
        out.extend(cat(&[
            b"PK\x05\x06\x00\x00\x00\x00",
            &count,
            &count,
            &size,
            &(offset as u32).to_le_bytes(),
            &[0, 0],
        ]));
        out
    }

    /// Serves `zip`, honouring ranges unless `ranges` is false.
    async fn serve(zip: Vec<u8>, ranges: bool) -> String {
        let zip = Arc::new(zip);
        let handler = move |headers: HeaderMap| {
            let zip = zip.clone();
            async move {
                let len = zip.len() as u64;
                let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()?.strip_prefix("bytes="));
                let (first, last) = match range.and_then(|r| r.split_once('-')) {
                    Some(_) if !ranges => return zip.to_vec().into_response(),
                    Some(("", n)) => (len - n.parse::<u64>().unwrap().min(len), len - 1),
                    Some((a, b)) => (a.parse().unwrap(), b.parse::<u64>().unwrap().min(len - 1)),
                    None => return zip.to_vec().into_response(),
                };
                let part = zip[first as usize..=last as usize].to_vec();
                (Code::PARTIAL_CONTENT, [(header::CONTENT_RANGE, format!("bytes {first}-{last}/{len}"))], part)
                    .into_response()
            }
        };
        let app = axum::Router::new().route("/Antigravity.zip", axum::routing::get(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/Antigravity.zip")
    }

    #[tokio::test]
    async fn reads_the_build_from_the_language_server_inside_the_zip() {
        let server = language_server();
        let http = reqwest::Client::new();
        for deflate in [true, false] {
            let release = zip(&[
                ("Antigravity.app/Contents/Info.plist", b"<plist/>", deflate),
                (LANGUAGE_SERVER, &server, deflate),
            ]);
            let url = serve(release, true).await;
            assert_eq!(read_build(&http, &url).await.unwrap(), "1023160026", "deflate {deflate}");
        }
        let without = serve(zip(&[("Antigravity.app/Contents/Info.plist", b"<plist/>", true)]), true).await;
        let err = read_build(&http, &without).await.unwrap_err().to_string();
        assert!(err.contains("language_server"), "{err}");
        // A server that ignores ranges would send all 200 MB: stop at once.
        let whole = serve(zip(&[(LANGUAGE_SERVER, &server, true)]), false).await;
        let err = read_build(&http, &whole).await.unwrap_err().to_string();
        assert!(err.contains("range"), "{err}");
    }

    /// Google's current release, read the way the updater reads it.
    #[tokio::test]
    #[ignore = "manual: downloads about 40 MB of Google's release; run with --ignored --nocapture"]
    async fn live_release() {
        let http = reqwest::Client::new();
        let manifest = http.get(crate::antigravity::MANIFEST_URL).send().await.unwrap().text().await.unwrap();
        let version = manifest.lines().find_map(|l| l.strip_prefix("version:")).unwrap().trim().to_string();
        let url = release_url(&manifest, &version).expect("a release in Google's storage");
        let started = std::time::Instant::now();
        let build = read_build(&http, &url).await.unwrap();
        println!("antigravity {version}: cl={build} in {:?}", started.elapsed());
        if let Some((_, known)) = crate::antigravity::CHANGELISTS.iter().find(|(v, _)| *v == version) {
            assert_eq!(build, *known);
        }
    }

    #[test]
    fn a_stamp_split_across_pieces_is_found() {
        let server = language_server();
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
        e.write_all(&server).unwrap();
        let deflated = e.finish().unwrap();
        for piece in [997, 65_536] {
            let mut reader = StampReader::new(8).unwrap();
            let found = deflated.chunks(piece).find_map(|chunk| reader.push(chunk).unwrap());
            assert_eq!(found.as_deref(), Some("1023160026"), "pieces of {piece}");
        }
        let mut stored = StampReader::new(0).unwrap();
        let found = server.chunks(7).find_map(|chunk| stored.push(chunk).unwrap());
        assert_eq!(found.as_deref(), Some("1023160026"));
        assert!(StampReader::new(12).is_err(), "bzip2");
    }
}
