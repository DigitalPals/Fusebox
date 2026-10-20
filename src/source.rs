//! Where a request came from.
//!
//! Fusebox sees the address of whatever opened the connection: the client itself, or a
//! proxy in front of it. A reverse proxy (tailscale serve, nginx, Caddy) names the client
//! in a forwarding header. A TCP forwarder that passes TLS through untouched
//! (cliproxy-gate, HAProxy in TCP mode) can't add headers; it names the client in a PROXY
//! protocol header instead, on a port of its own (`proxy-protocol-port`), where every
//! connection must start with one. Guessing whether a connection starts with one would let
//! any client behind such a forwarder claim to be someone else, so the ports never mix.
//!
//! Only proxies in `trusted-proxies` (this machine, unless set) are believed, on either
//! path. A raw TCP forwarder on this machine that connects to the ordinary port looks like
//! this machine, and so can't be told apart from a reverse proxy there. Once a PROXY
//! protocol port is on, forwarding headers therefore count only on that port: a forwarder
//! that falls back to the ordinary port can't pass its clients' headers off as its own.
//!
//! The source is worked out once per request, in [`layer`]. It is kept in the request's
//! extensions and, for code that only sees headers, in the internal `x-fusebox-source`
//! header, which never leaves Fusebox (`upstream::forwardable` drops `x-fusebox-*`).

use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::net::TcpStream;

use crate::config::Config;
use crate::state::App;

/// The resolved source, for code that only sees a request's headers.
pub const HEADER: &str = "x-fusebox-source";

/// How long a forwarder has to send its PROXY header after connecting.
const HEADER_TIMEOUT: Duration = Duration::from_secs(5);

/// How Fusebox learned the address.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    /// The connection's own address.
    #[default]
    Direct,
    /// A TCP forwarder's PROXY protocol header.
    ProxyProtocol,
    /// A trusted reverse proxy's forwarding header.
    Forwarded,
    /// A trusted proxy's forwarding chain did not name a readable client. The address is
    /// the proxy's own; address lists refuse it.
    Unknown,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::ProxyProtocol => "proxy_protocol",
            Self::Forwarded => "forwarded",
            Self::Unknown => "unknown",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [Self::Direct, Self::ProxyProtocol, Self::Forwarded, Self::Unknown].into_iter().find(|v| v.as_str() == s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub ip: IpAddr,
    pub via: Via,
    /// The Tailscale user, when tailscale serve forwarded the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
}

impl Source {
    fn header(&self) -> Option<HeaderValue> {
        let mut s = url::form_urlencoded::Serializer::new(String::new());
        s.append_pair("ip", &self.ip.to_string()).append_pair("via", self.via.as_str());
        if let Some(login) = &self.login {
            s.append_pair("login", login);
        }
        HeaderValue::from_str(&s.finish()).ok()
    }

    /// The source [`layer`] stamped on a request.
    pub fn from_headers(headers: &HeaderMap) -> Option<Self> {
        let value = headers.get(HEADER)?.to_str().ok()?;
        let (mut ip, mut via, mut login) = (None, None, None);
        for (k, v) in url::form_urlencoded::parse(value.as_bytes()) {
            match k.as_ref() {
                "ip" => ip = v.parse().ok(),
                "via" => via = Via::parse(&v),
                "login" => login = Some(v.into_owned()),
                _ => {}
            }
        }
        Some(Self { ip: ip?, via: via?, login })
    }
}

// --------------------------------------------------------------- address ranges

/// An address or a range of them (`10.0.0.0/8`, `fd7a:115c:a1e0::/48`, `100.64.0.7`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    net: u128,
    mask: u128,
    v4: bool,
}

impl Cidr {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (addr, prefix) = match text.split_once('/') {
            Some((a, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => (a, Some(p.parse::<u32>().ok()?)),
            Some(_) => return None,
            None => (text, None),
        };
        let (bits, width, v4) = match addr.parse::<IpAddr>().ok()?.to_canonical() {
            IpAddr::V4(a) => (u128::from(u32::from(a)), 32, true),
            IpAddr::V6(a) => (u128::from(a), 128, false),
        };
        let prefix = prefix.unwrap_or(width);
        if prefix > width {
            return None;
        }
        let full = if width == 128 { u128::MAX } else { (1u128 << width) - 1 };
        let mask = if prefix == 0 { 0 } else { (full << (width - prefix)) & full };
        Some(Self { net: bits & mask, mask, v4 })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match ip.to_canonical() {
            IpAddr::V4(a) if self.v4 => u128::from(u32::from(a)) & self.mask == self.net,
            IpAddr::V6(a) if !self.v4 => u128::from(a) & self.mask == self.net,
            _ => false,
        }
    }
}

/// Proxies whose word on the client's address counts. This machine, unless set.
pub fn trusted_proxies(cfg: &Config) -> Vec<Cidr> {
    match &cfg.trusted_proxies {
        Some(list) => list.iter().filter_map(|s| Cidr::parse(s)).collect(),
        None => ["127.0.0.0/8", "::1"].iter().filter_map(|s| Cidr::parse(s)).collect(),
    }
}

fn listed(list: &[Cidr], ip: IpAddr) -> bool {
    list.iter().any(|c| c.contains(ip))
}

/// Whether a named client's address list lets this source in. An empty list allows any.
pub fn allowed(patterns: &[String], ip: Option<IpAddr>) -> bool {
    patterns.is_empty() || ip.is_some_and(|ip| patterns.iter().filter_map(|p| Cidr::parse(p)).any(|c| c.contains(ip)))
}

// ------------------------------------------------------------------ resolution

/// The client behind `peer`: the peer itself, unless it is a trusted proxy that names one.
/// `forwarder` says the peer came from a PROXY protocol header.
pub fn resolve(peer: IpAddr, forwarder: bool, headers: &HeaderMap, trusted: &[Cidr]) -> Source {
    let peer = peer.to_canonical();
    if listed(trusted, peer) && ["x-forwarded-for", "forwarded", "x-real-ip"].iter().any(|h| headers.contains_key(*h)) {
        // A proxy that forwarded a chain without a readable client hasn't named one;
        // that isn't the proxy itself, so it must not pass for it.
        return match forwarded_for(headers, trusted) {
            Some(ip) => Source { ip, via: Via::Forwarded, login: tailscale_login(headers) },
            None => Source { ip: peer, via: Via::Unknown, login: None },
        };
    }
    Source { ip: peer, via: if forwarder { Via::ProxyProtocol } else { Via::Direct }, login: None }
}

/// The client a trusted proxy names: in `X-Forwarded-For`, else `Forwarded`, else
/// `X-Real-IP`. Proxies add themselves on the right, so the list is read from the right,
/// passing over trusted proxies; what a client wrote itself is further left.
///
/// The first of those headers that is there decides, even when nothing in it can be read:
/// moving on to the next would let a client that spoiled one choose its address with
/// another. Entries are split before they are read as text, so a client's stray bytes on
/// the left can't hide what the proxy added on the right.
fn forwarded_for(headers: &HeaderMap, trusted: &[Cidr]) -> Option<IpAddr> {
    let entries = |name: &str| -> Vec<Option<String>> {
        headers
            .get_all(name)
            .iter()
            .flat_map(|v| v.as_bytes().split(|b| *b == b',').map(|e| std::str::from_utf8(e).ok().map(str::to_string)))
            .collect()
    };
    if headers.contains_key("x-forwarded-for") {
        return rightmost(entries("x-forwarded-for"), trusted);
    }
    if headers.contains_key("forwarded") {
        let fors = entries("forwarded").into_iter().map(|element| {
            element.and_then(|element| {
                element.split(';').find_map(|pair| {
                    let (k, v) = pair.split_once('=')?;
                    k.trim().eq_ignore_ascii_case("for").then(|| v.trim().to_string())
                })
            })
        });
        return rightmost(fors.collect(), trusted);
    }
    headers.get("x-real-ip").and_then(|v| std::str::from_utf8(v.as_bytes()).ok()).and_then(address)
}

fn rightmost(entries: Vec<Option<String>>, trusted: &[Cidr]) -> Option<IpAddr> {
    let mut last = None;
    for entry in entries.iter().rev() {
        // We only reach another entry while the hops to its right are trusted. If it
        // can't name a client, none of those intermediaries can stand in for that client.
        let ip = entry.as_deref().and_then(address)?;
        last = Some(ip);
        if !listed(trusted, ip) {
            break;
        }
    }
    last
}

/// `10.0.0.7`, `10.0.0.7:4711`, `2001:db8::1`, `[2001:db8::1]:4711`, with or without quotes.
fn address(text: &str) -> Option<IpAddr> {
    let text = text.trim().trim_matches('"');
    if let Some(rest) = text.strip_prefix('[') {
        return rest.split(']').next()?.parse::<Ipv6Addr>().ok().map(|a| IpAddr::V6(a).to_canonical());
    }
    text.parse::<IpAddr>().ok().or_else(|| text.parse::<SocketAddr>().ok().map(|s| s.ip())).map(|ip| ip.to_canonical())
}

/// Tailscale's `Tailscale-User-Login`, which tailscale serve sets for tailnet users.
fn tailscale_login(headers: &HeaderMap) -> Option<String> {
    let login = headers.get("tailscale-user-login")?.to_str().ok()?.trim();
    let login: String = login.chars().filter(|c| !c.is_control()).take(200).collect();
    (!login.is_empty()).then_some(login)
}

/// Resolves every request's source before anything else looks at it.
pub async fn layer(State(app): State<Arc<App>>, mut req: Request, next: Next) -> Response {
    req.headers_mut().remove(HEADER);
    let forwarded = req.extensions_mut().remove::<ProxyPeer>();
    if let Some(ProxyPeer(client)) = forwarded {
        // From here on the connection is the client's own, as if the forwarder weren't there.
        req.extensions_mut().insert(ConnectInfo(client));
    }
    if let Some(ConnectInfo(peer)) = req.extensions().get::<ConnectInfo<SocketAddr>>().copied() {
        // With a forwarder on its own port, forwarding headers count only there. A forwarder
        // that falls back to the ordinary port (an older Fusebox, or the instant this one
        // starts) connects from this machine, and its clients' headers would pass for a
        // trusted proxy's. The port bound at startup is the one that counts.
        let trusted = if app.startup_config.proxy_protocol_port != 0 && forwarded.is_none() {
            Vec::new()
        } else {
            trusted_proxies(&app.cfg())
        };
        let source = resolve(peer.ip(), forwarded.is_some(), req.headers(), &trusted);
        if let Some(value) = source.header() {
            req.headers_mut().insert(HEADER, value);
        }
        req.extensions_mut().insert(source);
    }
    next.run(req).await
}

// -------------------------------------------------------------- PROXY protocol

/// The client a PROXY protocol header named for a connection.
#[derive(Clone, Copy, Debug)]
pub struct ProxyPeer(pub SocketAddr);

/// What a PROXY protocol header said.
#[derive(Debug, PartialEq, Eq)]
pub enum Proxied {
    Client(SocketAddr),
    /// The forwarder's own connection (a health check), or one it can't describe.
    Unknown,
}

const V2_SIGNATURE: [u8; 12] = *b"\r\n\r\n\0\r\nQUIT\n";
/// The longest version 1 header, line end included.
const V1_MAX: usize = 107;
/// Version 2 addresses plus TLVs; more than any forwarder sends.
const V2_MAX: usize = 16 << 10;

fn invalid(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("PROXY protocol: {why}"))
}

/// Reads a version 1 or 2 header and nothing after it, so TLS starts where it ends.
pub async fn read_header<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Proxied> {
    // Both versions are at least 12 bytes long ("PROXY UNKNOWN\r\n" is 15).
    let mut head = [0u8; 12];
    r.read_exact(&mut head).await?;
    if head == V2_SIGNATURE {
        return read_v2(r).await;
    }
    if !head.starts_with(b"PROXY ") {
        return Err(invalid("the connection doesn't start with a header"));
    }
    let mut line = head.to_vec();
    while !line.ends_with(b"\r\n") {
        if line.len() >= V1_MAX {
            return Err(invalid("header line too long"));
        }
        line.push(r.read_u8().await?);
    }
    parse_v1(&line[..line.len() - 2])
}

fn parse_v1(line: &[u8]) -> io::Result<Proxied> {
    let text = std::str::from_utf8(line).map_err(|_| invalid("header is not text"))?;
    let mut parts = text.split(' ');
    let _proxy = parts.next();
    let family = parts.next().ok_or_else(|| invalid("no address family"))?;
    if family == "UNKNOWN" {
        return Ok(Proxied::Unknown);
    }
    let mut next = || parts.next().ok_or_else(|| invalid("missing field"));
    let (src, dst, sport, dport) = (next()?, next()?, next()?, next()?);
    if parts.next().is_some() {
        return Err(invalid("extra fields"));
    }
    let port = |p: &str| {
        (!p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()))
            .then(|| p.parse::<u16>().ok())
            .flatten()
            .ok_or_else(|| invalid("bad port"))
    };
    let (sport, _) = (port(sport)?, port(dport)?);
    let src: IpAddr = match family {
        "TCP4" => IpAddr::V4(src.parse::<Ipv4Addr>().map_err(|_| invalid("bad source address"))?),
        "TCP6" => IpAddr::V6(src.parse::<Ipv6Addr>().map_err(|_| invalid("bad source address"))?),
        _ => return Err(invalid("unknown address family")),
    };
    let dst_ok = match family {
        "TCP4" => dst.parse::<Ipv4Addr>().is_ok(),
        _ => dst.parse::<Ipv6Addr>().is_ok(),
    };
    if !dst_ok {
        return Err(invalid("bad destination address"));
    }
    Ok(Proxied::Client(SocketAddr::new(src.to_canonical(), sport)))
}

async fn read_v2<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Proxied> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).await?;
    let (version_command, family, len) = (b[0], b[1], usize::from(u16::from_be_bytes([b[2], b[3]])));
    if version_command >> 4 != 2 {
        return Err(invalid("unsupported version"));
    }
    if len > V2_MAX {
        return Err(invalid("header too long"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    match version_command & 0x0f {
        0 => return Ok(Proxied::Unknown),
        1 => {}
        _ => return Err(invalid("unknown command")),
    }
    Ok(match family >> 4 {
        1 if body.len() >= 12 => {
            let ip = Ipv4Addr::new(body[0], body[1], body[2], body[3]);
            Proxied::Client(SocketAddr::new(IpAddr::V4(ip), u16::from_be_bytes([body[8], body[9]])))
        }
        2 if body.len() >= 36 => {
            let ip = Ipv6Addr::from(<[u8; 16]>::try_from(&body[..16]).unwrap());
            Proxied::Client(SocketAddr::new(IpAddr::V6(ip).to_canonical(), u16::from_be_bytes([body[32], body[33]])))
        }
        1 | 2 => return Err(invalid("address block too short")),
        // Unix sockets and unspecified: nothing to say about the client.
        _ => Proxied::Unknown,
    })
}

/// Accepts connections on `proxy-protocol-port`: only from trusted proxies, and only once
/// their PROXY header has named the client. TLS, if on, starts after the header.
#[derive(Clone)]
pub struct ProxyProtocol {
    app: Arc<App>,
}

impl ProxyProtocol {
    pub fn new(app: Arc<App>) -> Self {
        Self { app }
    }
}

type Accepted<S> = Pin<Box<dyn Future<Output = io::Result<(TcpStream, WithClient<S>)>> + Send>>;

impl<S: Send + 'static> axum_server::accept::Accept<TcpStream, S> for ProxyProtocol {
    type Stream = TcpStream;
    type Service = WithClient<S>;
    type Future = Accepted<S>;

    fn accept(&self, mut stream: TcpStream, service: S) -> Self::Future {
        let trusted = trusted_proxies(&self.app.cfg());
        Box::pin(async move {
            let peer = stream.peer_addr()?;
            if !listed(&trusted, peer.ip()) {
                tracing::warn!(peer = %peer.ip(), "closed a connection to proxy-protocol-port from outside trusted-proxies");
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "not a trusted proxy"));
            }
            stream.set_nodelay(true)?;
            let header = tokio::time::timeout(HEADER_TIMEOUT, read_header(&mut stream))
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "PROXY protocol: no header in time"))
                .and_then(|r| r);
            let client = match header {
                Ok(Proxied::Client(client)) => client,
                Ok(Proxied::Unknown) => peer,
                Err(e) => {
                    tracing::debug!(peer = %peer.ip(), "closed a connection to proxy-protocol-port: {e}");
                    return Err(e);
                }
            };
            Ok((stream, WithClient { inner: service, peer: ProxyPeer(client) }))
        })
    }
}

/// Tags each request on a forwarded connection with the client its header named.
#[derive(Clone)]
pub struct WithClient<S> {
    inner: S,
    peer: ProxyPeer,
}

impl<S, B> tower_service::Service<axum::http::Request<B>> for WithClient<S>
where
    S: tower_service::Service<axum::http::Request<B>>,
{
    type Response = <S as tower_service::Service<axum::http::Request<B>>>::Response;
    type Error = <S as tower_service::Service<axum::http::Request<B>>>::Error;
    type Future = <S as tower_service::Service<axum::http::Request<B>>>::Future;

    fn poll_ready(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), <S as tower_service::Service<axum::http::Request<B>>>::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(
        &mut self,
        mut req: axum::http::Request<B>,
    ) -> <S as tower_service::Service<axum::http::Request<B>>>::Future {
        req.extensions_mut().insert(self.peer);
        self.inner.call(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn loopback() -> Vec<Cidr> {
        trusted_proxies(&Config::default())
    }

    #[test]
    fn ranges_match_addresses_of_their_own_family() {
        let lan = Cidr::parse("10.10.0.0/24").unwrap();
        assert!(lan.contains(ip("10.10.0.235")));
        assert!(lan.contains(ip("::ffff:10.10.0.7")), "IPv4-mapped addresses are IPv4");
        assert!(!lan.contains(ip("10.10.1.1")));
        assert!(!lan.contains(ip("::1")));
        let tailnet = Cidr::parse("fd7a:115c:a1e0::/48").unwrap();
        assert!(tailnet.contains(ip("fd7a:115c:a1e0:ab12::1")));
        assert!(!tailnet.contains(ip("fd7a:115c:a1e1::1")));
        assert!(Cidr::parse("100.64.0.7").unwrap().contains(ip("100.64.0.7")));
        assert!(!Cidr::parse("100.64.0.7").unwrap().contains(ip("100.64.0.8")));
        assert!(Cidr::parse("0.0.0.0/0").unwrap().contains(ip("8.8.8.8")));
        assert!(Cidr::parse("::/0").unwrap().contains(ip("2001:db8::1")));
        assert_eq!(Cidr::parse("10.10.0.99/24"), Cidr::parse("10.10.0.0/24"), "host bits are ignored");
        for bad in ["", "10.0.0.0/33", "::/129", "10.0.0/8", "10.0.0.0/", "10.0.0.0/x", "example.com", "10.0.0.0/-1"] {
            assert_eq!(Cidr::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn address_lists_allow_listed_sources_only() {
        let list = vec!["10.10.0.0/24".to_string(), "100.64.0.7".to_string()];
        assert!(allowed(&list, Some(ip("10.10.0.12"))));
        assert!(allowed(&list, Some(ip("100.64.0.7"))));
        assert!(!allowed(&list, Some(ip("100.64.0.8"))));
        assert!(!allowed(&list, None), "an unknown source is refused");
        assert!(allowed(&[], None), "no list allows anyone");
    }

    #[test]
    fn a_direct_connection_is_its_own_source() {
        let s = resolve(ip("10.10.0.7"), false, &headers(&[("x-forwarded-for", "1.2.3.4")]), &loopback());
        assert_eq!(s, Source { ip: ip("10.10.0.7"), via: Via::Direct, login: None }, "untrusted peers can't forward");
        let s = resolve(ip("127.0.0.1"), false, &HeaderMap::new(), &loopback());
        assert_eq!(s.via, Via::Direct);
        let s = resolve(ip("::ffff:127.0.0.1"), false, &HeaderMap::new(), &loopback());
        assert_eq!(s.ip, ip("127.0.0.1"));
    }

    #[test]
    fn trusted_proxies_name_the_client_from_the_right() {
        let t = loopback();
        // tailscale serve replaces X-Forwarded-For and adds the user.
        let s = resolve(
            ip("127.0.0.1"),
            false,
            &headers(&[("x-forwarded-for", "100.101.7.12"), ("tailscale-user-login", "someone@github")]),
            &t,
        );
        assert_eq!(s, Source { ip: ip("100.101.7.12"), via: Via::Forwarded, login: Some("someone@github".into()) });
        // nginx appends: the client's own claim is further left.
        let s = resolve(ip("127.0.0.1"), false, &headers(&[("x-forwarded-for", "6.6.6.6, 10.10.0.9")]), &t);
        assert_eq!(s.ip, ip("10.10.0.9"));
        // Several trusted hops are passed over.
        let s = resolve(ip("127.0.0.1"), false, &headers(&[("x-forwarded-for", "10.10.0.9, 127.0.0.1")]), &t);
        assert_eq!(s.ip, ip("10.10.0.9"));
        let s = resolve(
            ip("127.0.0.1"),
            false,
            &headers(&[("x-forwarded-for", "127.0.0.2"), ("x-forwarded-for", "::1")]),
            &t,
        );
        assert_eq!(s.ip, ip("127.0.0.2"), "a chain of trusted proxies ends at the left-most");
        // Garbage on the right means the proxy said nothing usable.
        let s = resolve(ip("127.0.0.1"), false, &headers(&[("x-forwarded-for", "10.0.0.1, unknown")]), &t);
        assert_eq!((s.ip, s.via), (ip("127.0.0.1"), Via::Unknown));
        let s = resolve(ip("127.0.0.1"), false, &headers(&[("x-forwarded-for", "10.0.0.1,")]), &t);
        assert_eq!(s.via, Via::Unknown, "a trailing comma is an empty entry");
        // Garbage beyond a trusted hop leaves the client unknown.
        let s = resolve(ip("127.0.0.1"), false, &headers(&[("x-forwarded-for", "nonsense, 127.0.0.5")]), &t);
        assert_eq!((s.ip, s.via), (ip("127.0.0.1"), Via::Unknown));
        // Ports and brackets.
        let s = resolve(ip("127.0.0.1"), false, &headers(&[("x-forwarded-for", "[2001:db8::7]:4711")]), &t);
        assert_eq!(s.ip, ip("2001:db8::7"));
        let s = resolve(ip("127.0.0.1"), false, &headers(&[("x-forwarded-for", "10.1.2.3:55")]), &t);
        assert_eq!(s.ip, ip("10.1.2.3"));
    }

    #[test]
    fn trusted_intermediaries_cannot_stand_in_for_an_unreadable_client() {
        for (name, value) in [
            ("x-forwarded-for", "unknown, 127.0.0.5"),
            ("x-forwarded-for", ", ::1, 127.0.0.5"),
            ("forwarded", "for=unknown, for=127.0.0.5"),
            ("forwarded", "by=192.0.2.1, for=\"[::1]\", for=127.0.0.5"),
        ] {
            let h = headers(&[(name, value), ("tailscale-user-login", "someone@github")]);
            assert_eq!(
                resolve(ip("127.0.0.1"), false, &h, &loopback()),
                Source { ip: ip("127.0.0.1"), via: Via::Unknown, login: None },
                "{name}: {value}"
            );
        }
        // Once an untrusted client has been named, its earlier claims don't matter.
        for (name, value) in [
            ("x-forwarded-for", "unknown, 10.10.0.7, 127.0.0.5"),
            ("forwarded", "for=unknown, for=10.10.0.7, for=127.0.0.5"),
        ] {
            let s = resolve(ip("127.0.0.1"), false, &headers(&[(name, value)]), &loopback());
            assert_eq!((s.ip, s.via), (ip("10.10.0.7"), Via::Forwarded), "{name}: {value}");
        }
    }

    #[test]
    fn forwarded_and_x_real_ip_are_used_when_x_forwarded_for_is_absent() {
        let t = loopback();
        let s = resolve(
            ip("127.0.0.1"),
            false,
            &headers(&[("forwarded", r#"for=6.6.6.6;proto=https, for="[2001:db8:cafe::17]:4711";by=127.0.0.1"#)]),
            &t,
        );
        assert_eq!(s.ip, ip("2001:db8:cafe::17"));
        let s = resolve(ip("127.0.0.1"), false, &headers(&[("x-real-ip", "10.0.0.3")]), &t);
        assert_eq!(s.ip, ip("10.0.0.3"));
        // X-Forwarded-For wins when both are there.
        let s = resolve(
            ip("127.0.0.1"),
            false,
            &headers(&[("x-real-ip", "10.0.0.3"), ("x-forwarded-for", "10.0.0.4")]),
            &t,
        );
        assert_eq!(s.ip, ip("10.0.0.4"));
    }

    #[test]
    fn stray_bytes_cannot_hide_what_the_proxy_added() {
        let t = loopback();
        let raw = |pairs: &[(&str, &[u8])]| {
            let mut h = HeaderMap::new();
            for (k, v) in pairs {
                h.append(
                    axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                    HeaderValue::from_bytes(v).unwrap(),
                );
            }
            h
        };
        // nginx appended the real address after a client's non-ASCII junk.
        let h = raw(&[("x-forwarded-for", b"\x80\xff, 192.0.2.7"), ("x-real-ip", b"192.0.2.42")]);
        assert_eq!(resolve(ip("127.0.0.1"), false, &h, &t).ip, ip("192.0.2.7"));
        // A spoiled header that is there still decides: no falling through to X-Real-IP.
        let h = raw(&[("x-forwarded-for", b"\x80"), ("x-real-ip", b"192.0.2.42")]);
        assert_eq!(
            resolve(ip("127.0.0.1"), false, &h, &t),
            Source { ip: ip("127.0.0.1"), via: Via::Unknown, login: None }
        );
        let h = raw(&[("forwarded", b"for=\x80"), ("x-real-ip", b"192.0.2.42")]);
        assert_eq!(resolve(ip("127.0.0.1"), false, &h, &t).via, Via::Unknown);
        let h = raw(&[("x-real-ip", b"\x80")]);
        assert_eq!(resolve(ip("127.0.0.1"), false, &h, &t).via, Via::Unknown);
        let h = raw(&[("x-forwarded-for", b"\x80, 127.0.0.5")]);
        assert_eq!(resolve(ip("127.0.0.1"), false, &h, &t).via, Via::Unknown);
        let h = raw(&[("forwarded", b"for=\x80, for=127.0.0.5")]);
        assert_eq!(resolve(ip("127.0.0.1"), false, &h, &t).via, Via::Unknown);
        // From an untrusted peer the headers mean nothing at all.
        assert_eq!(resolve(ip("10.10.0.7"), false, &h, &t).via, Via::Direct);
        // An empty header is still there.
        let h = raw(&[("x-forwarded-for", b""), ("x-real-ip", b"192.0.2.42")]);
        assert_eq!(resolve(ip("127.0.0.1"), false, &h, &t).ip, ip("127.0.0.1"));
    }

    #[test]
    fn trusted_proxies_can_be_set_or_turned_off() {
        let cfg = Config { trusted_proxies: Some(vec!["10.10.0.2".into()]), ..Default::default() };
        let t = trusted_proxies(&cfg);
        let fwd = headers(&[("x-forwarded-for", "192.168.1.5")]);
        assert_eq!(resolve(ip("10.10.0.2"), false, &fwd, &t).ip, ip("192.168.1.5"));
        assert_eq!(resolve(ip("127.0.0.1"), false, &fwd, &t).ip, ip("127.0.0.1"), "loopback is no longer trusted");
        let none = trusted_proxies(&Config { trusted_proxies: Some(vec![]), ..Default::default() });
        assert_eq!(resolve(ip("127.0.0.1"), false, &fwd, &none).ip, ip("127.0.0.1"));
    }

    #[test]
    fn forwarded_clients_keep_their_forwarder_and_may_still_be_proxied() {
        let t = loopback();
        // A LAN client through the gate: the PROXY header named it, and it can't forward.
        let s = resolve(ip("10.10.0.7"), true, &headers(&[("x-forwarded-for", "127.0.0.1")]), &t);
        assert_eq!((s.ip, s.via), (ip("10.10.0.7"), Via::ProxyProtocol));
        // tailscale serve through the gate: the PROXY header says loopback, the header says who.
        let s = resolve(ip("127.0.0.1"), true, &headers(&[("x-forwarded-for", "100.101.7.12")]), &t);
        assert_eq!((s.ip, s.via), (ip("100.101.7.12"), Via::Forwarded));
    }

    #[test]
    fn the_internal_header_round_trips() {
        let s = Source { ip: ip("2001:db8::1"), via: Via::Forwarded, login: Some("Ünïcode user@github".into()) };
        let mut h = HeaderMap::new();
        h.insert(HEADER, s.header().unwrap());
        assert_eq!(Source::from_headers(&h), Some(s));
        h.insert(HEADER, HeaderValue::from_static("ip=nonsense&via=direct"));
        assert_eq!(Source::from_headers(&h), None);
    }

    async fn header_of(bytes: &[u8]) -> (io::Result<Proxied>, Vec<u8>) {
        let mut reader = bytes;
        let result = read_header(&mut reader).await;
        (result, reader.to_vec())
    }

    #[tokio::test]
    async fn version_one_headers_are_read_exactly() {
        let (r, rest) = header_of(b"PROXY TCP4 10.10.0.7 10.10.0.235 51234 8317\r\n\x16\x03\x01").await;
        assert_eq!(r.unwrap(), Proxied::Client("10.10.0.7:51234".parse().unwrap()));
        assert_eq!(rest, b"\x16\x03\x01", "the TLS hello is left alone");
        let (r, _) = header_of(b"PROXY TCP6 2001:db8::7 2001:db8::1 4711 443\r\n").await;
        assert_eq!(r.unwrap(), Proxied::Client("[2001:db8::7]:4711".parse().unwrap()));
        let (r, rest) = header_of(b"PROXY UNKNOWN ffff::1 ffff::2 1 2\r\nGET").await;
        assert_eq!((r.unwrap(), rest.as_slice()), (Proxied::Unknown, &b"GET"[..]));
        let (r, _) = header_of(b"PROXY UNKNOWN\r\n").await;
        assert_eq!(r.unwrap(), Proxied::Unknown);
        for bad in [
            &b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"[..],
            b"PROXY TCP4 10.0.0.1 10.0.0.2 1 2 3\r\n",
            b"PROXY TCP4 2001:db8::1 10.0.0.2 1 2\r\n",
            b"PROXY TCP6 10.0.0.1 2001:db8::1 1 2\r\n",
            b"PROXY TCP4 10.0.0.1 10.0.0.2 1 99999\r\n",
            b"PROXY TCP4 10.0.0.1 10.0.0.2 +1 2\r\n",
            b"PROXY UDP4 10.0.0.1 10.0.0.2 1 2\r\n",
            b"PROXY TCP4 10.0.0.1 10.0.0.2 1",
        ] {
            assert!(header_of(bad).await.0.is_err(), "{}", String::from_utf8_lossy(bad));
        }
        let long = format!("PROXY UNKNOWN {}\r\n", "x".repeat(200));
        assert!(header_of(long.as_bytes()).await.0.is_err(), "lines are bounded");
    }

    #[tokio::test]
    async fn version_two_headers_are_read_exactly() {
        let v2 = |cmd: u8, fam: u8, body: &[u8]| {
            let mut b = V2_SIGNATURE.to_vec();
            b.extend([0x20 | cmd, fam]);
            b.extend((body.len() as u16).to_be_bytes());
            b.extend(body);
            b.extend(b"after");
            b
        };
        let ipv4 = [10, 10, 0, 7, 10, 10, 0, 235, 0xc8, 0x22, 0x20, 0x7d];
        let (r, rest) = header_of(&v2(1, 0x11, &ipv4)).await;
        assert_eq!(r.unwrap(), Proxied::Client("10.10.0.7:51234".parse().unwrap()));
        assert_eq!(rest, b"after");
        // TLVs after the addresses are skipped.
        let mut with_tlv = ipv4.to_vec();
        with_tlv.extend([0x04, 0x00, 0x02, 0xaa, 0xbb]);
        let (r, rest) = header_of(&v2(1, 0x11, &with_tlv)).await;
        assert!(matches!(r.unwrap(), Proxied::Client(_)));
        assert_eq!(rest, b"after");
        let mut ipv6 = vec![0u8; 36];
        ipv6[..16].copy_from_slice(&"2001:db8::7".parse::<Ipv6Addr>().unwrap().octets());
        ipv6[32..34].copy_from_slice(&4711u16.to_be_bytes());
        assert_eq!(
            header_of(&v2(1, 0x21, &ipv6)).await.0.unwrap(),
            Proxied::Client("[2001:db8::7]:4711".parse().unwrap())
        );
        assert_eq!(header_of(&v2(0, 0x00, &[])).await.0.unwrap(), Proxied::Unknown, "LOCAL");
        assert_eq!(header_of(&v2(1, 0x31, &[0; 216])).await.0.unwrap(), Proxied::Unknown, "unix sockets");
        assert!(header_of(&v2(1, 0x11, &ipv4[..8])).await.0.is_err());
        assert!(header_of(&v2(2, 0x11, &ipv4)).await.0.is_err());
        let mut wrong_version = v2(1, 0x11, &ipv4);
        wrong_version[12] = 0x11;
        assert!(header_of(&wrong_version).await.0.is_err());
    }
}
