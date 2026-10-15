//! Decode authenticated API bodies before JSON or multipart extraction.

use std::sync::Arc;

use async_compression::tokio::bufread::{GzipDecoder, ZstdDecoder};
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, header};
use axum::middleware::Next;
use axum::response::Response;
use futures::{StreamExt, TryStreamExt};
use http_body_util::{LengthLimitError, Limited};
use tokio_util::io::{ReaderStream, StreamReader};

use super::{body_error, format_for_path};
use crate::state::App;

pub(super) use crate::admission::MAX_BODY_SIZE;

/// Validate POST encodings and bound compressed input after client authentication.
///
/// Accept one identity, gzip, or zstd encoding. Other methods pass through unread.
/// Bound input before decoding because a decoder can consume bytes without output.
pub(super) async fn encoded_body(State(app): State<Arc<App>>, mut req: Request, next: Next) -> Response {
    if req.method() != Method::POST {
        return next.run(req).await;
    }
    let values: Vec<_> = req.headers().get_all(header::CONTENT_ENCODING).iter().collect();
    let encoding = match values.as_slice() {
        [] => None,
        [value] => value.to_str().ok().map(str::trim).map(str::to_ascii_lowercase),
        _ => None,
    };
    if !values.is_empty() && (values.len() != 1 || !matches!(encoding.as_deref(), Some("identity" | "gzip" | "zstd"))) {
        let mut response =
            body_error(&app, format_for_path(req.uri().path()), req.headers(), 415, "unsupported Content-Encoding");
        response.headers_mut().insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip, zstd, identity"));
        return response;
    }
    if let Some(encoding) = encoding {
        req.headers_mut().insert(header::CONTENT_ENCODING, HeaderValue::from_str(&encoding).unwrap());
    }
    let cfg = app.cfg();
    let permit = if cfg.request_limits.max_in_flight == 0 && cfg.request_limits.max_in_flight_body_bytes == 0 {
        None
    } else {
        match app.admission.acquire(&cfg.request_limits) {
            Ok(permit) => Some(permit),
            Err(_) => return capacity_error(&app, &req),
        }
    };
    let limit = cfg.request_limits.body_limit(req.uri().path());
    if let Some(permit) = &permit {
        req.extensions_mut().insert(permit.clone());
    }
    req.extensions_mut().insert(BodyLimit(limit));
    let response = next.run(decode_body(limit_encoded_body(req, limit))).await;
    let Some(permit) = permit else { return response };
    // Keep admission through the stream, including client cancellation. Taking
    // the response headers alone must not release a large active request.
    let (parts, body) = response.into_parts();
    let stream = async_stream::stream! {
        let _permit = permit;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await { yield chunk; }
    };
    Response::from_parts(parts, Body::from_stream(stream))
}

#[derive(Clone, Copy)]
struct BodyLimit(usize);

fn capacity_error(app: &Arc<App>, req: &Request) -> Response {
    let mut response = body_error(
        app,
        format_for_path(req.uri().path()),
        req.headers(),
        503,
        "request capacity exhausted; retry later",
    );
    response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

/// Collect decoded POST bodies within the output limit before handlers run.
///
/// Record size and decoding failures without selecting or contacting a provider.
/// Other methods pass through unread, preserving WebSocket upgrade behavior.
pub(super) async fn decoded_body(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    if req.method() != Method::POST {
        return next.run(req).await;
    }
    let format = format_for_path(req.uri().path());
    let limit = req.extensions().get::<BodyLimit>().map_or(MAX_BODY_SIZE, |limit| limit.0);
    let permit = req.extensions().get::<Arc<crate::admission::Permit>>().cloned();
    let (parts, body) = req.into_parts();
    let body = if let Some(permit) = permit {
        Body::from_stream(body.into_data_stream().map(move |chunk| {
            let chunk = chunk.map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            permit.reserve(chunk.len()).map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(chunk)
        }))
    } else {
        body
    };
    match read_body(body, limit).await {
        Ok(bytes) => next.run(Request::from_parts(parts, Body::from(bytes))).await,
        Err((status, message)) => {
            let mut response = body_error(&app, format, &parts.headers, status, message);
            if status == 503 {
                response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            }
            response
        }
    }
}

/// Limit incoming body bytes independently of the decompressed output size.
fn limit_encoded_body(req: Request, limit: usize) -> Request {
    let (parts, body) = req.into_parts();
    Request::from_parts(parts, Body::new(Limited::new(body, limit)))
}

/// Stream a validated gzip or zstd body and remove its encoded representation headers.
///
/// Read every member or frame so trailing data and corruption reach the collector.
fn decode_body(req: Request) -> Request {
    let encoding = req.headers().get(header::CONTENT_ENCODING).and_then(|v| v.to_str().ok());
    if !matches!(encoding, Some("gzip" | "zstd")) {
        return req;
    }
    let gzip = encoding == Some("gzip");
    let (mut parts, body) = req.into_parts();
    parts.headers.remove(header::CONTENT_ENCODING);
    parts.headers.remove(header::CONTENT_LENGTH);
    let reader = StreamReader::new(body.into_data_stream().map_err(std::io::Error::other));
    // HTTP gzip bodies may contain multiple members, and zstd may contain multiple
    // frames. Read to EOF so neither remaining data nor trailing corruption is lost.
    let body = if gzip {
        let mut decoder = GzipDecoder::new(reader);
        decoder.multiple_members(true);
        Body::from_stream(ReaderStream::new(decoder))
    } else {
        let mut decoder = ZstdDecoder::new(reader);
        decoder.multiple_members(true);
        Body::from_stream(ReaderStream::new(decoder))
    };
    Request::from_parts(parts, body)
}

/// Collect a body, mapping either size limit to 413 and other stream failures to 400.
async fn read_body(body: Body, limit: usize) -> Result<Bytes, (u16, &'static str)> {
    to_bytes(body, limit).await.map_err(|error| {
        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
        while let Some(error) = source {
            if error.is::<LengthLimitError>() {
                return (413, "request body exceeds configured size limit");
            }
            if error.is::<crate::admission::Full>() {
                return (503, "request capacity exhausted; retry later");
            }
            source = error.source();
        }
        (400, "invalid or incomplete request body encoding")
    })
}

#[cfg(test)]
mod tests;
