mod accounts;
mod addresses;
mod admission;
mod affinity;
mod antigravity;
mod attribution;
#[cfg(test)]
mod attribution_tests;
mod banked_resets;
mod cache;
mod clients;
mod compat;
mod config;
mod config_editor;
mod device;
mod devin;
mod diagnostics;
mod discovery;
mod embeddings;
#[cfg(test)]
mod feature_tests;
mod files;
mod formats;
mod ir;
mod latency;
mod media;
mod metrics;
mod mgmt;
mod oauth;
mod policy;
mod pools;
mod proxy;
mod push;
mod quota;
#[cfg(test)]
mod routing_tests;
mod schema;
mod server;
mod source;
mod sse;
mod state;
mod stream_limits;
mod tls;
mod token_count;
mod upstream;
mod usage;
mod vertex;
mod window_starter;
mod ws;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::accounts::Provider;
use crate::config::Config;
use crate::state::App;

#[derive(Parser)]
#[command(
    name = "fusebox",
    version = metrics::VERSION,
    about = "OpenAI / Claude / Gemini compatible proxy for your Claude, ChatGPT, Gemini, Antigravity, Grok, Kimi, Meta, Devin and Vertex accounts"
)]
struct Cli {
    /// Path to the config file (created with defaults if missing). Default: $FUSEBOX_CONFIG, else config.yaml.
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Manage metadata-only local usage imports (explicit opt-in).
    Usage {
        #[command(subcommand)]
        command: usage::imports::UsageCommand,
    },
    /// Synchronize local usage metadata without running a proxy.
    Collector {
        #[command(subcommand)]
        command: usage::collector::CollectorCommand,
    },
    /// Run the proxy server (default).
    Serve,
    /// Sign in to an account: claude, codex, antigravity, kimi, xai, meta, devin or vertex.
    Login {
        provider: String,
        /// Print the URL instead of opening a browser.
        #[arg(long)]
        no_browser: bool,
        /// Vertex: path to a service account key (JSON).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Vertex: region, e.g. us-central1 or global.
        #[arg(long, default_value = "us-central1")]
        location: String,
    },
    /// Show what the config and auth directory contain, without starting the server.
    /// Handy before switching from CLIProxyAPI.
    Check,
}

fn main() -> Result<()> {
    tune_allocator();
    tokio::runtime::Builder::new_multi_thread().enable_all().build().context("start the runtime")?.block_on(run())
}

/// glibc gives each thread that allocates under contention its own heap arena (up
/// to eight per core) and keeps freed memory in them. With large request bodies
/// on many threads that holds on to a lot: under sixteen 1 MB streams on a
/// 64-thread runtime, four arenas and a fixed 256 KiB mmap threshold (so big
/// buffers go back to the system when freed) took RSS from about 210 MiB to 110 MiB
/// for about 7% more CPU per request at 4,000 requests a second, and changed
/// nothing measurable on a two-core server. Runs before any thread starts; glibc's
/// own MALLOC_ARENA_MAX and MALLOC_MMAP_THRESHOLD_ still win when set.
fn tune_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn mallopt(param: std::ffi::c_int, value: std::ffi::c_int) -> std::ffi::c_int;
        }
        const M_MMAP_THRESHOLD: std::ffi::c_int = -3;
        const M_ARENA_MAX: std::ffi::c_int = -8;
        // SAFETY: mallopt only adjusts allocator parameters; called before other threads exist.
        unsafe {
            if std::env::var_os("MALLOC_ARENA_MAX").is_none() {
                mallopt(M_ARENA_MAX, 4);
            }
            if std::env::var_os("MALLOC_MMAP_THRESHOLD_").is_none() {
                mallopt(M_MMAP_THRESHOLD, 256 << 10);
            }
        }
    }
}

async fn run() -> Result<()> {
    // CLIProxyAPI's Go-style flags (-config, -claude-login, ...) work too.
    let cli = Cli::parse_from(compat::translate_args(std::env::args().collect()));
    let command = match cli.cmd {
        Some(Cmd::Usage { command: usage::imports::UsageCommand::Backup { to, database } }) => {
            let database = match database {
                Some(d) => d,
                None => {
                    let path = config_path(cli.config, |n| std::env::var(n).ok());
                    usage::database_path(&Config::load(&path)?, &path)
                }
            };
            return tokio::task::spawn_blocking(move || usage::store::backup(&database, &to)).await?;
        }
        Some(Cmd::Usage {
            command:
                command @ (usage::imports::UsageCommand::Compact { .. }
                | usage::imports::UsageCommand::Downgrade { .. }
                | usage::imports::UsageCommand::RollbackCopy { .. }),
        }) => {
            let (database, downgrade, rollback_to) = match command {
                usage::imports::UsageCommand::Compact { database } => (database, false, None),
                usage::imports::UsageCommand::Downgrade { database } => (database, true, None),
                usage::imports::UsageCommand::RollbackCopy { database, to } => (database, false, Some(to)),
                _ => unreachable!(),
            };
            let database = match database {
                Some(d) => d,
                None => {
                    let path = config_path(cli.config, |n| std::env::var(n).ok());
                    usage::database_path(&Config::load(&path)?, &path)
                }
            };
            let result = tokio::task::spawn_blocking(move || {
                if let Some(to) = rollback_to {
                    usage::store::rollback_copy(&database, &to)
                } else if downgrade {
                    usage::store::downgrade(&database)
                } else {
                    usage::store::compact(&database)
                }
            })
            .await??;
            println!("{}", serde_json::to_string_pretty(&result)?);
            return Ok(());
        }
        Some(Cmd::Usage { command }) => return usage::imports::run_command(command).await,
        Some(Cmd::Collector { command }) => return usage::collector::run_command(command).await,
        other => other,
    };
    let path = config_path(cli.config, |n| std::env::var(n).ok());
    if matches!(command, Some(Cmd::Check)) {
        return check(&path);
    }
    let cfg = Config::load(&path)?;
    init_logging(cfg.debug, cfg.log_format);
    if cfg.legacy_auth_dir {
        tracing::info!(
            "using sign-ins from {} because {} doesn't exist; set auth-dir in config.yaml to choose",
            config::LEGACY_AUTH_DIR,
            config::DEFAULT_AUTH_DIR
        );
    }
    std::fs::create_dir_all(cfg.auth_dir()).ok();

    let app = tokio::task::spawn_blocking(move || App::new(cfg, path)).await?;
    match command {
        Some(Cmd::Login { provider, no_browser, file, location }) => {
            login(app, &provider, no_browser, file, &location).await
        }
        _ => serve(app).await,
    }
}

/// `RUST_LOG` takes `target=level` directives (`fusebox=debug,hyper=warn`). Span and
/// field filters are not supported; a value that can't be read falls back to the default.
fn log_filter(value: Option<&str>, debug: bool) -> (tracing_subscriber::filter::Targets, bool) {
    let default = if debug { "fusebox=debug" } else { "fusebox=info" };
    match value.map(str::parse) {
        Some(Ok(targets)) => (targets, true),
        Some(Err(_)) => (default.parse().expect("default log filter"), false),
        None => (default.parse().expect("default log filter"), true),
    }
}

/// Text for people, or one JSON object per line for log collectors (`log-format: json`
/// or `FUSEBOX_LOG_FORMAT=json`).
fn init_logging(debug: bool, format: config::LogFormat) {
    use std::io::IsTerminal;
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let value = std::env::var("RUST_LOG").ok();
    let (targets, understood) = log_filter(value.as_deref(), debug);
    let json = match std::env::var("FUSEBOX_LOG_FORMAT").ok().as_deref().map(str::trim) {
        Some(f) if f.eq_ignore_ascii_case("json") => true,
        Some(f) if f.eq_ignore_ascii_case("text") => false,
        _ => format == config::LogFormat::Json,
    };
    if json {
        let layer = tracing_subscriber::fmt::layer().json().with_target(false).with_filter(targets);
        tracing_subscriber::registry().with(layer).init();
    } else {
        let layer = tracing_subscriber::fmt::layer()
            .with_target(false)
            .with_ansi(std::io::stdout().is_terminal())
            .compact()
            .with_filter(targets);
        tracing_subscriber::registry().with(layer).init();
    }
    if !understood {
        tracing::warn!("RUST_LOG takes target=level directives such as fusebox=debug; using the default instead");
    }
}

/// `--config`, then `FUSEBOX_CONFIG`, then the pre-rename `CLIPROXYAPI_RUST_CONFIG`, then config.yaml.
fn config_path(flag: Option<PathBuf>, get: impl Fn(&str) -> Option<String>) -> PathBuf {
    flag.or_else(|| config::first_env(&["FUSEBOX_CONFIG", "CLIPROXYAPI_RUST_CONFIG"], get).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("config.yaml"))
}

async fn serve(app: Arc<App>) -> Result<()> {
    let cfg = app.cfg();
    if !cfg.is_loopback() && cfg.api_keys.is_empty() && cfg.named_clients.is_empty() {
        tracing::warn!(
            "listening on {} without api-keys: anyone who can reach this port can use your accounts",
            cfg.host
        );
    }
    let addr: SocketAddr = format!("{}:{}", if cfg.host.is_empty() { "0.0.0.0" } else { &cfg.host }, cfg.port)
        .parse()
        .or_else(|_| format!("[{}]:{}", cfg.host, cfg.port).parse())
        .context("invalid host/port")?;
    // The forwarder's port is bound first, so a forwarder that falls back to the main port
    // when this one refuses rarely meets a Fusebox that is up. When it does (it dialled just
    // before both were bound), `source::layer` believes no forwarding headers on the main
    // port while this one is on, so its clients show as this machine, not as whoever they
    // claim to be.
    let forwarded_addr = (cfg.proxy_protocol_port != 0).then(|| SocketAddr::new(addr.ip(), cfg.proxy_protocol_port));
    let forwarded_listener = match forwarded_addr {
        Some(a) => {
            Some(tokio::net::TcpListener::bind(a).await.with_context(|| format!("binding proxy-protocol-port {a}"))?)
        }
        None => None,
    };
    let listener = tokio::net::TcpListener::bind(addr).await.with_context(|| format!("binding {addr}"))?;
    if !cfg.ignored.is_empty() {
        tracing::warn!("these CLIProxyAPI settings have no effect here: {}", cfg.ignored.join(", "));
    }
    let tls = if cfg.tls.enable {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cfg.tls.cert, &cfg.tls.key)
            .await
            .context("loading tls.cert / tls.key")?;
        Some(tls)
    } else {
        None
    };

    let import_task = app.usage.clone().map(|s| tokio::spawn(usage::imports::poller(s)));
    policy::refresh(&app).await;
    let background = [
        tokio::spawn(oauth::refresher(app.clone())),
        tokio::spawn(antigravity::version_updater(app.clone())),
        tokio::spawn(quota::poller(app.clone())),
        tokio::spawn(window_starter::poller(app.clone())),
        tokio::spawn(push::watcher(app.clone())),
        tokio::spawn(policy::refresher(app.clone())),
        tokio::spawn(watch(app.clone())),
    ];

    let scheme = if cfg.tls.enable { "https" } else { "http" };
    let shown = if addr.ip().is_unspecified() {
        format!("{scheme}://127.0.0.1:{}", addr.port())
    } else {
        format!("{scheme}://{addr}")
    };
    let accounts = app.pool.all();
    println!();
    println!("  \x1b[1mFusebox\x1b[0m {}", metrics::VERSION);
    println!("  dashboard  {shown}");
    println!("  openai     {shown}/v1");
    println!("  anthropic  {shown}");
    println!("  gemini     {shown}/v1beta");
    if let Some(a) = forwarded_addr {
        println!("  forwarded  {a} (PROXY protocol, from {})", trusted_list(&cfg));
    }
    println!("  accounts   {} loaded from {}", accounts.len(), cfg.auth_dir().display());
    if accounts.is_empty() {
        println!(
            "\n  No accounts yet. Run `fusebox login <provider>` (claude, codex, antigravity, kimi, xai,\n  meta, devin, vertex) or open the dashboard."
        );
    }
    println!();

    let service = server::router(app.clone()).into_make_service_with_connect_info::<SocketAddr>();
    // Streamed answers are many small writes; without TCP_NODELAY, Nagle's algorithm
    // holds each one back until the client acknowledges the previous one.
    // On Ctrl-C or SIGTERM, stop taking connections, tell open sockets (the dashboard,
    // Codex) to close, and give requests in flight up to DRAIN to finish. Then the
    // sessions and the usage database are saved, well inside systemd's or Docker's
    // stop timeout.
    let stopping = app.shutdown.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("shutting down; finishing requests in flight");
        stopping.cancel();
    });
    let forwarded =
        forwarded_listener.map(|l| tokio::spawn(serve_forwarded(app.clone(), l, tls.clone(), service.clone())));
    let result = if let Some(tls) = tls {
        let handle = axum_server::Handle::new();
        let stop = handle.clone();
        let stopping = app.shutdown.clone();
        tokio::spawn(async move {
            stopping.cancelled().await;
            stop.graceful_shutdown(Some(DRAIN));
        });
        axum_server::from_tcp_rustls(listener.into_std()?, tls)?
            .map(|acceptor| acceptor.acceptor(axum_server::accept::NoDelayAcceptor::new()))
            .handle(handle)
            .serve(service)
            .await
    } else {
        use axum::serve::ListenerExt;
        let listener = listener.tap_io(|tcp| {
            if let Err(e) = tcp.set_nodelay(true) {
                tracing::debug!("could not set TCP_NODELAY on a client connection: {e}");
            }
        });
        let stopping = app.shutdown.clone();
        let server = axum::serve(listener, service).with_graceful_shutdown(async move { stopping.cancelled().await });
        let stopping = app.shutdown.clone();
        tokio::select! {
            result = server => result,
            _ = async move { stopping.cancelled().await; tokio::time::sleep(DRAIN).await } => {
                tracing::warn!("stopping with requests still open after {}s", DRAIN.as_secs());
                Ok(())
            }
        }
    };
    if let Some(task) = forwarded {
        match task.await {
            Ok(Err(e)) => tracing::error!("proxy-protocol-port: {e:#}"),
            Err(e) => tracing::error!("proxy-protocol-port: {e}"),
            Ok(Ok(())) => {}
        }
    }
    if let Some(task) = import_task {
        task.abort();
        let _ = task.await;
    }
    for task in background {
        task.abort();
        let _ = task.await;
    }
    // A provider may already have rotated a refresh token for a disconnected
    // request. Let those bounded operations publish and persist before exiting.
    app.refresh_tasks.shutdown().await;
    app.sessions.save_async().await;
    save_cooldowns(&app);
    save_attribution(&app, true);
    app.addresses.save(&app.cfg().named_clients, true);
    if let Some(store) = &app.usage
        && store.shutdown().await.is_err()
    {
        tracing::error!("usage shutdown flush failed");
    }
    result.context("serving requests")
}

/// How long requests in flight get to finish once Fusebox is asked to stop.
const DRAIN: Duration = Duration::from_secs(10);

fn trusted_list(cfg: &Config) -> String {
    match &cfg.trusted_proxies {
        None => "this machine".into(),
        Some(list) if list.is_empty() => "nobody (trusted-proxies is empty)".into(),
        Some(list) => list.join(", "),
    }
}

/// `proxy-protocol-port`: the same service, for a TCP forwarder in front. Each connection
/// must come from a trusted proxy and start with a PROXY header naming the client; TLS,
/// if on, follows it. Stops with the main port.
async fn serve_forwarded(
    app: Arc<App>,
    listener: tokio::net::TcpListener,
    tls: Option<axum_server::tls_rustls::RustlsConfig>,
    service: axum::extract::connect_info::IntoMakeServiceWithConnectInfo<axum::Router, SocketAddr>,
) -> Result<()> {
    let handle = axum_server::Handle::new();
    let stop = handle.clone();
    let stopping = app.shutdown.clone();
    tokio::spawn(async move {
        stopping.cancelled().await;
        stop.graceful_shutdown(Some(DRAIN));
    });
    let accept = source::ProxyProtocol::new(app);
    let server = axum_server::from_tcp(listener.into_std()?)?.handle(handle);
    match tls {
        Some(tls) => {
            server.acceptor(axum_server::tls_rustls::RustlsAcceptor::new(tls).acceptor(accept)).serve(service).await?
        }
        None => server.acceptor(accept).serve(service).await?,
    }
    Ok(())
}

/// Keeps quota attribution across restarts (at most every 30 seconds, and at
/// shutdown).
fn save_attribution(app: &App, force: bool) {
    static LAST: parking_lot::Mutex<Option<std::time::Instant>> = parking_lot::Mutex::new(None);
    let mut last = LAST.lock();
    if !force && last.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
        return;
    }
    let (saved, changed) = app.pool.attributions();
    if !changed && !force {
        return;
    }
    *last = Some(std::time::Instant::now());
    let path = app.cfg().auth_dir().join(attribution::FILE);
    if let Err(e) = saved.save(&path) {
        tracing::warn!("could not save quota attribution: {e:#}");
    }
}

/// Keeps cooldowns across restarts; written only when they changed.
fn save_cooldowns(app: &App) {
    static LAST: parking_lot::Mutex<Option<accounts::Cooldowns>> = parking_lot::Mutex::new(None);
    let now = app.pool.cooldowns();
    let mut last = LAST.lock();
    if last.as_ref() == Some(&now) {
        return;
    }
    let path = app.cfg().auth_dir().join(accounts::COOLDOWNS_FILE);
    if let Err(e) = now.save(&path) {
        tracing::warn!("could not save cooldowns: {e:#}");
        return;
    }
    *last = Some(now);
}

/// Ctrl-C, or SIGTERM from `docker stop` / systemd.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async { if let Some(t) = term.as_mut() { t.recv().await; } else { std::future::pending::<()>().await } } => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn auth_signature(dir: &Path) -> Vec<(String, Option<SystemTime>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .map(|e| (e.file_name().to_string_lossy().to_string(), mtime(&e.path())))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Hot-reloads the config file and the auth directory.
async fn watch(app: Arc<App>) {
    let mut cfg_time = mtime(&app.cfg_path);
    let mut auth = auth_signature(&app.cfg().auth_dir());
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        app.sessions.flush(app.cfg().session_affinity_idle_seconds);
        save_cooldowns(&app);
        save_attribution(&app, false);
        app.addresses.save(&app.cfg().named_clients, false);
        let t = mtime(&app.cfg_path);
        if t != cfg_time {
            cfg_time = t;
            let _guard = app.config_write.lock();
            match std::fs::read_to_string(&app.cfg_path).map_err(anyhow::Error::from).and_then(|s| Config::parse(&s)) {
                Ok(cfg) => {
                    tracing::info!("config reloaded");
                    app.set_config(cfg);
                }
                Err(e) => tracing::error!("config not reloaded: {e:#}"),
            }
        }
        let sig = auth_signature(&app.cfg().auth_dir());
        if sig != auth {
            auth = sig;
            if !app.reload_suppressed() {
                tracing::info!("auth directory changed, reloading accounts");
                app.reload_accounts();
            }
        }
    }
}

async fn login(app: Arc<App>, provider: &str, no_browser: bool, file: Option<PathBuf>, location: &str) -> Result<()> {
    let Some(provider) = Provider::parse(provider) else {
        anyhow::bail!("unknown provider `{provider}` (claude, codex, antigravity, kimi, xai, meta, devin, vertex)")
    };
    if provider == Provider::Vertex {
        let Some(path) = file else { anyhow::bail!("pass the service account key with --file key.json") };
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let label = vertex::import(&app, &text, location).await?;
        println!("\n✓ Added Vertex service account {label}");
        return Ok(());
    }
    let (state, login) = mgmt::start_login(&app, provider).await?;
    if login.kind == "device" {
        println!(
            "\nOpen this URL and enter the code:\n\n  {}\n\n  code: \x1b[1m{}\x1b[0m\n",
            login.url,
            login.user_code.unwrap_or_default()
        );
    } else {
        println!("\nOpen this URL to sign in:\n\n  {}\n", login.url);
    }
    if !no_browser && open::that(&login.url).is_err() {
        println!("(could not open a browser automatically)");
    }
    if login.kind == "device" {
        println!("Waiting for approval…");
    } else if login.callback {
        println!("Waiting for the browser to finish… (or paste the redirect URL here)");
    } else {
        println!("Paste the URL your browser was redirected to:");
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(1);
    std::thread::spawn(move || {
        let mut line = String::new();
        while std::io::stdin().read_line(&mut line).is_ok_and(|n| n > 0) {
            if !line.trim().is_empty() && tx.blocking_send(line.trim().to_string()).is_err() {
                break;
            }
            line.clear();
        }
    });
    loop {
        tokio::select! {
            Some(input) = rx.recv(), if login.kind != "device" => {
                let (code, _) = mgmt::parse_pasted(&input);
                match mgmt::complete_login(&app, &state, &code).await {
                    Ok(label) => { println!("\n✓ Signed in as {label}"); return Ok(()); }
                    Err(e) => { println!("\n✗ {e:#}"); return Err(e); }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(400)) => {
                let status = app.logins.lock().get(&state).map(|l| (l.status, l.message.clone()));
                match status {
                    Some(("done", m)) => { println!("\n✓ Signed in as {}", m.unwrap_or_default()); return Ok(()); }
                    Some(("error", m)) => anyhow::bail!(m.unwrap_or_default()),
                    _ => {}
                }
            }
        }
    }
}

fn check(path: &Path) -> Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let cfg = Config::parse(&text)?;
    let bind = if cfg.host.is_empty() { "0.0.0.0" } else { &cfg.host };
    let key = &cfg.management_key;
    let hashed = ["$2a$", "$2b$", "$2y$"].iter().any(|p| key.starts_with(p));
    println!("\n  \x1b[1mFusebox\x1b[0m {} · {}\n", env!("CARGO_PKG_VERSION"), path.display());
    println!("  listen       {bind}:{}{}", cfg.port, if cfg.tls.enable { " (https)" } else { "" });
    println!(
        "  client keys  {}",
        if cfg.api_keys.is_empty() { "none (open)".to_string() } else { cfg.api_keys.len().to_string() }
    );
    println!(
        "  dashboard    {}",
        match (key.is_empty(), hashed, cfg.management_allow_remote) {
            (true, _, _) => "localhost only, no key".to_string(),
            (false, h, remote) => format!(
                "management key{}{}",
                if h { " (bcrypt hash)" } else { "" },
                if remote == Some(false) { ", localhost only" } else { "" }
            ),
        }
    );
    println!("  routing      {:?}, {} accounts per request", cfg.routing, cfg.request_retry.max(1));
    println!(
        "  affinity     {}, idle expiry {}s",
        if cfg.session_affinity { "on" } else { "off" },
        cfg.session_affinity_idle_seconds
    );
    if !cfg.proxy_url.is_empty() {
        println!("  proxy        {}", cfg.proxy_url);
    }
    crate::clients::configure(&cfg.client_identities);
    let identities: Vec<String> =
        clients::CONFIGURABLE.iter().map(|c| format!("{c} {}", clients::version(c))).collect();
    println!("  speaks as    {}", identities.join(", "));
    let dir = cfg.auth_dir();
    println!("  auth dir     {}", dir.display());

    let pool = accounts::Pool::default();
    pool.reload(&cfg);
    let all = pool.all();
    println!("\n  accounts     {}", all.len());
    let mut by: std::collections::BTreeMap<&str, (usize, usize)> = Default::default();
    for a in &all {
        let e = by.entry(a.provider.as_str()).or_default();
        if a.is_oauth() { e.0 += 1 } else { e.1 += 1 }
    }
    for (p, (oauth, keys)) in by {
        let mut parts = vec![];
        if oauth > 0 {
            parts.push(format!("{oauth} signed in"));
        }
        if keys > 0 {
            parts.push(format!("{keys} API key{}", if keys == 1 { "" } else { "s" }));
        }
        println!("    {p:<14}{}", parts.join(", "));
    }
    println!("  models       {}", pool.models().len());

    let mut skipped = vec![];
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "json") && accounts::read_oauth_file(&p).is_none() {
            let kind = std::fs::read_to_string(&p)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| v["type"].as_str().map(String::from))
                .unwrap_or_else(|| "unknown".into());
            skipped.push(format!("{} (type {kind})", p.file_name().unwrap_or_default().to_string_lossy()));
        }
    }
    if !cfg.ignored.is_empty() {
        println!("\n  not used here: {}", cfg.ignored.join(", "));
    }
    if !skipped.is_empty() {
        println!("  skipped credential files: {}", skipped.join(", "));
    }
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_filter_reads_target_directives_and_falls_back_on_unsupported_syntax() {
        use tracing::Level;
        use tracing_subscriber::filter::Targets;
        let enabled = |t: &Targets, target, level| t.would_enable(target, &level);
        let (t, ok) = log_filter(None, false);
        assert!(ok && enabled(&t, "fusebox::proxy", Level::INFO) && !enabled(&t, "fusebox::proxy", Level::DEBUG));
        let (t, ok) = log_filter(None, true);
        assert!(ok && enabled(&t, "fusebox", Level::DEBUG) && !enabled(&t, "hyper", Level::INFO));
        let (t, ok) = log_filter(Some("fusebox=debug,hyper=warn"), false);
        assert!(ok && enabled(&t, "fusebox", Level::DEBUG) && enabled(&t, "hyper", Level::WARN));
        assert!(!enabled(&t, "hyper", Level::INFO));
        let (t, ok) = log_filter(Some("info"), false);
        assert!(ok && enabled(&t, "anything", Level::INFO));
        let (t, ok) = log_filter(Some("fusebox[span{field=1}]=debug"), false);
        assert!(!ok && enabled(&t, "fusebox", Level::INFO) && !enabled(&t, "fusebox", Level::DEBUG));
    }

    #[test]
    fn config_path_prefers_the_flag_then_the_new_variable_then_the_old_one() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |n: &str| vars.iter().find(|(k, _)| *k == n).map(|(_, v)| v.to_string())
        };
        let both = env(&[("FUSEBOX_CONFIG", "/new.yaml"), ("CLIPROXYAPI_RUST_CONFIG", "/old.yaml")]);
        assert_eq!(config_path(Some("/flag.yaml".into()), both), PathBuf::from("/flag.yaml"));
        assert_eq!(config_path(None, both), PathBuf::from("/new.yaml"));
        assert_eq!(config_path(None, env(&[("CLIPROXYAPI_RUST_CONFIG", "/old.yaml")])), PathBuf::from("/old.yaml"));
        assert_eq!(config_path(None, env(&[("FUSEBOX_CONFIG", "")])), PathBuf::from("config.yaml"));
        assert_eq!(config_path(None, env(&[])), PathBuf::from("config.yaml"));
    }
}

#[cfg(test)]
mod optimization_tests;
