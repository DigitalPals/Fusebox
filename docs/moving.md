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

1. **`config.yaml`** — the file `--config`, `FUSEBOX_CONFIG` or the working
   directory points at. It holds client keys, API keys, webhook URLs and all
   settings.
2. **The `auth-dir` directory, whole.** The `*.json` files are the sign-ins.
   The dotfiles next to them are part of the installation, not caches:
   - `.cooldowns.state` — account cooldowns and refresh holds
   - `.quota.state` — last quota readings, including confirmed no-5h-limit
     findings
   - `.quota-attribution.state` — which clients and sessions used each
     quota window, and the weekly outlook history
   - `.routing-sessions.state` — coding-session account assignments
   - `.model-lists.state` — fetched provider model lists
   - `.client-addresses.state` — addresses client keys were seen from
   - `.web-push.state` — the web-push signing key and device subscriptions;
     without it, installed apps stop receiving notifications
   - `.disabled-api-keys.state` — the Enabled switches of API-key accounts
     (contains no keys)
   - `.banked-resets/`, `.window-starters/` — banked-reset and window
     starter journals

   Losing a dotfile is not fatal — Fusebox relearns it — but sessions move
   accounts (and lose their prompt caches) and quota readings restart.
3. **The usage database**, if Usage & Costs is on. Copy it with its own
   command rather than copying the live file:
   `fusebox usage backup --to <file>` writes a consistent snapshot. Put the
   snapshot where the new installation's config expects the database.
4. **TLS certificate and key files**, if `tls` is configured.

## Then

Start Fusebox on the new machine with the same `--config`. Run
`fusebox --config <path> check` first if you want to see what it found
without starting the server. Clients keep working unchanged once they point
at the new address: the client keys, named clients and model names all come
from `config.yaml`.

Keep the copies private while they travel and after they arrive. The auth
directory and `config.yaml` together are every account you connected.
Delete transfer copies you no longer need.
