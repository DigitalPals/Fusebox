# Fusebox for Android

A native Kotlin + Jetpack Compose app for a Fusebox server, built for phones and foldables
(Android 12+, API 31–36): every subscription's 5-hour and weekly meters with the even-pace mark,
each provider's combined weekly limit, faults, the live request log with its routing explanation,
usage and costs with interactive charts, native FCM push (direct or encrypted via the push relay),
Glance home screen widgets, and Android 16 Live Updates for provider outages.

It reads the same management API as the web dashboard (`ui/app.js`, `ui/usage.js`, `ui/config.js`)
and matches the iOS app's data rules and `DESIGN.md` design system.

- **Application ID:** `com.digitalpals.fusebox` (matches the iOS bundle ID)
- **Min / Target / Compile SDK:** 31 (Android 12) / 36 (Android 16) / 36
- **Stack:** Kotlin 2.2, Jetpack Compose + Material 3, Jetpack WindowManager (`FoldingFeature`),
  Jetpack Glance widgets, WorkManager, OkHttp (HTTP + WebSocket), Kotlinx Serialization,
  Firebase Cloud Messaging (optional at build time — sideloads build without `google-services.json`).
- **Security:** Management key, server URL and relay encryption key are encrypted at rest with an
  Android Keystore AES-256-GCM key and excluded from cloud backups (`allowBackup="false"`).

## Foldable layout (Option A — Adaptive)

The layout follows window size and physical `FoldingFeature` geometry from Jetpack WindowManager
(`ui/adaptive/FoldableLayout.kt`), never device model names:

- **Folded / cover screen / standard phone (`< 600dp`):** Bottom navigation bar (Overview, Accounts,
  Requests, Usage, Settings), single-column screens, full-screen detail navigation for Accounts and
  Requests.
- **Unfolded foldable / tablet (`>= 600dp`):**
  - Left **Navigation Rail** with the Fusebox mark, section icons, and Faults badge.
  - **Accounts** and **Requests** switch to a hinge-aware **2-pane List-Detail** split (`FoldableListDetail`),
    placing the list left of the vertical fold and the detail view right of the fold so nothing sits
    across the crease.
  - **Overview** and **Usage** spread into hinge-aware **2-column grids** (`FoldAwareColumns`),
    keeping panels on either side of a vertical fold.
- **Tabletop / half-folded posture:** Detected via `FoldingFeature.State.HALF_OPENED` and
  `Orientation.HORIZONTAL` (`TabletopAwareSplit`).
- **Continuity across folds:** Active tab, selected account ID, selected request ID, usage range,
  and filters are preserved with `rememberSaveable` across fold/unfold/rotation without reloading.

## Design

Matches `DESIGN.md` and the iOS app in dark and light themes:

- **Colour:** `bg`, `surface`, `surfaceInset`, `line`/`lineStrong`, `fg`–`fg4`, `meter`, `segOn`,
  and state colours `ok`/`warn`/`err` (darker in light mode). **No amber anywhere in the UI** —
  accent, tab selection, buttons and links use `fg`; amber is only on the launcher icon.
- **Type:** Bundled IBM Plex Sans, IBM Plex Sans Condensed (tracked caps headers), and DM Mono
  (tabular figures `tnum` everywhere). All clock times use 24-hour format (`HH:mm`, `HH:mm:ss`).
- **Segmented meter:** 20 segments of 5%, lit `meter` (< 75% used), `warn` (>= 75%), `err` (>= 95%),
  with the even-pace tick drawn over the segments with a knockout gap on each side.

## Build, test, and package

Requires JDK 21 and Android SDK (API 36).

```sh
cd android
./gradlew testDebugUnitTest             # JVM unit tests (uses shared ios/FuseboxTests/Fixtures)
./gradlew assembleDebug                 # Debug APK for emulator / local device
./gradlew assembleRelease               # Release APK for sideloading
./gradlew bundleRelease                 # Release AAB for Google Play
```

Or use the convenience build script:

```sh
android/scripts/build-release                    # unit tests + release APK (sideload) + AAB (Play)
android/scripts/build-release --apk-only         # sideload APK only
android/scripts/build-release --version 1.0.0 --code 2
```

Outputs are written to `android/app/build/outputs/apk/release/` (sideload APK) and
`android/app/build/outputs/bundle/release/` (Play Store AAB).

### Re-generating design assets

Fonts, colours, launcher icon, mark, and all 18 provider vector drawables are generated from
`DESIGN.md`, `assets/icon.svg`, `ui/logos.svg`, and `ios/Shared/Fonts/`:

```sh
node android/scripts/make-assets.mjs
```

### Release signing (for sideloading signed APKs & Google Play)

Create `~/.config/fusebox-android/signing.properties` (never committed):

```properties
storeFile=/home/john/.config/fusebox-android/fusebox-upload.jks
storePassword=...
keyAlias=fusebox
keyPassword=...
```

Or pass `FUSEBOX_KEYSTORE_FILE`, `FUSEBOX_KEYSTORE_PASSWORD`, `FUSEBOX_KEY_ALIAS`,
`FUSEBOX_KEY_PASSWORD` in the environment. When signing is not configured, `assembleRelease` and
`bundleRelease` still succeed and produce unsigned artifacts (`app-release-unsigned.apk` /
`app-release.aab`).

To create a new upload keystore once:

```sh
mkdir -p ~/.config/fusebox-android && chmod 700 ~/.config/fusebox-android
keytool -genkeypair -v -keystore ~/.config/fusebox-android/fusebox-upload.jks \
  -keyalg RSA -keysize 4096 -validity 10000 -alias fusebox
```

### Firebase Cloud Messaging (`google-services.json`)

The app builds cleanly **without** `google-services.json` (the Google Services Gradle plugin is
applied automatically only when `android/app/google-services.json` is present). To enable FCM
push notifications:

1. Create or open a Firebase project and add Android app `com.digitalpals.fusebox`.
2. Download `google-services.json` into `android/app/google-services.json` (git-ignored).
3. Either:
   - **Push relay (recommended for Play Store):** give the relay its Firebase service account JSON
     with `--fcm-service-account-file` (`FUSEBOX_RELAY_FCM_SERVICE_ACCOUNT_FILE`), so any Fusebox
     server can send end-to-end encrypted notifications without holding the Firebase key; or
   - **Direct FCM:** set `fcm.service-account-file` in your Fusebox `config.yaml`.

### Demo mode (Debug builds only)

Debug builds accept launch extras so you can inspect every screen without a server:

```sh
adb shell am start -n com.digitalpals.fusebox/.MainActivity --ez demo true --ez dark true
adb shell am start -n com.digitalpals.fusebox/.MainActivity --ez demo true --es tab accounts
adb shell am start -n com.digitalpals.fusebox/.MainActivity --ez demo true --es tab requests
adb shell am start -n com.digitalpals.fusebox/.MainActivity --ez demo true --es tab usage --ez dark false
```

None of the demo helpers are invoked in Release builds.
