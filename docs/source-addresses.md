# Source addresses

Fusebox records where each request came from: the Requests page shows it with the client
key, Config, Clients lists the addresses each named key was used from and what is
connected now, and a named key can be limited to some addresses. This page explains where
the address comes from, and when it can be trusted.

## Where the address comes from

| How the client reaches Fusebox | What Fusebox sees | Shown as |
|---|---|---|
| Directly | the connection's own address | `direct`, or `this machine` for 127.0.0.1 and ::1 |
| Through a reverse proxy in `trusted-proxies` (tailscale serve, nginx, Caddy) | the client in `X-Forwarded-For`, else `Forwarded`, else `X-Real-IP` | `via reverse proxy`, or `via Tailscale` with the Tailscale user when tailscale serve sends one |
| Through a TCP forwarder on `proxy-protocol-port` | the client in its PROXY protocol header | `via TCP forwarder` |

`trusted-proxies` lists the proxies whose word counts, as addresses or ranges. Unset, it is
this machine (127.0.0.0/8 and ::1); an empty list trusts no one. Forwarding headers from
anyone else are ignored, so a client can't claim another address by sending them. Lists are
read from the right, passing over trusted proxies, because proxies such as nginx add the
address they saw on the right of whatever the client sent. Of `X-Forwarded-For`,
`Forwarded` and `X-Real-IP`, the first one present decides. When nothing in it can be read,
the source is unknown: Fusebox shows the proxy's own address, marked as such, and address
lists refuse it rather than mistake the client for the proxy. A proxy that only sets
`X-Real-IP` should also drop the `X-Forwarded-For` and `Forwarded` headers clients send.

## TCP forwarders and `proxy-protocol-port`

A forwarder that passes TLS through untouched (HAProxy in TCP mode, a connection gate) can't
add headers, so behind it every client looks like the forwarder. Give it a port of its own:

```yaml
proxy-protocol-port: 8319   # same address as host; needs a restart
```

Every connection on that port must come from a trusted proxy and start with a PROXY protocol
header (version 1 or 2) naming the client; TLS, if on, follows the header. Anything else is
closed. The ports never mix: on the ordinary port a PROXY header is never looked for, so a
client behind the forwarder can't send its own.

A reverse proxy can sit behind the forwarder too (tailscale serve → forwarder → Fusebox): the
PROXY header then names the reverse proxy, which is trusted, and its `X-Forwarded-For` names
the client.

**With `proxy-protocol-port` on, forwarding headers count only on that port.** A raw forwarder
on this machine that connects to the ordinary port looks like this machine, so a client
behind it could otherwise send `X-Forwarded-For` and be believed. That happens when a
forwarder falls back to the ordinary port because the PROXY port refused it: during a
rollback to a Fusebox without the port, or in the instant a starting Fusebox binds its two
ports. So once the PROXY port is on, everything on the ordinary port is taken at its
connection address: tools on this machine show as this machine, and so does a client that
came through a fallback. Put reverse proxies behind the forwarder (as tailscale serve is in
front of a gate), or have them connect to the PROXY port themselves. The setting that counts
is the one Fusebox started with.

Through a fallback a client looks like this machine, so set a `management-key` (the
dashboard without one trusts this machine), and don't list 127.0.0.1 in the `addresses` of a
key that is used through the forwarder.

## What uses the address

- **Requests page:** each request's source and key; the filter matches addresses and
  Tailscale users. Pinned sessions on an account's page show them too.
- **Config, Clients:** each named key's addresses, with first and last use and a request
  count: the 20 most recent are shown, of up to 32 seen in the last half year and kept in
  `<auth-dir>/.client-addresses.state`,
  and **Connected now**: every key and address with requests in flight, open WebSockets, or a
  request in the last five minutes.
- **New-address notifications:** a named key used from an address it wasn't used from before,
  or refused by its address list, notifies devices and webhooks (`client-new-address`, on by
  default). A key's first address ever is not news. Each key and address notifies once while
  Fusebox runs, and ten notifications an hour at most.
- **Address lists:** `addresses` on a named client refuses its key (403) from anywhere else,
  on every request and every WebSocket turn.
- **Management key lockout:** five wrong keys lock out the source address, not the proxy in
  front, so one machine's mistakes don't lock out the others.
- **The dashboard without a key:** only this machine may open it. A client that reaches
  Fusebox through a TCP forwarder on the PROXY port is seen as itself, not as this machine.

Provider requests never carry the address: Fusebox drops forwarding headers and Tailscale's
`Tailscale-User-*` headers before forwarding a request.
