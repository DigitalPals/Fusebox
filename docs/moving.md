# Moving Fusebox to another machine

Fusebox keeps everything in files, so moving an installation is a copy, not
an export. There is deliberately no `fusebox backup` command for the whole
state: one archive holding every refresh token, API key and webhook URL is
exactly the file you least want lying around.

## Stop first

Stop the Fusebox process on the source machine, and do not start the copy
while the original still runs. Two instances must never share one set of
credentials: OAuth refresh tokens rotate, so whichever instance refreshes
second is told its token is revoked and the account is taken out of rotation
until it is signed in again. Run one proxy per auth directory.

## Copy these

1. **`config.yaml`** — the file `--config`, `FUSEBOX_CONFIG` (or the older
   `CLIPROXYAPI_RUST_CONFIG`) or the working directory points at. It holds
   client keys, API keys, webhook URLs and all settings.
2. **The `auth-dir` directory, whole.** The `*.json` files are the sign-ins.
   The dotfiles next to them are part of the installation, not caches. Some
   are relearned when missing; others are not, and losing those changes what
   Fusebox does:
   - `.disabled-api-keys.state` — the Enabled switches of API-key accounts
     (no keys in it). **Not relearned:** an API-key account you switched off,
     say because its key leaked, serves again.
   - `.web-push.state` — the web-push signing key and device subscriptions.
     **Not relearned:** installed apps and browsers stop receiving
     notifications until they subscribe again.
   - `.banked-resets/`, `.window-starters/` — journals of banked-reset and
     window-starter operations. **Not relearned:** an operation that hadn't
     settled is forgotten and may be made again.
   - `.cooldowns.state` — account cooldowns and refresh holds. Lost, they
     lift early: accounts are tried again, and one held out after failed
     refreshes is retried.
   - `.quota-attribution.state` — which clients and sessions used each quota
     window, and the weekly outlook history. Lost, the history starts again.
   - `.routing-sessions.state` — coding-session account assignments. Lost,
     sessions move accounts and lose their prompt caches.
   - `.quota.state` — last quota readings, including confirmed no-5h-limit
     findings. Relearned from the next readings.
   - `.model-lists.state`, `.client-addresses.state` — fetched model lists and
     the addresses client keys were seen from. Relearned.
3. **The usage database**, if Usage & Costs is on. Copy it with its own
   command rather than copying the live file:
   `fusebox --config <path> usage backup <file>` writes a consistent
   snapshot. Put the snapshot where the new installation's config expects the
   database.
4. **Files the config names**: the TLS certificate and key if `tls` is
   configured, and the Apple push key (`apns.key-file`, a `.p8` file) if push
   notifications go through APNs directly.

Paths in `config.yaml` (`auth-dir`, the TLS files, `apns.key-file`, the usage
database) are used as written. Relative ones follow the config file; absolute
ones must exist on the new machine too, or be edited before the first start.

## Then

Start Fusebox on the new machine with the same `--config`. Run
`fusebox --config <path> check` first if you want to see what it found
without starting the server. Clients keep working unchanged once they point
at the new address: the client keys, named clients and model names all come
from `config.yaml`.

Keep the copies private while they travel and after they arrive. The auth
directory and `config.yaml` together are every account you connected.
Delete transfer copies you no longer need.
