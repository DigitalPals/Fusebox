# Fusebox for iPhone

A native SwiftUI app for a Fusebox server: every subscription's 5-hour and weekly meters with the
even-pace mark, each provider's combined weekly limit, faults, the request log with its routing
explanation, usage and costs, native push, Home and Lock Screen widgets and a Live Activity. It
reads the same management API as the web dashboard (`ui/app.js`, `ui/usage.js`, `ui/config.js`)
and uses its words. It runs on iPhone, including the foldable iPhone Duo, iOS 27.0 or later.

- Bundle `com.digitalpals.fusebox` (display name “Fusebox”; App Store name “Fusebox - Subscription
  Proxy”, app record 6829404456), widgets `com.digitalpals.fusebox.widgets`, team `4R8P9MTF9T`,
  automatic signing, version 1.0.0.
- Keychain access group `<team>.com.digitalpals.fusebox.shared`, shared with the widget extension:
  the management key (only ever in the Keychain), the server URL and the widgets' last snapshot.
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
    Models.swift           the API's JSON (lenient: every field the server may omit is optional)
    Formatting.swift       RFC 3339 with nanoseconds, and the dashboard's wording (fmt, left, compactLeft, …)
    QuotaRules.swift       quotaOf, windowOf, even pace, account state, plans, combined pools, routing text
    Connection.swift       URL/Keychain storage and the API client
    DeepLink.swift         notification `path` and fusebox:// routing
    Components.swift       the segmented meter, provider logos, status dots
    OutageActivity.swift   Live Activity attributes and the widgets' snapshot file
  Fusebox/                 the app (App/, Model/, Views/, Assets.xcassets)
  FuseboxWidgets/          widgets and the Live Activity
  FuseboxTests/            unit tests (Swift Testing) and anonymised fixtures
  FuseboxUITests/          the UI tour, push and Live Activity checks that also take screenshots
  scripts/
    publish-testflight     archive, sign, upload, wait, assign to the TestFlight group
    capture-fixtures.mjs   refresh FuseboxTests/Fixtures from a server, anonymised
    make-assets.mjs        app icon, mark and provider logos from assets/ and ui/logos.svg
    ui-screenshots         run a UI test on a simulator and collect its screenshots
```

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
`/api/push` shapes, and deep-link routing.

**Running against a server.** The app opens on Connect: the server URL (your Fusebox's HTTPS
address, for example its `tailscale serve` name) and the management key. It checks both
against `/api/overview` before saving and shows the server's error text when that fails.

For simulator runs, **Debug builds only** read `FUSEBOX_DEBUG_KEY_FILE` (a file holding the key;
the simulator reads the Mac's files) once into the Keychain, so the key never appears on a command
line or in a log. `FUSEBOX_DEBUG_URL` sets the URL, `FUSEBOX_DEBUG_RESET=1` forgets the key,
`FUSEBOX_DEBUG_REGIONS=1` outlines the reserved regions, `FUSEBOX_DEBUG_OUTAGE=claude` adds a
synthetic “every account used up” fault to try the Live Activity. None of this is compiled into
Release builds.

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
so remove that device afterwards (Settings, or `DELETE /api/push/subscriptions/{id}`).

## The iPhone Duo

The layout follows the size class and the space the scene gets, never the device model or screen
size, so folding, unfolding and resizing beside another app all go through the same path:

- **Compact width** (folded, on the 5.4" outer screen, or narrow beside another app): a tab bar
  (Overview, Accounts, Requests, Usage, Settings) with a NavigationStack in each, and a faults button
  with its count in every header, as the dashboard's phone layout.
- **Regular width** (unfolded, 7.6" inner screen): a NavigationSplitView with the sections in a
  sidebar. Accounts and Faults are three columns (list and account detail); the request log is the
  detail column with the selected request in an inspector. Overview cards reflow into columns.
- **Keeping the place.** The section, the selected account and the selected request are
  `@SceneStorage`. In compact width they drive the stacks' paths, in regular width the split view's
  selections, so a fold or an unfold shows the same account or request in the other layout.
- **Clear of the fold.** iOS 27.1 really has the reserved-region API: SwiftUI's `ReservedRegion`
  with kinds `.division` and `.occlusion`, read through `GeometryProxy.reservedRegions(kind:options:)`
  (UIKit: `UIView.reservedRegions(of:)`), plus `ArrangementView` with `.automatic`, `.split` and
  `.overlay` styles. On the Duo simulator the inner screen (951 × 669 pt, landscape) reports a
  `.division` region at x = 455–495 pt, inactive while the phone lies flat, and an active occlusion
  for the status area. The app uses it in three places (all behind `#available(iOS 27.1, *)`; the
  deployment target stays 27.0):
  - `FoldAwareColumns` puts card columns on either side of the division instead of across it
    (always when it's active; when inactive, whenever the width gives two columns anyway).
  - In the three-column layout the list column's width is capped at the division's leading edge.
  - `clearOfFold()` insets a column's content when the division runs along its leading or trailing
    part (the account detail, the request log, Usage, Settings), so buttons such as Refresh quota
    and the on/off switch never sit on the hinge. The request inspector takes the page beyond it.

  `ArrangementView` isn't used: the standard containers plus the division region cover these
  screens, and its split style would duplicate what NavigationSplitView already does.
- What the simulator can't do: the Duo simulator can't be folded from the command line (neither
  simctl nor devicectl has a posture command, and `devicectl device appResize` refuses the inner
  panel's orientation), so the folded layout was checked through the same compact code path on the
  iPhone 18 Pro.

## Live data

`/api/live` (the key in `?key=`, as the dashboard opens it) brings requests, account load, faults
and totals; an `accounts` message refetches accounts and the overview. While the socket is down
the app reconnects with backoff (1 s up to 15 s) and polls everything every 30 seconds; coming back
to the foreground it reconnects and reloads, as the dashboard does after a while away. The app badge
follows the fault count (faults with `badge: false`, pace warnings, don't count).

## Push (APNs)

Settings, Notifications: on/off for this iPhone, Send test, and the other registered devices.
Which events are sent is the server-wide `notifications` config (edit it in the dashboard).

- `POST /api/push/apns` `{"token": "<hex device token>", "environment": "production"|"development",
  "label": "<device name>"}` returns `{"id"}`; registering the same token again updates it. Debug
  builds send `development`, Release (TestFlight, App Store) `production`, decided at compile time.
  The app re-registers at launch when it's on, so a changed token reaches the server.
- A server without APNs answers 503 `{"error": "..."}` (older builds 404); Settings shows the text,
  nothing more happens.
- `GET /api/push` returns `{"public_key", "subscriptions": [{"id","label","kind","created_at"}], "apns"}`;
  a missing `kind` reads as `web`, a missing `apns` as false.
- `DELETE /api/push/subscriptions/{id}` turns this device (or another) off; `POST /api/push/test`
  `{"id"}` sends a test.
- The payload: `aps.alert.title/body`, `aps.badge` (fault count), `aps.sound`, `aps.thread-id`,
  plus top-level `path` (the dashboard's hash route), `kind`, `level`, `provider`, `account`.
  A tap on `#/accounts/<url-encoded id>` opens that account; anything else opens the Overview.

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
