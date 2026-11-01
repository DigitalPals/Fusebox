# iPhone app push (APNs) production deployment, 1 November 2026

Native push for the Fusebox iPhone app is live in production as
**`0.12.1+gui.8bed43a2a2a5`**, built from commit
`22d9ffffeb6e19c2660d3cb92e6949a42f8c60d9` (PR #49). The previous release,
`0.12.1+gui.ba9e5be2f325`, is kept for rollback. The user authorized the forced
deployment.

The binary's SHA-256 is
`4efa325afca5cacd74db5a7747923913fdb9cc6dafd37621dc61dccda1fd544f`. Qualification
ran inside a managed task (lease `caa090af2dc6485c87b1c109d1977881`, closed with no
resources left). It covered 809 Rust tests (7 ignored), 159 dashboard tests, 46
Python tests, formatting and all-target Clippy with warnings denied on the
committed tree, a release build of the exact archive, 15 deployment-wrapper tests,
the previous/candidate database reopen smoke, the dev smoke and the usage
end-to-end check.

The updater committed transaction `ed21322800c2439ab008a2a6aeb309a0`. The forced
drain found 5 open connections and got admission in 67 ms. Postflight, the
feature postflight, the final service check and the public certificate/asset check
all passed:

- 0 automatic restarts, no network hold left, and usage schema 5 unchanged and
  healthy.
- 5 account identities preserved, with 4 of 5 ready, as before.
- The notification settings and both existing browser push devices are preserved,
  and the dashboard serves the "iPhone app" device label.

After the release committed, the `apns` section (team `4R8P9MTF9T`, key
`BVT5VMDU87`, topic `com.digitalpals.fusebox`) was added to `config.yaml`, and the
key was installed readable only by the service user. The config reloaded without
a restart, and `GET /api/push` now reports `"apns": true`. A throwaway device
token was registered for each APNs environment and sent a test notification. In
both, Apple accepted the provider token and refused only the unknown device token
(`BadDeviceToken`), and Fusebox removed the throwaway devices by itself. The
earlier config is kept beside it as `config.yaml.pre-apns-20261101`.

Before the build, a named client `mac-studio` was added through the settings API,
for Claude Code on the Mac Studio that builds the iPhone app. Together with the
`apns` section, this means the production config hash has changed since the
previous rollout.

Afterwards, the cleanup removed releases `7f867d6cc5c4`, `8815bfae7fb7` and
`98ebc3fb819f`, plus the incoming directories, one-shot units and job records of
the optimization, pool-bar and reset-notifications rollouts. Private local
artifacts and receipts are under `target/apns-push-production-20261101/`.
