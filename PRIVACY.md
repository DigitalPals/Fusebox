# Privacy

Last updated: 1 November 2026

This page covers the Fusebox iPhone app (“Fusebox - Subscription Proxy” on the App Store), the
Fusebox push relay at `push.whombat.com`, and the Fusebox server. In short: Fusebox is
self-hosted, the developer runs no service that receives your data except the push relay, and
the relay can't read your notifications.

## The iPhone app

**What it stores on your iPhone.** The address of your Fusebox server and its management key,
the quota display setting, your notification settings, the last data shown to the widgets, and,
if you turn on notifications through the relay, a relay handle and an encryption key. The key,
the address, the widgets' data and the relay details are kept in the iOS Keychain. Nothing is
sent to the developer.

**Who it talks to.**

- **Your Fusebox server**, at the address you enter. The app reads accounts, quotas, faults,
  requests and usage from it, and sends your actions (refresh, turn an account on or off, dismiss
  a fault, notification settings). That server is run by you or whoever gave you its key; its
  operator decides what it keeps.
- **Apple Push Notification service**, only if you allow notifications. iOS gives the app a
  device token. The app sends it either to your Fusebox server (direct delivery) or to the push
  relay (below). Your server also gets the name of your device, so you can tell your devices
  apart in its settings.
- **The Fusebox push relay**, only if notifications are on and your server sends them through
  the relay.

The app has no analytics, advertising, tracking, crash reporting or third-party code, and asks
for no account. It doesn't track you across apps or websites.

## The push relay

The relay lets any Fusebox server send notifications to the app without holding Apple's push
key. It is run by the developer at `https://push.whombat.com` behind Cloudflare.

- **What it keeps:** your device's push token, whether it is a development or production build,
  and a SHA-256 hash of each random handle it gave out for that token (one per Fusebox server),
  with when each was last used. A handle that has been neither used nor touched for 180 days is
  forgotten. When Apple reports a token as no longer valid, the relay deletes it.
- **What it can't read:** the notifications. Your Fusebox server encrypts each one with a key
  that only your iPhone and your server have (AES-256-GCM). The relay passes on a generic
  “Fusebox” alert, the badge count and the sealed message, which the app opens on your iPhone.
- **What it doesn't keep:** it logs neither tokens nor handles. It uses the caller's IP address
  only in memory, to limit how often each address can register devices and send notifications.
  Cloudflare, which carries the traffic, processes IP addresses as described in its own privacy
  policy.
- **Turning it off:** turning notifications off in the app's Settings deletes the device from
  your server and its handle from the relay.

## The Fusebox server

Fusebox is open-source software that you run yourself. It keeps its configuration, account
sign-ins, request statistics and usage history on the machine it runs on. It sends requests to
the AI providers you configure, and notifications to the destinations you set up (web push,
webhooks, the push relay). It sends nothing to the developer.

## Children

Fusebox is a tool for developers and isn't directed at children.

## Changes and contact

Changes to this page are made in this repository, so its history shows every change. Questions
about privacy: open an issue at
[github.com/DigitalPals/Fusebox/issues](https://github.com/DigitalPals/Fusebox/issues).
