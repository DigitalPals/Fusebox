# Notifications and the installed app

The dashboard can be installed as an app and send push notifications when something trips, so you hear about an expired sign-in or a provider that has run out without keeping a tab open.

## What you need

- **HTTPS**, or the dashboard on `localhost`. Browsers only allow service workers and push in a secure context. On a server, use Fusebox's own `tls` setting or a proxy such as `tailscale serve`, nginx or Caddy; see [HTTPS with Tailscale, nginx or Caddy](../README.md#https-with-tailscale-nginx-or-caddy). A self-signed certificate only works on devices that trust it.
- **A `management-key`** whenever a proxy is in front of Fusebox.
- **On iPhone and iPad** (iOS 16.4 or later), the dashboard added to the Home Screen: in Safari tap **Share**, then **Add to Home Screen**, and open Fusebox from its icon. Safari tabs can't receive push notifications on iOS. The installed app keeps its own storage, so it asks for the management key once.
- **Elsewhere**, any current Chrome, Edge, Firefox or Safari. Installing is optional on Android and desktops, but the app gets its own window and a fault count on its icon.

## Turning them on

Open **Config, Notifications** on the device that should get them and click **Turn on**, then allow notifications when the browser asks. **Send test** checks the whole path. Each device is turned on separately; the section lists the others, and **Remove** stops notifications to one of them.

The events are shared by every device and saved in `config.yaml`:

```yaml
notifications:
  sign-in-expired: true      # an account needs signing in again
  provider-exhausted: true   # every account of a provider is out, and when one is back
  account-used-up: false     # one account used up its 5-hour or weekly limit
  account-errors: false      # account errors, and three or more failed requests in an hour
  banked-resets: true        # a new reset, one used automatically, needing review, or expired unused
  renewals: true             # two days before a ChatGPT subscription charges again, or ends
```

## What is sent, and when

Fusebox checks every 30 seconds, using the rules behind the dashboard's faults button. A fault must still be there at the next check before it is sent, so a blip doesn't notify, and each fault is sent once until it clears. Faults that were already there when notifications were turned on aren't sent. Restarting Fusebox doesn't send anything again.

- **Sign-in expired:** "Claude sign-in expired", with the account. Opens its page.
- **Provider out of capacity:** "Every Claude account is used up" with when the first one is back, or "No Claude account can take requests" when sign-ins have expired too. When a provider has run out, its accounts' own limit notifications are left out. "Claude is back" follows when an account can take requests again.
- **Account limit used up:** "work@example.com: weekly limit used up · back in 3d 7h".
- **Account errors:** the account's error, or "3 failed requests" in the last hour.
- **Banked resets:** sent when observed, independently of the 30-second check. "New Claude banked reset" or "New Codex banked reset" when an account receives resets, with the number received, total banked and any reported expiry. "Codex reset used" when automatic use spends one; "Codex reset needs review" when an automatic spend got no clear answer; "Codex reset expired unused" within about a minute of a reset expiring, with the provider's last refusal if there was one. A refusal alone isn't sent: during a reset's final hour automatic use retries every five minutes. Resets you use yourself aren't sent either. Opens the account's page.

- **Subscription renewals:** sent by the 30-second check, two days before a ChatGPT subscription charges again: "Codex subscription renews on Friday", with the date and time ChatGPT gives. A cancelled subscription sends "Codex subscription ends tomorrow" instead. Each date is sent once, also across restarts, and also for disabled accounts, which are still charged. Opens the account's page, whose sign-in card shows the next date as **Renews** (or **Ends**). Claude accounts get none: Claude's profile only says when the subscription started, plan changes move the billing day, and its billing endpoints don't accept the sign-in's token.

New-reset detection requires **Banked resets** under Config, Connections. It uses the existing inventory checks every 30 minutes and manual refreshes, so a new reset normally notifies within about 30 minutes while the provider is reachable. The first successful inventory read for a subscription establishes a quiet baseline. Receipt history is saved with the reset journal, shared by duplicate credential files and preserved across restarts. Failed reads and changes in whether a reset is usable do not create receipt notifications. Observations continue while the notification switch is off or no device/webhook is listening, so turning notifications on does not announce resets already observed.

Grant identities and allocation increases detect new resets even when another reset is spent between reads. Codex sometimes returns only a count or partial credit details; Fusebox detects net count increases and avoids announcing those credits again when their details arrive. With count-only data, simultaneous receipt and spending can hide each other. A reset received and spent entirely between checks can also go unseen. Delivery uses the same best-effort push and webhook behavior as other events.

Rate limits are never sent; they come and go too quickly. The app's icon shows the number of faults the dashboard shows.

## The iPhone app

The native Fusebox app for iPhone gets the same notifications. The app turns them on for its device under **Settings, Notifications**, and the dashboard lists it among the other devices as "iPhone app". The events chosen in the dashboard apply to it as well.

Apple delivers notifications for an app only from a server holding a push key of the developer account the app is signed with. So the app's notifications reach your phone in one of two ways:

- **Through the push relay (the default).** The app registers with the Fusebox push relay at `https://push.whombat.com` and gets back a handle. It gives your Fusebox that handle and a key it made itself. Fusebox encrypts each notification with that key (AES-256-GCM) and gives the relay only the handle and the ciphertext. The relay passes them to Apple, and the app decrypts them on the phone. So the relay and Apple see that a notification was sent and how big it is, but not what it says, and your Fusebox never learns the phone's device token. Nothing needs setting up. To use another relay, set `push-relay: https://…` in `config.yaml`; `push-relay: ""` turns it off. [Running a relay](push-relay.md) describes the relay itself.
- **Directly, with the app's push key.** This is for whoever signs the app, or anyone who builds it with their own Apple developer account and bundle ID:

  ```yaml
  apns:
    team-id: ABCDE12345                          # the developer account's team ID
    key-id: KEY1234567                           # the key's ID, also in its file name
    key-file: /etc/fusebox/AuthKey_KEY1234567.p8 # readable by Fusebox only
    topic: com.digitalpals.fusebox               # the app's bundle ID; this is the default
  ```

  Fusebox reads the key when it first sends and signs a new token for Apple every 50 minutes, so a replaced key file is picked up within the hour, and a changed `apns` setting at once. When `apns` is set, the app registers directly instead of through the relay.

A device that Apple or the relay no longer knows is removed, the same way a browser's expired subscription is: for example, when the app was removed or registered against the other push environment. Opening the app registers it again.

The app registers with `POST /api/push/apns`. Directly, it sends `{"token": "<hex>", "environment": "production" | "development", "label": "<device name>"}`; App Store and TestFlight builds use `production`. Through the relay, it sends `{"relay": {"handle": "<from the relay>", "key": "<32 bytes, base64>"}, "label": "<device name>"}`. Either way it gets back the subscription's `id`. `GET /api/push` lists each device with `"kind": "web"` or `"apns"` and, for app devices, `"via": "direct"` or `"relay"`. It also says whether `apns` is set up and which `relay` URL is in use (or `null`). Removing a device and **Send test** work as they do for browsers. A direct notification carries its alert, the fault count as the badge, and top-level `path` (where it opens in the dashboard, which the app maps onto its own screens), `kind`, `level`, `provider` and `account`. A relayed one carries the badge and a generic alert, plus `sealed`, which holds the same fields encrypted.

## Chat and webhooks

The same notifications can go to chat. Under **Config, Notifications**, paste the URL a service gives you into **Chat and webhooks** and press **Add**; **Send test** sends a test message at once, saved or not. The events chosen below apply to devices and webhooks alike, and the list is saved in `config.yaml`:

```yaml
notifications:
  webhooks:
    - https://discord.com/api/webhooks/123/abc
    - https://api.telegram.org/bot123:abc/sendMessage?chat_id=-1001234567890
```

The URL says how a message is sent:

| URL | Sent as |
| --- | --- |
| `discord.com/api/webhooks/…` | A Discord embed |
| `hooks.slack.com/…` | Slack text |
| `*.webhook.office.com`, `*.logic.azure.com`, Power Automate | A Teams Adaptive Card (Workflows) |
| `api.telegram.org/bot<token>/sendMessage?chat_id=<chat>` | A Telegram message |
| a host named `ntfy` (`ntfy.sh/<topic>`, `ntfy.example.com/<topic>`) | An ntfy notification with title, tags and priority |
| anything else | JSON with `text` (which Mattermost, Google Chat and Rocket.Chat show as it is) plus `title`, `body`, `event`, `kind`, `level`, `provider`, `account`, `label` and `until` |

`event` is `sign_in_expired`, `provider_exhausted`, `account_used_up`, `account_errors`, `client_new_address`, `banked_reset`, `subscription_renewal`, `recovered` or `test`; a newly received or used reset has `level` `ok`. A destination is named by its kind and host only, in the dashboard and in logs; the rest of the URL is usually its secret, so keep `config.yaml` private. Webhooks are sent once per fault, like push notifications, and a failed delivery is logged, not retried.

## Reading faults from other tools

Desktop widgets and scripts can read the same faults through the management API, with the management key as a bearer token, like every other `/api` route:

- `GET /api/faults` lists them, errors first, without the dismissed ones.
- `/api/live` sends a `faults` event with the whole list as soon as a socket connects, then again whenever the list changes. The dashboard's `load` events work the same way. Its `dismissed` field lists the dismissed faults that haven't cleared.

### Dismissing faults

The ✕ beside a fault in the dashboard's faults menu, or in the iPhone app's Faults list, dismisses it everywhere: on every dashboard and every iPhone, and from the app badge. It stays hidden until it clears (gone for a minute, so a fault that flickers stays dismissed) or until a warning becomes an error. If it trips again after that, it shows again. Dismissing hides a fault from the lists only. Notifications already sent stay sent, and `fusebox_faults` in `/metrics` still counts it. Dismissals are kept in `<auth-dir>/.web-push.state` and survive a restart. The menu's **Show again** brings every dismissed fault back.

- `POST /api/faults/dismiss` with `{"key": "<fault key>"}` dismisses a current fault (404 when it has already cleared).
- `POST /api/faults/restore` with `{"key": "<fault key>"}` shows one fault again; `{}` shows them all.
- `GET /api/faults/dismissed` lists the dismissed faults that haven't cleared.

```json
{
  "key": "quota:file:claude-work.json",
  "kind": "quota",
  "level": "warn",
  "provider": "claude",
  "provider_name": "Claude",
  "account_id": "file:claude-work.json",
  "label": "work@example.com",
  "title": "Weekly limit used up",
  "detail": null,
  "until": "2026-10-12T09:00:00+00:00",
  "path": "#/accounts/file%3Aclaude-work.json"
}
```

The same socket's `load` event says what each busy account is doing: `in_flight` requests, `sessions` seen in the last five minutes (the dashboard's live sessions, and what smart-quota routing weighs), and `ongoing_sessions` seen in the last 30 minutes, which still counts a session waiting on its user or a long tool run. Each counts calls in flight, and no window outlasts `session-affinity-idle-seconds`.

`kind` is `signin`, `quota`, `rate_limit`, `error`, `failures`, `pace` or `provider`. A `pace` fault says a subscription window would run out before it renews at its pace so far, and its `detail` names the client that used most of it ([who used a window](usage-costs.md#who-used-a-subscription-window)); it is never pushed and has `"badge": false`, so it does not count on the app badge. A provider with two or more enabled subscriptions gets no `pace` fault for any one of them: smart-quota routing runs down the account that renews first on purpose. It gets one per window for all of them instead (`pace:pool:<provider>:<window>`, or `pace:pool:<provider>:<window>:<model>` for a model's own limit, with no `account_id` or `label`), raised only when their combined remaining quota, at their combined pace so far, runs out before renewals bring capacity back. Its `until` is the renewal that does, and its `detail` names the client that used most of the windows. Whole-account weekly warnings use the forecast of the overview's [Combined weekly quota](weekly-outlook.md), including recent consumption and all scheduled resets over the coming seven days; insufficient or stale data produces no forecast warning. Other windows use their pace so far. The accounts count as on the outlook: by weekly plan allowance for Claude (Pro 1, Max 5, Ultra 10) when every plan is known, equally otherwise. A model's own limit is pooled only when every subscription has one, since an account without one takes that model's requests under its other limits. For the same reason there is no five-hour pool while any member without a five-hour limit (a verified Claude Ultra account) can take the overflow: signed in, not paused as a whole and with weekly quota left. Otherwise the pool is the members that have the limit. `level` is `err` while requests fail and `warn` while an account sits out for a while. `title` and `detail` never contain a countdown: `until` is when the fault should clear by itself, so a client counts down on its own and a fault only changes when something does. A whole provider (`kind: "provider"`), like a provider's pace warning, has no `account_id` or `label`. `path` is where the fault opens in the dashboard. Unlike notifications, the list includes rate limits and doesn't wait for a second check.

## Privacy

Notifications go from Fusebox to your browser's push service (Google for Chrome and Edge, Mozilla for Firefox, Apple for Safari), which delivers them to the device. The message is end-to-end encrypted ([RFC 8291](https://www.rfc-editor.org/rfc/rfc8291)); the push service sees only when a message is sent and how large it is. Nothing is sent unless a device has turned notifications on.

Fusebox keeps its push key (VAPID), the subscribed devices and the faults it has sent in `.web-push.state` in the credentials directory (`auth-dir`), readable only by its owner. Deleting the file signs every device out of notifications; they turn them on again from the dashboard.

Notifications for the iPhone app sent directly go to Apple's push service as plain JSON over HTTPS, which Apple can read, like any app's notifications. Through the relay they are encrypted for the device, so neither the relay nor Apple can read them.

Webhook messages go straight to the URLs you add, as plain HTTPS requests that service can read.

## Troubleshooting

- **"Notifications need HTTPS"**: open the dashboard over HTTPS, not `http://<ip>:8317`.
- **"Add Fusebox to your Home Screen first"**: on iOS, push only works in the installed app.
- **"Notifications are blocked for this site"**: allow them in the browser's site settings (on iOS: Settings, Notifications, Fusebox), then reload.
- **The test arrives but faults don't**: check that the event is switched on, and that the fault is still there after 30 seconds. Fusebox logs each notification it sends (`push sent`) and each one a push service refuses.
- **A device stopped getting them**: browsers sometimes replace a subscription; the push service then tells Fusebox the old one is gone and it is removed. Turn notifications on again on that device.
- **A proxy in front, and the dashboard says it is local-only**: set `management-key`; Fusebox no longer treats proxied requests as coming from `localhost`.
