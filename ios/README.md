# Fusebox for iPhone

A native SwiftUI app for a Fusebox server: every subscription's 5-hour and weekly meters with the
even-pace mark, each provider's combined weekly limit, faults, the request log with its routing
explanation, usage and costs, native push, Home and Lock Screen widgets and a Live Activity. It
reads the same management API as the web dashboard (`ui/app.js`, `ui/usage.js`, `ui/config.js`)
and uses its words. It runs on iPhone, including the foldable iPhone Duo, iOS 27.0 or later.

- Bundle `com.digitalpals.fusebox` (display name “Fusebox”; App Store name “Fusebox - Subscription
  Proxy”, app record 6829404456), widgets `com.digitalpals.fusebox.widgets`, notification service
  `com.digitalpals.fusebox.notifications`, team `4R8P9MTF9T`, automatic signing, version 1.2.0.
  1.0 and 1.1 are on the App Store: once App Review approves a version, App Store Connect takes no more
  builds of it, so each App Store release raises `MARKETING_VERSION` in `project.yml`.
- Keychain access group `<team>.com.digitalpals.fusebox.shared`, shared with both extensions:
  the management key (only ever in the Keychain), the server URL, the widgets' last snapshot, and
  the push relay's handle and key, which the notification service decrypts with.
  **No App Group**, a deviation from the original plan: App Store Connect API keys can't register
  an App Group, so with one the unattended publish fails (`Authentication failed …` for the
  `group.com.digitalpals.fusebox` capability, while the same archive without it signs). The keychain
  group does everything the widgets needed the group for. To add an App Group later, register it
  once at developer.apple.com (Identifiers → App Groups) and enable it on both App IDs, then add
  `com.apple.security.application-groups` back to both targets in project.yml.
- Swift 6 (complete concurrency checking), SwiftUI, Swift Charts, WidgetKit, ActivityKit, AppIntents.
  No third-party code.

## Layout

```
ios/
  project.yml              XcodeGen project (the .xcodeproj is generated and not committed)
  Shared/                  compiled into the app and the widget extension
    Models.swift           the API's JSON (lenient: every field the server may omit is optional),
                           and the push path (direct, relay, unavailable) from /api/push
    SharedKeychain.swift   the keychain access group (also in the notification service)
    SealedPush.swift       the relay device (handle, key) and opening sealed notifications
                           (also in the notification service)
    Formatting.swift       RFC 3339 with nanoseconds, and the dashboard's wording (fmt, left, compactLeft, …)
    QuotaRules.swift       quotaOf, windowOf, even pace, account state, plans, combined pools, routing text
    Connection.swift       URL/Keychain storage and the API client
    DeepLink.swift         notification `path` and fusebox:// routing
    Components.swift       the segmented meter (with the pace mark), provider logos, status dots
    Theme.swift            the design system: DESIGN.md's colour tokens, the three typefaces, and the
                           buttons, breaker switch and segmented control
    Fonts/                 IBM Plex Sans (variable), IBM Plex Sans Condensed, DM Mono, from ui/fonts
                           (woff2 unwrapped to TTF, unmodified), with their SIL OFL 1.1 licences
    OutageActivity.swift   Live Activity attributes and the widgets' snapshot file
  Fusebox/                 the app (App/, Model/, Views/, Assets.xcassets)
  FuseboxWidgets/          widgets and the Live Activity
  FuseboxNotifications/    the notification service: decrypts notifications from the push relay
  FuseboxTests/            unit tests (Swift Testing) and anonymised fixtures
  FuseboxUITests/          the UI tour, push and Live Activity checks that also take screenshots
  scripts/
    publish-testflight     archive, sign, upload, wait, assign to the TestFlight group
    capture-fixtures.mjs   refresh FuseboxTests/Fixtures from a server, anonymised
    make-assets.mjs        app icon, mark and provider logos from assets/ and ui/logos.svg
    make-colors.mjs        DESIGN.md's colour tokens as asset-catalogue colours (dark and light)
    ui-screenshots         run a UI test on a simulator and collect its screenshots
    relay-e2e.mjs          play a Fusebox's side of the push relay for a simulator (Debug)
```

## Design

The app follows the web dashboard's design system (`DESIGN.md` at the repository root; tokens
and sizes from `ui/style.css`), in dark and light:

- **Colour.** The tokens are asset-catalogue colours with a dark and a light appearance
  (`scripts/make-colors.mjs` writes them; `Palette` reads them): `bg`, `surface`, `surface-inset`,
  the `line` family, the `fg` ramp, `idle`, `meter`, `seg-on`, and the state colours `ok`/`warn`/`err`
  (darker in light mode). Colour is only for state (dots, short status words, meter segments) and
  provider logos. **No amber anywhere**: the tint, the tab selection, buttons and links are `fg`,
  and the mark is a one-colour template.
- **Type.** IBM Plex Sans for reading, IBM Plex Sans Condensed in tracked capitals for labels,
  section heads and page titles, DM Mono for data (times, ids, countdowns, model names). All
  three have tabular figures; every size scales with Dynamic Type (`relativeTo:`).
- **Surfaces.** Flat `surface` panels with a 1pt `line` hairline and 8pt corners, 16pt page
  margins, rows separated by hairlines. Outline buttons (`line-strong`, `fg` text), at most one
  primary (`fg` fill) per view, destructive actions in `err` text; the dashboard's segmented control
  and square-knobbed breaker switch (`meter` when on). Touch targets are at least 44pt.
- **Segmented meter.** 20 segments of 5% with 2pt gaps and 1pt corners, lit `meter` below 75%
  used, `warn` from 75%, `err` from 95% (always measured on used, in Left mode too), unlit `line`;
  9pt in lists, 20pt on the account page. The pace mark is a 2pt `fg-3` tick at the elapsed share
  of the window, drawn over the segments and 3–4pt past them above and below, mirrored in Left
  mode, with a 1pt gap in the panel's colour on each side so it reads over lit segments in both
  appearances. `MeterModel` and `Quotas.runsOut` hold the maths (`FuseboxTests/MeterTests.swift`).

## Build, run, test

```sh
cd ios
xcodegen generate                       # /opt/homebrew/bin/xcodegen
xcodebuild -project Fusebox.xcodeproj -scheme Fusebox \
  -destination 'platform=iOS Simulator,name=iPhone 18 Pro' build
xcodebuild -project Fusebox.xcodeproj -scheme Fusebox \
  -destination 'platform=iOS Simulator,name=iPhone 18 Pro' test      # unit tests
```

The unit tests decode fixtures captured from the production server (`scripts/capture-fixtures.mjs`
replaces emails, labels, client names, addresses, keys, endpoints and session ids, keeping ids
consistent across files, and refuses to write a file that still holds a key or a real email), and
cover the date parser, the dashboard's formatting and quota rules, live socket messages, old and new
`/api/push` shapes and the push path each one gives, opening sealed notifications (including the test
vector `the_sealed_format_matches_the_apps_test_vector` in `src/push/relay.rs` checks too), and
deep-link routing.

**Running against a server.** The app opens on Connect: the server URL (your Fusebox's HTTPS
address, for example its `tailscale serve` name) and the management key. It checks both
against `/api/overview` before saving and shows the server's error text when that fails.

For simulator runs, **Debug builds only** read `FUSEBOX_DEBUG_KEY_FILE` (a file holding the key;
the simulator reads the Mac's files) once into the Keychain, so the key never appears on a command
line or in a log. `FUSEBOX_DEBUG_URL` sets the URL, `FUSEBOX_DEBUG_RESET=1` forgets the key,
`FUSEBOX_DEBUG_REGIONS=1` outlines the reserved regions, `FUSEBOX_DEBUG_DIVISION=455-495[,active]`
adds a synthetic hinge (a vertical division region at those window x positions),
`FUSEBOX_DEBUG_OPEN=fusebox://…` starts on that page, `FUSEBOX_DEBUG_OUTAGE=claude` adds a
synthetic “every account used up” fault to try the Live Activity, and
`FUSEBOX_DEBUG_FORCE_RELAY=1` registers through the push relay (below) whatever the server offers,
without telling the server. None of this is compiled into Release builds.

```sh
SIMCTL_CHILD_FUSEBOX_DEBUG_KEY_FILE=~/.config/fusebox-ios/management-key \
  xcrun simctl launch <simulator> com.digitalpals.fusebox
```

**UI tests and screenshots.** `scripts/ui-screenshots <simulator> <prefix> [test] [display]` runs
`FuseboxUITests` against the server and saves PNGs to `/tmp/fusebox-shots`. On the iPhone Duo pass
display `3` (the inner panel): XCUIScreen only captures the main (outer) panel, so the test asks the
script to take the inner one with `simctl io screenshot --display=3`. The UI tests are tooling for
screenshots and end-to-end checks against a live server, not part of `xcodebuild test` on the
Fusebox scheme; `testPushRegistrationAndTestNotification` registers the simulator with the server,
so remove that device afterwards (Settings, or `DELETE /api/push/subscriptions/{id}`), then launch
the app once with `FUSEBOX_DEBUG_RESET=1` (it forgets the key and that notifications were wanted,
so the next launch doesn't register again).

## The iPhone Duo

The layout follows the size class and the space the scene gets, never the device model or screen
size, so folding, unfolding and resizing beside another app all go through the same path:

- **One layout, folded or unfolded.** A tab bar (Overview, Accounts, Requests, Usage, Settings) with
  a NavigationStack in each, and a faults button with its count in every header, as the dashboard's
  phone layout. iOS 27.1 puts the tab bar and the header buttons down the right edge on both the outer
  screen and the unfolded inner screen. `MainView` sets the scene's horizontal size class to compact,
  so an unfolded Duo (regular width) keeps the outer screen's tabs. Builds up to 1.0.0 (3) switched to a
  NavigationSplitView with a sidebar behind a menu button. Each page is one column at most
  `Metrics.column` (600pt) wide, centred on the inner screen (`pageContent()`). The page doesn't spread
  into two columns.
- **Keeping the place.** The section, the selected account and the selected request are
  `@SceneStorage` and drive the stacks' paths, so a fold or an unfold keeps the same account or request.
- **Clear of the fold.** iOS 27.1 really has the reserved-region API: SwiftUI's `ReservedRegion`
  with kinds `.division` and `.occlusion`, read through `GeometryProxy.reservedRegions(kind:options:)`
  (UIKit: `UIView.reservedRegions(of:)`), plus `ArrangementView` with `.automatic`, `.split` and
  `.overlay` styles. On the Duo simulator the inner screen (951 × 669 pt, landscape) reports a
  `.division` region at x = 455–495 pt, inactive while the phone lies flat, and an active occlusion
  for the status area. The app uses it in two places (all behind `#available(iOS 27.1, *)`; the
  deployment target stays 27.0):
  - `FoldAwareColumns` puts card columns on either side of the division instead of across it
    (always when it's active; when inactive, whenever the width gives two columns anyway), but
    only when a whole column fits on each side. In the unfolded Duo's centred 600pt column it
    doesn't, so pages stay one column there.
  - `clearOfFold()` insets a column's content when an *active* division runs along its leading or
    trailing part, so buttons such as Refresh and the on/off switch never sit on a hinge that
    content must divide at. The insets are never negative, at most a third of the width each, and
    dropped when less than 280pt would be left.
  - **Columns take the width offered now.** `FoldAwareColumns` places its columns with a `Layout`
    (`ColumnsLayout`) in exactly the proposed width; the measured geometry only chooses the number
    of columns and the hinge, and a hinge that doesn't fit the current width is ignored. Build 2
    sized columns from the last measured width, which could grow but never shrink, so when the
    window got narrower (the outer screen moving its bars to the side, a rotation to portrait) the
    page stayed wider than the screen and was cut off on the left.
    `testAccountDetailNarrowingKeepsContentOnScreen` (UI test) rotates an account page from
    landscape to portrait and checks its title, status, Refresh, switch and Limits panel lie inside
    the window.

  `ArrangementView` isn't used: the standard containers plus the division region cover these
  screens.
- **Folded and unfolded on the simulator.** A booted (or rebooted, or erased) iPhone Duo simulator
  starts *folded*: the outer screen is active (`xcrun devicectl device info displays --device <id>`
  shows display 1 `active`, display 3, the inner panel, not). On it iOS 27.1 puts the status bar,
  the navigation bar's buttons and the tab bar down the right edge, as on the device, and
  `scripts/ui-screenshots <duo> outer` runs the tour there (XCUIScreen captures the outer panel;
  XCUI doesn't report the side tab bar as a tab bar, so the tests find tabs by their buttons).
  Neither simctl nor devicectl has a posture command (`screenConfig power`, orientation, a fold
  notification and an erase don't change it). T3 Code's `agent-device fold open | closed` changes
  the pose through a HID hinge event (it may report that it can't read the angle back; check
  which display is `active`). DeviceHub's hinge slider does the same. Without either, a resizable
  session at the inner panel's size gives the same regular-width scene, and Debug builds take a
  synthetic hinge, because the session has none:

  ```sh
  SIMCTL_CHILD_FUSEBOX_DEBUG_KEY_FILE=… SIMCTL_CHILD_FUSEBOX_DEBUG_DIVISION=455-495 \
  SIMCTL_CHILD_FUSEBOX_DEBUG_OPEN='fusebox://accounts' xcrun simctl launch <duo> com.digitalpals.fusebox
  xcrun devicectl device appResize start --device <duo> --preferred-size 951x669 &
  xcrun simctl io <duo> screenshot --display=5 inner.png     # display 5 is the resizable one
  ```

## Live data

`/api/live` (the key in `?key=`, as the dashboard opens it) brings requests, account load, faults
and totals; an `accounts` message refetches accounts and the overview. While the socket is down
the app reconnects with backoff (1 s up to 15 s) and polls everything every 30 seconds; coming back
to the foreground it reconnects and reloads, as the dashboard does after a while away. The app badge
follows the fault count (faults with `badge: false`, pace warnings, don't count).

Faults have a ✕ (and a long-press **Dismiss until it clears**) that dismisses them on the server:
gone from every iPhone and dashboard, and from the badge, until the fault clears or a warning becomes
an error (`POST /api/faults/dismiss`, see `docs/notifications.md`). The `faults` message's
`dismissed` list, or `GET /api/faults/dismissed` while polling, gives the count under the list, with
**Show again** (`POST /api/faults/restore`).

## Push (APNs)

Settings, Notifications: on/off for this iPhone, how it's delivered (“Direct” or “Through the
Fusebox push relay (encrypted)”), Send test, and the other registered devices. Which events are
sent is the server-wide `notifications` config (edit it in the dashboard). docs/notifications.md
(“The iPhone app”) and docs/push-relay.md describe the server's side.

- **Which path.** `GET /api/push` returns `{"public_key", "subscriptions": [{"id","label","kind","via","created_at"}], "apns", "relay"}`.
  `apns: true`: register directly. Otherwise a `relay` URL: register through the relay. Otherwise
  (`relay` null, or an older server without the fields; a missing `kind` reads as `web`, a missing
  `apns` as false) Settings says this Fusebox can't send to the app: update Fusebox or set `apns`.
  Each registration reads the path again, so a server that changes is followed at the next launch;
  a registration on another path (or with a new handle) removes the old subscription.
- **Directly:** `POST /api/push/apns` `{"token": "<hex device token>", "environment":
  "production"|"development", "label": "<device name>"}` returns `{"id"}`; the same token again
  is the same device. Debug builds send `development`, Release (TestFlight, App Store)
  `production`, decided at compile time.
- **Through the relay:** `POST {relay}/v1/devices` `{"token", "environment"}` returns a
  `handle`; the app makes a 32-byte key (`SecRandomCopyBytes`), keeps relay, handle, key and
  token in the shared keychain group, and sends `POST /api/push/apns` `{"relay": {"handle",
  "key": "<base64>"}, "label"}`. At launch and in the foreground, at most every 6 hours, it
  touches the handle (`POST {relay}/v1/devices/{handle}/touch`); a 410 (forgotten) or a new APNs
  token gets a new handle and key, sent to the server again. Turning off: `DELETE
  /api/push/subscriptions/{id}`, then `DELETE {relay}/v1/devices/{handle}`.
- The app re-registers at launch when it's on, so a changed token reaches the server.
  `DELETE /api/push/subscriptions/{id}` turns this device (or another) off; `POST /api/push/test`
  `{"id"}` sends a test.
- **Direct payload:** `aps.alert.title/body`, `aps.badge` (fault count), `aps.sound`,
  `aps.thread-id`, plus top-level `path` (the dashboard's hash route), `kind`, `level`,
  `provider`, `account`. A tap on `#/accounts/<url-encoded id>` opens that account; anything else
  opens the Overview.
- **Relayed payload:** a generic alert (“Fusebox” / “Open Fusebox to see what happened.”),
  `mutable-content`, the badge, `thread-id: fusebox`, and top-level `sealed` and `v: 1`.
  `sealed` is base64 of `nonce(12) ‖ ciphertext ‖ tag(16)`, AES-256-GCM with the AAD
  `fusebox-push-v1`, of `{"title","body","path","kind","level","provider","account","thread"}`.
  The notification service (`FuseboxNotifications`) opens it with the stored key, shows the
  title and body, sets the thread, and puts `path`, `kind`, `level`, `provider` and `account` into
  userInfo, so a tap routes as a direct one does. Without the key, or when it doesn't open, the
  generic alert stays. Direct notifications pass through unchanged.

**Trying the relay on a simulator.** The production server has `apns` set, so the app sends there
directly. To exercise the relay anyway, Debug builds take `FUSEBOX_DEBUG_FORCE_RELAY=1`: they
register with `https://push.whombat.com` (development), skip `POST /api/push/apns`, and write the
handle and key to their own container (`Library/Caches/relay-debug.json`; never commit it). Then
`scripts/relay-e2e.mjs` plays the Fusebox, sealing as `src/push/relay.rs` does:

```sh
FUSEBOX_URL=https://<your fusebox> FORCE_RELAY=1 \
  scripts/ui-screenshots <sim> relay testRelayNotificationDecryptsAndOpensAccount &   # turns on, waits
node scripts/relay-e2e.mjs <sim> wait                     # until the app has its handle
node scripts/relay-e2e.mjs <sim> send '<account id>' 'Relay check'
# the test expects the decrypted banner, taps it, expects the account, and turns off (relay DELETE)
node scripts/relay-e2e.mjs <sim> cleanup                  # only if the test stopped early
node scripts/relay-e2e.mjs - health                       # "devices": 0
```

## Widgets and the Live Activity

- **Combined weekly limit** (small, medium, Lock Screen rectangular and circular): each provider's
  subscriptions together with the even-pace mark, the forecast's verdict and the next reset.
- **Subscription** (small, medium, Lock Screen rectangular), configurable with an AppIntent: one
  account's 5-hour and weekly meters.
- Both read the server with the shared key and fall back to the app's last snapshot (in the shared
  keychain group) when it can't be reached. They refresh about every 20 minutes, sooner when a reset or a
  cooldown ends first, and the app reloads them when faults change (otherwise at most every 5 min).
- **Live Activity:** when a provider's every account is out (the `provider` fault), the app starts
  one counting down to the first account back (`Text(timerInterval:)`), updates it while it runs and
  ends it (“Claude is back”) when the fault clears. Local updates only, no push-to-update; it can be
  turned off in Settings.

## Privacy

The App Store privacy policy URL is `PRIVACY.md` at the repository root
(https://github.com/DigitalPals/Fusebox/blob/main/PRIVACY.md). Keep it in step with what the
app, the push relay and the server store and send.

## Publishing to TestFlight

```sh
ios/scripts/publish-testflight                 # archive, upload, wait for processing, assign
ios/scripts/publish-testflight --check         # archive and sign only
ios/scripts/publish-testflight --setup         # keychain, group and testers only
ios/scripts/publish-testflight --build-number 7
ios/scripts/publish-testflight --assign 7      # only wait for build 7 and check it's in the group
```

It runs unattended on this Mac (over SSH too), modelled on Whombat's pipeline
(`~/Code/Whombat-publish/apps/mobile/scripts/publish-ios.mjs`, decision AP-5802):

1. Reads the App Store Connect API key (Admin) in place from `~/.config/whombat/ios/`
   (`asc.json` and `AuthKey_<keyId>.p8`; `FUSEBOX_ASC_DIR` overrides). John approved reusing it;
   it is never copied into this repository.
2. Makes sure the internal TestFlight group “Fusebox” exists (created through the API with
   `hasAccessToAllBuilds`) and every user of the team is a tester in it.
3. Build number: the highest build uploaded for the app plus 1 (asked of the API), or `--build-number`.
4. `xcodegen generate`, then `xcodebuild archive` (Release, `generic/platform=iOS`) with
   `-allowProvisioningUpdates` and the API key flags, so Xcode makes the profiles, registers the
   widget's App ID and its capabilities, and signs for the App Store in Apple's cloud.
5. Exports with `method: app-store-connect, destination: upload`, then checks the build every
   minute until App Store Connect has processed it (up to 90 minutes), and confirms through the API
   that it is in the “Fusebox” group (adding it if it isn't). Apple doesn't let a build's groups be
   read (`/v1/builds/{id}/betaGroups` is create/delete only), so it reads the group's builds.

The export needs every interface orientation in Info.plist (upside down too), because the app
supports multitasking.

**Signing keychain: Fusebox's own, not Whombat's.** The archive is signed with an Apple Development
identity in a dedicated `fusebox-build` keychain (`~/Library/Keychains/fusebox-build.keychain-db`;
its random password and the certificate's id, SHA-1 and expiry in `~/.config/fusebox-ios/`), made
through the API (`/usr/bin/openssl` for the CSR and the `.p12`, which `security import` can read)
and renewed a month before it expires. Reusing Whombat's `whombat-build` keychain read-only was
possible but not safe: Whombat's script deletes and recreates that keychain and revokes its
certificate when it renews, which would break a Fusebox run at any time, and both scripts would be
editing the keychain search list around the same keychain. With its own keychain Fusebox never
touches Whombat's keychain, certificate or profiles, and only ever revokes its own previous
certificate. The keychain is in the user's search list only while the archive is signed (Xcode
picks the identity from the list, and other SSH sessions would find it locked:
`errSecInternalComponent`), and the script refuses to start while a Whombat or Fusebox publish is
running, so the two never edit the list at once.
