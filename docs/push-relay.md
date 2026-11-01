# Running a push relay

The push relay lets the Fusebox iPhone app get notifications from any Fusebox, without each Fusebox holding the app's Apple push key. [Notifications](notifications.md#the-iphone-app) describes how a Fusebox and the app use it. The relay the app uses is `https://push.whombat.com`. You only need your own if you build the app under your own Apple developer account.

## What it does

The relay holds the app's APNs key and a list of devices. `POST /v1/devices` with `{"token", "environment"}` registers a device token and returns a random `handle`. A token can have up to 20 handles, one for each Fusebox the phone gets notifications from. `POST /v1/send` with `{"handle", "sealed", "badge", "collapse_id", "priority", "ttl"}` sends one notification. It has a generic alert, `mutable-content` (so the app's extension can decrypt it), the badge, and `sealed`, which is the notification the Fusebox encrypted for the device. The relay can't read `sealed` and never sees the key. `POST /v1/devices/{handle}/touch` keeps a handle while the app is used, and `DELETE /v1/devices/{handle}` removes it. `GET /healthz` reports that it runs and how many devices it knows.

It keeps device tokens and a SHA-256 hash of each handle in `relay.sqlite3` in its state directory. A handle that has been neither used nor touched for 180 days is forgotten. When Apple says a token is no longer valid, every handle for it is removed, and a Fusebox sending to one of them gets `410 Gone` and removes the device. Each address can register 30 devices an hour. Each handle can get 120 notifications an hour, and each address can send 1,200. A sealed notification can be up to 3,000 characters. It logs neither tokens nor handles.

## Running it

It is part of the Fusebox binary:

```sh
fusebox push-relay --team-id ABCDE12345 --key-id KEY1234567 \
  --key-file /etc/fusebox-push-relay/AuthKey_KEY1234567.p8 \
  --topic com.example.fusebox --listen 127.0.0.1:4750 --state-dir /var/lib/fusebox-push-relay
```

Every flag can also be set in the environment: `FUSEBOX_RELAY_TEAM_ID`, `FUSEBOX_RELAY_KEY_ID`, `FUSEBOX_RELAY_KEY_FILE`, `FUSEBOX_RELAY_TOPIC`, `FUSEBOX_RELAY_LISTEN` and `FUSEBOX_RELAY_STATE_DIR`. It signs a provider token at start and stops if the key can't be read. It serves plain HTTP, so put it behind something that adds HTTPS, such as a Cloudflare Tunnel, Caddy or nginx. Behind a local proxy it reads the caller's address from `CF-Connecting-IP` or `X-Real-IP`, and only trusts those headers on connections from loopback. A systemd unit can keep the key out of the service's reach until it starts:

```ini
[Service]
DynamicUser=yes
StateDirectory=fusebox-push-relay
LoadCredential=apns.p8:/etc/fusebox-push-relay/AuthKey_KEY1234567.p8
Environment=FUSEBOX_RELAY_KEY_FILE=%d/apns.p8
EnvironmentFile=/etc/fusebox-push-relay/relay.env
ExecStart=/usr/local/bin/fusebox push-relay
```

Point Fusebox servers at your relay with `push-relay: https://your-relay` in their `config.yaml`.
