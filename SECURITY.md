# Security

Fusebox holds sign-ins for paid accounts, so security reports are welcome and handled first.

## Reporting a vulnerability

Report it privately through GitHub: **Security → Report a vulnerability** on
[DigitalPals/Fusebox](https://github.com/DigitalPals/Fusebox/security/advisories/new).
Please don't open a public issue. Include the version (`fusebox --version`), how Fusebox
was run (binary, Docker, behind a proxy) and the steps that show the problem. You will get
an answer within a few days.

## What matters most

- Credentials: sign-in files in `auth-dir`, keys in `config.yaml`, and anything that could
  send them somewhere other than their provider.
- Access: using the proxy or the dashboard without the configured key, or from a web page.
- Requests: one client reaching another's sessions, history or usage.

## Hardening checklist

- Give each client its own key (`named-clients`, or Config, Clients) and set `management-key`
  whenever anything other than this machine can reach the port, and always behind a reverse
  proxy. A key per client can be replaced or removed without touching the others.
- Behind a TCP forwarder that passes TLS through, give it `proxy-protocol-port` so Fusebox
  sees each client's address, and keep that port reachable only from the forwarder. With
  it on, forwarding headers count only on that port, so a forwarder that falls back to the
  ordinary port shows its clients as this machine: keep a `management-key` set.
  `trusted-proxies` lists whose forwarding headers count (this machine by default).
- An `addresses` list on a named client refuses its key from anywhere else. It is only as
  good as the address Fusebox sees: see `docs/source-addresses.md`.
- Keep `auth-dir` and `config.yaml` readable only by the user Fusebox runs as. On Windows,
  keep them inside your user profile, whose permissions already exclude other users.
- Prefer the `-nonroot` Docker image when you don't need CLIProxyAPI's exact layout.
