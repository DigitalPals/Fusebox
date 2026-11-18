# Installing and running Fusebox

The [README](../README.md#install) has a prompt that lets an AI agent do this for you. These are the steps by hand.

## Get it and start it

**1. Get the binary.** Download it for macOS, Linux or Windows from [Releases](https://github.com/DigitalPals/Fusebox/releases/latest). The Linux builds need glibc 2.36 or newer (Debian 12, Ubuntu 24.04 or later), and `gh attestation verify <archive> -R DigitalPals/Fusebox` confirms a download was built here. Or build it with Rust 1.89 or newer:

```sh
cargo install --git https://github.com/DigitalPals/Fusebox
```

Or run it with Docker (for amd64 and arm64):

```sh
touch config.yaml && mkdir -p auths
docker run -d --name fusebox -p 8317:8317 \
  -v ./config.yaml:/CLIProxyAPI/config.yaml -v ./auths:/root/.cli-proxy-api \
  ghcr.io/digitalpals/fusebox
```

**2. Start it.**

```sh
fusebox
```

The first run writes a commented `config.yaml` in the current directory (or wherever `--config` or `FUSEBOX_CONFIG` points) and serves everything on `http://127.0.0.1:8317`. That address is also the dashboard.

**3. Add an account.** Click **Connect account** in the dashboard, or use the terminal:

```sh
fusebox login claude        # Claude Pro / Max / Ultra
fusebox login codex         # ChatGPT Plus / Pro / Team
fusebox login antigravity   # Google account with Antigravity
fusebox login xai           # SuperGrok / X Premium (shows a code to confirm)
fusebox login kimi          # Kimi Code (shows a code to confirm)
fusebox login meta          # Meta Muse (shows a code to confirm)
fusebox login devin         # Devin / Windsurf
fusebox login vertex --file key.json --location global   # Vertex AI service account
```

API keys (Anthropic, OpenAI, Gemini, Vertex, Kimi, xAI, Meta, OpenRouter, Ollama, …) can be added from the dashboard or in `config.yaml`.

## Coming from CLIProxyAPI

Your config, your sign-ins and your Docker setup carry over as they are.

**Docker.** Keep your `docker-compose.yml`, `config.yaml` and `auths/` folder. Add one line to the `.env` next to the compose file and restart:

```sh
echo "CLI_PROXY_IMAGE=ghcr.io/digitalpals/fusebox:latest" >> .env
docker compose up -d
```

The image uses the same paths (`/CLIProxyAPI/config.yaml`, `/root/.cli-proxy-api`) and port, and `./CLIProxyAPI` still works inside the container. To go back, delete the line.

**Binary.** Point it at your existing file: `fusebox --config /path/to/config.yaml`. CLIProxyAPI's flags work too: `-config`, `-claude-login`, `-codex-login`, `-antigravity-login`, `-kimi-login`, `-xai-login`, `-meta-login`, `-devin-login`, `-vertex-import`, `-no-browser`.

**Check before you switch.** This prints the accounts it found per provider, where it will listen, and any settings it will ignore, without starting the server:

```sh
fusebox --config config.yaml check
```

| From CLIProxyAPI | |
| --- | --- |
| Credential files in `auth-dir` | Read and written in the same format, for every provider |
| `config.yaml` | Both layouts: v8 (`server:`, `access:`, grouped `api-keys:`) and the older flat one |
| Client keys, management key (plain or bcrypt-hashed), `allow-remote`, TLS, proxy, routing strategy, retries | Used as is |
| API keys for Claude, Codex, Gemini, Vertex, xAI, Meta and OpenAI-compatible providers | Used with their `base-url`, `proxy-url` (including `direct`), `headers`, model aliases, `prefix` and `excluded-models` |
| `oauth-model-alias`, `oauth-excluded-models`, per-file `prefix` and `model_aliases` | Used as is |
| Session affinity (`routing.session-affinity` or `session-affinity`) | Supported and on by default, with any routing strategy. See [coding sessions](routing.md#coding-sessions-and-prompt-caching). |
| Payload rules, plugins, Redis usage queue, weighted routing, the `/v0/management` API | Not supported. The built-in dashboard replaces the separate management panel. |

Changes made from the dashboard keep your file's layout, YAML comments, and settings this binary doesn't use, so you can switch back at any time. The rare layout that can't be edited in place, such as lists written without indentation, is rewritten instead, and the original is kept as `config.yaml.bak`.

`config.yaml` is read with YAML 1.2's core schema: `yes`, `on` and `017` stay strings, quoted values are always strings, a repeated key or a second document is an error, and `<<` is an ordinary key rather than a merge. Every file earlier releases loaded still loads, including sections Fusebox doesn't use that hold numeric keys, local tags or `.inf`. When the dashboard has to rewrite the file, it quotes strings other YAML readers could take for something else, such as `yes` or `017`, so CLIProxyAPI reads the same values.

Retiring an older Redis-based usage collector requires a compatible replacement and a database backup; see [production log follow-up](../notes/production-log-followup.md). Fusebox's native usage capture replaces that ingestion path.

### Compared with CLIProxyAPI

Fusebox is a smaller rewrite of [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI), not a port.

| | Fusebox | CLIProxyAPI |
| --- | --- | --- |
| Language | Rust, one binary | Go |
| Dashboard | Built in | Separate web panel |
| Codex WebSockets | Yes, native relay | Yes |
| Claude, ChatGPT, Antigravity, Grok, Kimi, Meta, Devin sign-in | Yes | Yes |
| Vertex service accounts, API keys, OpenAI-compatible | Yes | Yes |
| Images, xAI video, Responses compaction | Yes | Yes |
| Go plugins, Redis usage queue | No | Yes |
| Credential files, config, Docker image layout, CLI flags | Compatible with CLIProxyAPI's | — |

Plugins are Go shared libraries loaded into CLIProxyAPI's process, and the Redis queue feeds its separate usage service. Neither applies to a single Rust binary that keeps its own stats.

## Running it on a server

Set `host: "0.0.0.0"`, a client for each machine or tool (`named-clients`, or Config, Clients), and a `management-key` for the dashboard. In Docker the dashboard also needs a `management-key`, because browser requests reach the container from outside `localhost`. Then keep it running, for example with systemd:

```ini
# /etc/systemd/system/fusebox.service
[Unit]
Description=Fusebox
After=network-online.target

[Service]
ExecStart=/usr/local/bin/fusebox --config /etc/fusebox/config.yaml
Restart=always
TimeoutStopSec=90s

[Install]
WantedBy=multi-user.target
```

To sign in accounts on a server, open the dashboard, click **Connect account**, approve in your browser, then paste the `localhost` URL the browser lands on (it won't load, which is expected). Grok, Kimi and Meta use device codes, so they work from anywhere with nothing to paste. `fusebox login <provider>` on the server works the same way.

On Ctrl-C or SIGTERM, Fusebox stops taking connections, tells open WebSockets to reconnect, and gives requests in flight up to 10 seconds before it saves sessions, cooldowns and the usage database, well inside `TimeoutStopSec`. A request still open after 9 seconds is ended: a streamed answer stops where it stands with a last error event, a waiting one gets a 503 with `Retry-After: 1`, and a turn running on a WebSocket ends with its socket. Each is recorded with what it used by then, as stopped by Fusebox (`server_shutdown`) and not as a client that left.

**Keeping an eye on it.** `GET /healthz` answers while the process runs. `GET /readyz` answers 200 when at least one account can take requests and the usage database works, and 503 with the reason otherwise. `GET /metrics` serves Prometheus metrics (requests, tokens, each account's readiness, load and quota windows, faults, client spend, private Responses events left out per client program), with the dashboard's authentication: from localhost, or with the management key as a bearer token:

```yaml
scrape_configs:
  - job_name: fusebox
    metrics_path: /metrics
    authorization: { credentials: "<management-key>" }
    static_configs: [{ targets: ["127.0.0.1:8317"] }]
```

Metrics also expose separate input, cache-read, cache-write, output and reasoning counters across attempts, reported/unknown usage fields, preparation/header/first-content/total latency histograms, retry counts, WebSocket fallback reasons and exact embedding-cache statistics. Model/provider/transport series are bounded; session identifiers and prompt hashes are excluded. Reasoning is an output subset and must not be added to output again. See the [optimization implementation and measurements](efficiency-2026-10-20.md) and [context experiment guide](context-efficiency.md).

`log-format: json` (or `FUSEBOX_LOG_FORMAT=json`) writes one JSON object per line for log collectors. `fusebox usage backup ~/usage-backup.sqlite3` copies the usage database safely while Fusebox runs and checks the copy. `fusebox --version` names the commit it was built from.

**Docker without root.** The `-nonroot` tags (`ghcr.io/digitalpals/fusebox:latest-nonroot`) run as uid 65532 instead of root. Mount the config at `/home/nonroot/config.yaml` and sign-ins at `/home/nonroot/.fusebox`, owned by that uid:

```sh
mkdir -p auths && touch config.yaml && sudo chown -R 65532 auths config.yaml
docker run -d --name fusebox -p 8317:8317 -e FUSEBOX_DEFAULT_HOST=0.0.0.0 \
  -v ./config.yaml:/home/nonroot/config.yaml -v ./auths:/home/nonroot/.fusebox \
  ghcr.io/digitalpals/fusebox:latest-nonroot
```

The default tags keep CLIProxyAPI's layout (root, `/CLIProxyAPI`), so they stay a drop-in.

## HTTPS with Tailscale, nginx or Caddy

Installing the dashboard as an app and push notifications need HTTPS. Either give Fusebox a certificate (`tls`, see [configuration](configuration.md)) or keep it on `127.0.0.1` and put an HTTPS proxy in front. **Behind any proxy, set a `management-key`:** a proxy on the same machine connects from `127.0.0.1`, which Fusebox would otherwise trust as local. Fusebox recognises Tailscale, nginx and Caddy by the `X-Forwarded-For` header they add, asks them for the key, and takes the client's address from that header. A proxy that forwards raw TCP adds nothing; give it `proxy-protocol-port` instead ([Source addresses](source-addresses.md)). Add clients too, unless everyone who can reach the proxy may use your accounts.

What Fusebox trusts by address alone (the dashboard without a `management-key`, the proxy without client keys on `127.0.0.1`) is refused to web pages: a request whose `Origin` names another site, or that arrives directly with a `Host` naming anything other than this machine, gets a 403, so a site you visit can't reach your accounts or settings. Tools on this machine, and pages served from `localhost`, are unaffected. Five wrong management keys lock that address out, from 30 seconds doubling up to 15 minutes; behind a proxy that names the client, that is the client's address, not the proxy's.

**Tailscale.** With [HTTPS certificates](https://tailscale.com/kb/1153/enabling-https) enabled for your tailnet, this serves Fusebox at `https://<machine>.<tailnet>.ts.net` to your tailnet only:

```sh
tailscale serve --bg 8317
```

If Fusebox itself serves HTTPS with its own certificate, point Tailscale at it with `tailscale serve --bg https+insecure://127.0.0.1:8317`. `tailscale funnel` would put the dashboard on the public internet; don't, unless you mean to.

**nginx.** Pass websocket upgrades (the dashboard's live view, Codex), don't buffer streamed responses, and allow long streams and large requests:

```nginx
server {
    listen 443 ssl;
    server_name fusebox.example.com;
    ssl_certificate     /etc/letsencrypt/live/fusebox.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/fusebox.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:8317;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_buffering off;
        proxy_read_timeout 600s;
        proxy_send_timeout 600s;
        client_max_body_size 256m;
    }
}

# in the http block:
map $http_upgrade $connection_upgrade { default upgrade; '' close; }
```

**Caddy** gets a certificate and handles all of that by itself:

```caddy
fusebox.example.com {
    reverse_proxy 127.0.0.1:8317
}
```

The dashboard sends its key in the live view's URL (browsers can't set headers on a websocket), so it can appear in the proxy's access log; keep that log private or turn it off for this site.

## Credentials and older installs

**Where are my credentials stored?** In `auth-dir`, one JSON file per account, replaced atomically and readable only by you (`0600`) from the moment it is written. On Windows, files take the permissions of their folder, so keep `auth-dir` inside your user profile, which other users can't read. It defaults to `~/.fusebox`; if that doesn't exist but `~/.cli-proxy-api` (CLIProxyAPI's directory, and this project's before it was renamed) does, that one is used and the server says so at startup. Setting `auth-dir` always wins. Nothing leaves your machine except requests to the providers you use.

**Claude sign-in fails or gets blocked.** Some Anthropic endpoints sit behind bot protection that CLIProxyAPI works around with a browser TLS fingerprint. Fusebox uses standard rustls. If token exchange fails for you, please open an issue with the error from the dashboard.

**I used this project when it was called CLIProxyAPI-Rust.** Everything keeps working. The old `CLIPROXYAPI_RUST_CONFIG` and `CLIPROXYAPI_RUST_DEFAULT_HOST` variables are still read (the new names are `FUSEBOX_CONFIG` and `FUSEBOX_DEFAULT_HOST`), the `x-cliproxy-*` session headers are still accepted, `~/.cli-proxy-api` is still found, and the dashboard moves its saved preferences over. Rename the binary in your scripts from `cliproxyapi-rust` to `fusebox`, and Codex's `env_key` to `FUSEBOX_KEY` if you like.
