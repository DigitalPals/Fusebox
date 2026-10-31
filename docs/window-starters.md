# Start idle usage windows

Fusebox can send a small Claude subscription request before a job arrives, so
the provider starts its five-hour usage window earlier. For example, starting
at 08:00 can bring renewal forward to about 13:00 even when real work starts
at 11:00. The provider's reported reset time is always authoritative.

This consumes a little subscription allowance. Unused capacity expires earlier,
and the feature does not increase weekly allowance. Automatic starting is off
by default, and only explicitly selected accounts participate. API keys, custom
gateways and other subscription providers are not supported.

## Settings

Under **Config → Routing → Start idle usage windows**, select **Always** or
**During working hours**, then select individual accounts. An empty selection
starts no accounts. You can choose a native Claude model available on those
accounts; the default is `claude-haiku-4-5-20251001`.

Working hours use an IANA timezone, selected weekdays and an exclusive end time.
Daylight saving changes follow that timezone. Overnight hours belong to the
day they start: Monday 22:00–02:00 includes Tuesday 01:00. Equal start and end
times are rejected; use Always for unrestricted operation.

```yaml
window-starter:
  mode: working-hours        # off (default), always, working-hours
  accounts: ["<account-id>"] # IDs from GET /api/accounts, not email addresses
  model: claude-haiku-4-5-20251001
  timezone: Europe/Amsterdam
  weekdays: [1, 2, 3, 4, 5]  # Monday=1, Sunday=7
  start: "08:00"
  end: "18:00"
```

In the CLIProxyAPI v8 layout, put the same object at `routing.window-starter`.
Settings reload without restarting the server. Existing windows are never
restarted or moved by a settings change. Fusebox must be running to send starters;
the provider's clock keeps running if Fusebox stops.

Each supported account's details and drawer include **Start now** and its latest
status. Manual starting works even when automatic mode is off or outside its
schedule. It still performs the same quota, activity and duplicate checks.
**Check status** only reads the provider; it never generates a message.

## Execution and accounting

The worker checks once a minute and ordinarily rechecks an account every five
minutes. It refreshes OAuth credentials and verifies the provider's account and
organization IDs. Duplicate credential files for that identity share one journal.
The provider must explicitly report an inactive short window and available weekly
quota; missing or malformed data and zero-percent active windows do not qualify.

Verified Claude Ultra accounts (formerly Max 20x) have no five-hour limit. Their
usage reports `five_hour: null` and its typed limit list has a weekly entry but no
session entry. Such an account shows **No five-hour limit**, gets no starter request,
manual or automatic, and is rechecked hourly in case a limit returns. A new Ultra
account keeps a temporary five-hour limit until Anthropic verifies it, and is
started like any other until then. When `five_hour` is null, the list's session
entry decides: idle only at 0% with no reset time, active with a future reset,
unknown otherwise. A list with an entry that can't be classified, or that names a
session in any way, proves nothing. A null without any list keeps its old meaning,
an idle window, except on Ultra or an account already confirmed without a limit,
where it is unknown and only a status check is made; only a reported five-hour
window brings the limit back. Once an account reports a five-hour window again, the
starter rechecks it within five minutes instead of at the hourly recheck.

Before dispatch, Fusebox checks settings again and yields to in-flight requests,
recent requests and recent coding sessions. It skips disabled accounts, applicable
cooldowns and exhausted limits. Starters share an organization lock with banked
reset redemption, including across processes sharing the same credentials directory.
Unresolved banked resets block new starters. Requests from external clients can
still start a window between the final check and dispatch; a starter never forces
a reset of an already active window.

One request goes directly to the selected account using native upstream
preparation, with an eight-token output cap, thinking disabled and no tools,
premium options, account fallback, redirects or automatic HTTP retries. If the
chosen model is unavailable, that account is skipped with an explanation.
Provider-enabled usage credits remain subject to the provider's own rules; the
starter checks subscription allowance first and never deliberately falls back
to credits or an API key.

Tokens are captured in Usage under the client **Window starter**, including
subscription quota attribution. They do not create coding sessions, update
latency-routing samples or count as ordinary client jobs. Usage capture follows
the installation's existing analytics setting.

## Durable outcomes

A private journal in `<auth-dir>/.window-starters/` records intent before the
request is sent. Never delete this directory to retry a recent uncertain attempt.
The organization lock uses the existing `<auth-dir>/.banked-resets/` directory,
even when the banked-reset feature is off. No saved reset is spent by a starter.

A successful message alone does not prove a new window. Fusebox re-reads usage
and displays the actual provider reset time. A crash, cancellation, redirect,
server error or missing confirmation leaves the attempt uncertain. Subsequent
checks only reconcile usage and hold further generations for five hours and ten
minutes from the attempt, unless an active window is confirmed sooner. After
that hold, a new request still requires fresh, explicitly inactive provider data.
Definite refusals use the provider's Retry-After (between one minute and a day),
or a 30-minute backoff. Starter errors are separate from ordinary account health.
Malformed journals block starting rather than being overwritten.

The authenticated management action is:

```http
POST /api/accounts/<id>/window-starter
Content-Type: application/json

{"request_id":"<new UUID>","check_only":false}
```

Set `check_only` to true for a read-only provider check. The account response
includes `window_starter_supported`, `window_starter_selected` and
`window_starter` status, last attempt time, confirmed reset and retry time.
