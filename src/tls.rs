//! One TLS client setup for every upstream connection.
//!
//! HTTP clients and native WebSockets trust the same roots: the bundled Mozilla set
//! plus the operating system's store, so a locally installed CA works on both paths.
//! The roots are read once. Subscription clients keep their own resumption cache.

use std::sync::{Arc, LazyLock};

use rustls::{ClientConfig, RootCertStore};

static ROOTS: LazyLock<Arc<RootCertStore>> = LazyLock::new(|| {
    let mut roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let native = rustls_native_certs::load_native_certs();
    let (added, _) = roots.add_parsable_certificates(native.certs);
    if !native.errors.is_empty() {
        tracing::debug!(added, errors = native.errors.len(), "some system certificates could not be loaded");
    }
    Arc::new(roots)
});

/// Shared configs, so TLS sessions resume across clients for the same host.
static HTTP: LazyLock<ClientConfig> = LazyLock::new(|| config(&[b"h2", b"http/1.1"]));
static HTTP1: LazyLock<ClientConfig> = LazyLock::new(|| config(&[b"http/1.1"]));
#[cfg(test)]
static WEBSOCKET: LazyLock<Arc<ClientConfig>> = LazyLock::new(|| Arc::new(config(&[b"http/1.1"])));

fn config(alpn: &[&[u8]]) -> ClientConfig {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .with_root_certificates(ROOTS.clone())
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    config
}

/// Load the system roots ahead of the first request (they are read from disk).
pub fn warm() {
    LazyLock::force(&ROOTS);
}

/// For reqwest's `use_preconfigured_tls`, which uses the ALPN list as given.
pub fn http(http1_only: bool) -> ClientConfig {
    if http1_only { HTTP1.clone() } else { HTTP.clone() }
}

/// WebSockets are upgraded over HTTP/1.1, so the server must not pick h2.
#[cfg(test)]
pub fn websocket() -> Arc<ClientConfig> {
    WEBSOCKET.clone()
}

/// HTTP, control and WebSocket transports for one account identity. Cloning a
/// config preserves its verifier and session store, so reconnects can resume
/// within this account without sharing tickets with another subscription.
pub(crate) struct AccountTls {
    http: ClientConfig,
    http1: Arc<ClientConfig>,
    #[cfg(test)]
    sessions: Arc<rustls::client::ClientSessionMemoryCache>,
}

impl AccountTls {
    pub(crate) fn new() -> Self {
        let sessions = Arc::new(rustls::client::ClientSessionMemoryCache::new(256));
        let mut http = HTTP.clone();
        http.resumption = rustls::client::Resumption::store(sessions.clone());
        let mut http1 = http.clone();
        http1.alpn_protocols = vec![b"http/1.1".to_vec()];
        Self {
            http,
            http1: Arc::new(http1),
            #[cfg(test)]
            sessions,
        }
    }

    pub(crate) fn http(&self, http1: bool) -> ClientConfig {
        if http1 { self.http1.as_ref().clone() } else { self.http.clone() }
    }

    pub(crate) fn websocket(&self) -> Arc<ClientConfig> {
        self.http1.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configs_share_one_root_store_and_negotiate_the_right_protocols() {
        assert!(ROOTS.len() >= webpki_roots::TLS_SERVER_ROOTS.len());
        assert_eq!(http(false).alpn_protocols, [b"h2".to_vec(), b"http/1.1".to_vec()]);
        assert_eq!(http(true).alpn_protocols, [b"http/1.1".to_vec()]);
        assert_eq!(websocket().alpn_protocols, [b"http/1.1".to_vec()]);
    }

    #[test]
    fn reqwest_accepts_the_shared_config() {
        for http1 in [false, true] {
            reqwest::Client::builder().use_preconfigured_tls(http(http1)).build().expect("preconfigured rustls client");
        }
    }

    #[test]
    fn account_transports_reuse_only_their_own_resumption_storage() {
        use rustls::client::ClientSessionStore;
        let first = AccountTls::new();
        let second = AccountTls::new();
        let host = rustls::pki_types::ServerName::try_from("provider.example").unwrap();
        first.sessions.set_kx_hint(host.clone(), rustls::NamedGroup::X25519);
        assert_eq!(first.sessions.kx_hint(&host), Some(rustls::NamedGroup::X25519));
        assert_eq!(second.sessions.kx_hint(&host), None);
        assert!(Arc::ptr_eq(&first.websocket(), &first.websocket()));
        assert!(!Arc::ptr_eq(&first.websocket(), &second.websocket()));
        for http1 in [false, true] {
            reqwest::Client::builder().use_preconfigured_tls(first.http(http1)).build().unwrap();
        }
        assert_eq!(first.websocket().alpn_protocols, [b"http/1.1".to_vec()]);
    }
}
