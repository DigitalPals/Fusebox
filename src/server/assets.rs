//! Precompressed dashboard text with representation-aware validators. Identity
//! clients share one lazy decoded copy instead of embedding a second asset set.
//!
//! Every asset has a short content version, and the index names its scripts and
//! stylesheet with it (`/ui/app.js?v=…`, see build.rs). A request carrying the current
//! version may be cached for good; any other is revalidated each time.

use std::io::Read;
use std::sync::OnceLock;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

struct Asset {
    gzip: &'static [u8],
    content_type: &'static str,
    etag: &'static str,
    gzip_etag: &'static str,
    version: &'static str,
    identity: OnceLock<Bytes>,
}

impl Asset {
    const fn new(
        gzip: &'static [u8],
        content_type: &'static str,
        etag: &'static str,
        gzip_etag: &'static str,
        version: &'static str,
    ) -> Self {
        Self { gzip, content_type, etag, gzip_etag, version, identity: OnceLock::new() }
    }

    fn identity(&self) -> Bytes {
        self.identity
            .get_or_init(|| {
                let mut decoded = Vec::new();
                flate2::read::GzDecoder::new(self.gzip)
                    .read_to_end(&mut decoded)
                    .expect("build-generated dashboard gzip");
                Bytes::from(decoded)
            })
            .clone()
    }
}

include!(concat!(env!("OUT_DIR"), "/ui-assets.rs"));

/// The `v` a request's query names, if any.
pub(super) fn version(query: Option<&str>) -> Option<String> {
    url::form_urlencoded::parse(query?.as_bytes()).find(|(key, _)| key == "v").map(|(_, v)| v.into_owned())
}

/// `version`: the `v` the request named. Only the asset's current version is immutable,
/// and never the index, which names the others.
pub(super) fn serve(path: &str, headers: &HeaderMap, version: Option<&str>) -> Response {
    let Some((_, asset)) = ASSETS.iter().find(|(name, _)| *name == path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let immutable = path != "/" && version == Some(asset.version);
    let cache = if immutable { "public, max-age=31536000, immutable" } else { "no-cache" };
    let gzip_quality = quality(headers, "gzip");
    let identity_quality = quality(headers, "identity");
    if gzip_quality == 0 && identity_quality == 0 {
        let mut response = StatusCode::NOT_ACCEPTABLE.into_response();
        response.headers_mut().insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        return response;
    }
    let gzip = gzip_quality > 0 && gzip_quality >= identity_quality;
    let etag = if gzip { asset.gzip_etag } else { asset.etag };
    let not_modified = headers.get_all(header::IF_NONE_MATCH).iter().filter_map(|h| h.to_str().ok()).any(|value| {
        value.split(',').map(str::trim).any(|tag| tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == etag)
    });
    // A conditional request must not decompress or allocate a response body.
    let mut response = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let bytes = if gzip { Bytes::from_static(asset.gzip) } else { asset.identity() };
        Response::new(Body::from(bytes))
    };
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(asset.content_type));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    h.insert(header::ETAG, HeaderValue::from_static(etag));
    if gzip {
        h.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    response
}

/// Explicit coding preferences override `*`. Identity remains acceptable unless
/// explicitly excluded (including `*;q=0`), as required by HTTP negotiation.
fn quality(headers: &HeaderMap, coding: &str) -> u16 {
    let mut explicit = None;
    let mut wildcard = None;
    for field in headers.get_all(header::ACCEPT_ENCODING).iter().filter_map(|h| h.to_str().ok()) {
        for entry in field.split(',') {
            let mut parts = entry.trim().split(';');
            let name = parts.next().unwrap_or_default().trim();
            let mut q = 1000;
            for parameter in parts {
                if let Some((key, value)) = parameter.trim().split_once('=')
                    && key.trim().eq_ignore_ascii_case("q")
                {
                    q = value
                        .trim()
                        .parse::<f32>()
                        .ok()
                        .filter(|q| q.is_finite() && (0.0..=1.0).contains(q))
                        .map(|q| (q * 1000.0) as u16)
                        .unwrap_or(0);
                }
            }
            if name.eq_ignore_ascii_case(coding) {
                explicit = Some(q);
            } else if name == "*" {
                wildcard = Some(q);
            }
        }
    }
    explicit.unwrap_or_else(|| {
        if coding == "identity" { if wildcard == Some(0) { 0 } else { 1000 } } else { wildcard.unwrap_or(0) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn every_text_asset_round_trips_with_distinct_representation_validators() {
        for (path, asset) in &ASSETS {
            let identity = serve(path, &HeaderMap::new(), None);
            assert_eq!(identity.status(), StatusCode::OK);
            assert!(!identity.headers().contains_key(header::CONTENT_ENCODING));
            assert_eq!(identity.headers()[header::VARY], "Accept-Encoding");
            let identity_etag = identity.headers()[header::ETAG].clone();
            let identity = to_bytes(identity.into_body(), 2 << 20).await.unwrap();
            let mut headers = HeaderMap::new();
            headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("br, GZip;q=1"));
            let gzip = serve(path, &headers, None);
            assert_eq!(gzip.headers()[header::CONTENT_ENCODING], "gzip");
            assert_ne!(gzip.headers()[header::ETAG], identity_etag);
            let compressed = to_bytes(gzip.into_body(), 2 << 20).await.unwrap();
            let mut decoded = Vec::new();
            flate2::read::GzDecoder::new(compressed.as_ref()).read_to_end(&mut decoded).unwrap();
            assert_eq!(identity.as_ref(), decoded, "{path}");
            assert_eq!(asset.identity(), identity);
            if *path == "/" {
                let html = std::str::from_utf8(&identity).unwrap();
                assert!(!html.contains("<!-- logos -->"));
                assert!(html.contains("<svg"));
            }
        }
    }

    #[tokio::test]
    async fn conditional_requests_match_only_the_selected_representation() {
        let mut headers = HeaderMap::new();
        let identity_etag = serve("app.js", &headers, None).headers()[header::ETAG].clone();
        headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
        headers.insert(header::IF_NONE_MATCH, identity_etag);
        assert_eq!(serve("app.js", &headers, None).status(), StatusCode::OK);
        let gzip_etag = serve("app.js", &headers, None).headers()[header::ETAG].clone();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_str(&format!("\"old\", W/{}", gzip_etag.to_str().unwrap())).unwrap(),
        );
        let response = serve("app.js", &headers, None);
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers()[header::ETAG], gzip_etag);
        assert_eq!(response.headers()[header::VARY], "Accept-Encoding");
        assert!(to_bytes(response.into_body(), 16).await.unwrap().is_empty());
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("*"));
        assert_eq!(serve("app.js", &headers, None).status(), StatusCode::NOT_MODIFIED);
        assert_eq!(serve("missing.js", &headers, None).status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_current_version_is_immutable_and_any_other_is_revalidated() {
        let asset = |name: &str| &ASSETS.iter().find(|(n, _)| *n == name).unwrap().1;
        let current = asset("app.js").version;
        assert_eq!(current.len(), 12);
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
        let response = serve("app.js", &headers, Some(current));
        assert_eq!(response.headers()[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(response.headers()[header::VARY], "Accept-Encoding");
        let etag = response.headers()[header::ETAG].clone();
        for stale in [None, Some("000000000000"), Some("")] {
            assert_eq!(serve("app.js", &headers, stale).headers()[header::CACHE_CONTROL], "no-cache", "{stale:?}");
        }
        // A revalidation of the current version keeps both its validator and its lifetime.
        headers.insert(header::IF_NONE_MATCH, etag);
        let response = serve("app.js", &headers, Some(current));
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
        // The index itself is never cached for good, whatever it is asked with.
        assert_eq!(
            serve("/", &HeaderMap::new(), Some(asset("/").version)).headers()[header::CACHE_CONTROL],
            "no-cache"
        );
        assert_eq!(version(Some("x=1&v=abc")), Some("abc".into()));
        assert_eq!(version(Some("x=1")), None);
        assert_eq!(version(None), None);
    }

    async fn text(name: &str) -> String {
        let body = to_bytes(serve(name, &HeaderMap::new(), None).into_body(), 2 << 20).await.unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn the_index_and_i18n_name_the_current_versions() {
        let index = text("/").await;
        let i18n = text("i18n.js").await;
        for (name, asset) in &ASSETS {
            let url = format!("/ui/{name}?v={}", asset.version);
            match *name {
                "app.js" | "usage.js" | "config.js" | "i18n.js" | "style.css" | "icon.svg" => {
                    assert!(index.contains(&format!("\"{url}\"")), "{url}")
                }
                "i18n-zh.js" => assert!(i18n.contains(&format!("'{url}'")), "{url}"),
                _ => {}
            }
        }
        assert!(!index.contains("\"/ui/app.js\""));
    }

    #[test]
    fn negotiates_quality_exclusions_wildcards_and_identity() {
        for (value, expected) in [
            ("gzip", Some(true)),
            ("gzip;q=0", Some(false)),
            ("gzip;q=0.5", Some(false)),
            ("gzip;q=0.5, identity;q=0.1", Some(true)),
            ("*;q=1", Some(true)),
            ("gzip;q=0, *;q=1", Some(false)),
            ("gzip;q=1, *;q=0", Some(true)),
            ("gzip;q=NaN", Some(false)),
            ("gzip;q=0, identity;q=0", None),
            ("*;q=0", None),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_str(value).unwrap());
            let response = serve("style.css", &headers, None);
            if let Some(gzip) = expected {
                assert_eq!(response.status(), StatusCode::OK, "{value}");
                assert_eq!(response.headers().contains_key(header::CONTENT_ENCODING), gzip, "{value}");
            } else {
                assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE, "{value}");
            }
        }
    }
}
