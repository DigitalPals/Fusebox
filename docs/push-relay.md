# Running a push relay

The push relay lets the Fusebox iPhone and Android apps get notifications from any Fusebox, without each Fusebox holding the app's Apple APNs key or Firebase service account key. [Notifications](notifications.md#the-iphone-app) describes how a Fusebox and the apps use it. The relay the apps use is `https://push.whombat.com`. You only need your own if you build an app under your own Apple developer account or Firebase project.

## What it does

The relay holds the app's APNs key, its Firebase service account key, or both, and a list of devices. `POST /v1/devices` with `{"token", "platform": "apns" | "fcm", "environment"}` (`platform` defaults to `"apns"`; `environment` is ignored for `"fcm"`) registers a device token and returns a random `handle`. A token can have up to 20 handles, one for each Fusebox the phone gets notifications from. `POST /v1/send` with `{"handle", "sealed", "badge", "collapse_id", "priority", "ttl"}` sends one notification. For APNs it sends a generic alert, `mutable-content` (so the iOS extension can decrypt it), the badge, and `sealed`. For FCM it sends a data-only message carrying `sealed`, `v: "1"`, `badge` and `collapse_id` (so the Android messaging service decrypts it locally). The relay can't read `sealed` and never sees the key. `POST /v1/devices/{handle}/touch` keeps a handle while the app is used, and `DELETE /v1/devices/{handle}` removes it. `GET /healthz` reports that it runs, which platforms are configured (`apns`, `fcm`), and how many devices it knows.

It keeps device tokens, their platform, and a SHA-256 hash of each handle in `relay.sqlite3` in its state directory. A handle that has been neither used nor touched for 180 days is forgotten. When Apple or Google says a token is no longer valid, every handle for it is removed, and a Fusebox sending to one of them gets `410 Gone` and removes the device. Each address can register 30 devices an hour. Each handle can get 120 notifications an hour, and each address can send 1,200. A sealed notification can be up to 3,000 characters. It logs neither tokens nor handles.

## Running it

It is part of the Fusebox binary. Supply APNs flags, `--fcm-service-account-file`, or both:

```sh
fusebox push-relay --team-id ABCDE12345 --key-id KEY1234567 \
  --key-file /etc/fusebox-push-relay/AuthKey_KEY1234567.p8 \
  --topic com.example.fusebox \
  --fcm-service-account-file /etc/fusebox-push-relay/firebase-service-account.json \
  --listen 127.0.0.1:4750 --state-dir /var/lib/fusebox-push-relay
```

Every flag can also be set in the environment: `FUSEBOX_RELAY_TEAM_ID`, `FUSEBOX_RELAY_KEY_ID`, `FUSEBOX_RELAY_KEY_FILE`, `FUSEBOX_RELAY_TOPIC`, `FUSEBOX_RELAY_FCM_SERVICE_ACCOUNT_FILE`, `FUSEBOX_RELAY_LISTEN` and `FUSEBOX_RELAY_STATE_DIR`. It signs an APNs token and validates the FCM service account at start and stops if a configured key can't be read. It serves plain HTTP, so put it behind something that adds HTTPS, such as a Cloudflare Tunnel, Caddy or nginx. Behind a local proxy it reads the caller's address from `CF-Connecting-IP` or `X-Real-IP`, and only trusts those headers on connections from loopback. A systemd unit can keep the keys out of the service's reach until it starts:

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
