# Usage and cost estimates

Fusebox keeps a local history of token usage reported by requests sent through the proxy. It can also import metadata from opted-in Claude Code and Codex history files, or receive that metadata from a standalone collector on another machine. The Usage page combines these sources into one total that counts each provider response once, and shows an estimated API list-price equivalent when the model and reported token categories have a matching rate.

These numbers are for comparison and troubleshooting. They are not invoices, provider-reported subscription charges, or a forecast of remaining subscription allowance. Subscription quota windows remain on the Accounts page and are never converted into dollars or added to API-token estimates.

## What is recorded

Proxy observations are captured from usage metadata in HTTP requests, including translated and pass-through responses, SSE streams, native Codex WebSocket turns, and retry attempts. A logical request and its attempts are shown separately. If an upstream attempt fails after reporting usage, that usage remains attached to the attempt; retries do not erase it. Images and compaction are included when the upstream reports usage. Video generation is observed where usage is returned, but video status polls do not count as generations or infer generation use. Token-counting and model-list endpoints are not billed generations and are excluded.

Fusebox records allowlisted metadata such as provider, actual or requested model, token categories, event time, source, and identities that the source actually provides. It does not save prompts, completions, tools, transcripts, authorization headers, or API keys in the usage database. Account, auth type, client, request, and response identities are optional and remain unknown when the request does not establish them. Devin's richer usage metadata is not available through its current protobuf path, so any usage observed there is limited to fields exposed by the translated response.

Token categories use these definitions:

- **Input** is non-cache input. **Cache read** and **cache write** are separate input categories; they are not included in the input value again.
- **5-minute** and **1-hour cache writes** (Anthropic) and **30-minute** and **12-hour cache writes** (OpenAI) are details within cache write, not extra tokens.
- **Output** includes reasoning tokens. Reasoning is a detail within output and is not added a second time.
- A missing value is unknown, not zero. Partial usage stays marked partial.

Provider-native fields do not always line up. Claude Code and Anthropic report uncached input separately from cache reads and writes. Codex usage reports input inclusively, so the importer subtracts its cache-read and cache-write categories to derive uncached input. Fusebox preserves what it can and labels incomplete or unsupported records rather than filling gaps with zeroes.

## Cost estimates and subscription quotas

Fusebox bundles rate catalogue `2026-10-27.2`. Base model, tier and region prices were verified on 2026-10-18; OpenAI cache-write lifetime rules were rechecked on 2026-10-21 against the [prompt caching guide](https://developers.openai.com/api/docs/guides/prompt-caching). GPT-6.1 Terra and Muse Wonder Standard rates were added from official pricing on 2026-10-27; see the [model support review](model-support-2026-10-27.md). Its published source links, supported model IDs, rate periods, and assumptions are shown in the Usage page. The catalogue contains selected OpenAI, Anthropic and Meta API model rates. Other providers and models remain unpriced unless a matching rate is configured. Rates are stored at nanodollar precision to avoid rounding each request to cents. A price is calculated only when Fusebox has an actual model identity and all token categories needed by that rate; a requested model by itself is not enough to price an observation.

The bundled rows do not declare historical effective dates, so each has no start date. Usage older than the catalogue is priced at today's published rates and labelled a **current-rate equivalent**: its pricing basis is `current_rate_equivalent` and its saved price snapshot carries `"backdated": true`. That figure is what the same usage would cost at today's list prices; it is not what you actually paid at the time. Backdating alone does not mark an estimate partial. A rate that declares `effective_from` keeps a strict start: events before it, unknown models, unsupported explicitly reported tiers, unavailable cache-write TTL detail, unsupported regions, and incomplete token categories can be unpriced. Claude fast mode is read from `usage.speed`, and Codex native tiers from applied thread settings and their associated turn. Anthropic `inference_geo: "not_available"` uses an explicitly assumed global rate and a partial estimate; unknown region names remain unpriced. When a tier is omitted or reported as `auto`, Fusebox applies the standard rate as an explicit assumption and marks the estimate partial. An override can supply an organization-specific USD rate card; its content hash becomes part of the catalogue version, and it changes Fusebox's estimate, not a provider invoice. Unknown prices remain blank rather than `$0`.

OpenAI models with explicit write pricing use 1.25 times ordinary input for 30-minute writes and 2.5 times ordinary input for 12-hour writes, with the applicable tier, context and region adjustments. Positive writes without sufficient lifetime detail remain unpriced. Custom catalogues containing only the legacy generic write rate retain their configured pricing behavior. Anthropic five-minute and one-hour rates remain separate.

The catalogue has a limited set of explicit service-tier multipliers. For GPT-6 and GPT-5.6 models, Batch and Flex use one-half of standard, Fast and Priority use twice standard, and GPT-6 Astra and GPT-6.1 Sol Ultrafast use six times standard. Older OpenAI models use their published multipliers: GPT-5.5 Fast/Priority is 2.5 times standard and GPT-5 mini is 1.8 times standard; other supported Fast/Priority combinations use twice standard. GPT-5.5 and GPT-5.4 Fast/Priority requests above 272,000 input tokens remain unpriced because that combination has no published rate. For the listed Anthropic models, Batch uses one-half of standard; Fast uses twice standard only for Opus 5.5, Opus 5, and Opus 4.8. Other model/tier combinations are unpriced rather than assigned a guessed discount. The catalogue applies the published 272,000-token request threshold only to the OpenAI models that have threshold rates. Haiku 5.5 uses five times its base rates strictly above 100,000 input tokens, including cache reads and writes; five-minute and one-hour cache-write rates remain distinct at both lengths. Sonnet 5.5 cached input is $0.10 per million tokens, while Sonnet 5 remains $0.20. It applies a 10% US-region adjustment only to model families whose listed rates support that assumption; an unknown region does not imply US processing.

In particular, API-token estimates do not describe Claude Pro/Max, ChatGPT Plus/Pro/Team, or other subscription-plan usage. API billing and subscription allowances are separate provider products. The quota meters and cooldowns on Accounts continue to represent provider-reported subscription limits; the Usage page does not combine those limits with API costs.

### Who used a subscription window

Subscriptions run out in percent of a 5-hour or weekly window, and the providers report only the window's total. Fusebox estimates who used it. It reads the total after every Claude and ChatGPT response and from the usage endpoints, and it knows every attempt that finished on the account in between. When the reported use rises, the rise is split over the attempts that finished since the previous rise, weighted by their API list-price estimate (by weighted tokens when a model has no price). Whole-percent readings make attempts wait until the reported use actually moves; then they share it. A rise with no Fusebox attempt to explain it counts as **outside Fusebox**: claude.ai, the desktop app or another tool using the same subscription. Use already on a window when Fusebox began following it is kept apart as **before**.

These are estimates and labelled so. The provider's own accounting is per token and model, and Fusebox sees only the sum. Readings that arrive with a response describe the window before that response's own work, so each attempt is usually explained by the next reading. Model-scoped windows (such as a weekly Opus limit) count only matching models.

A client is its key: a named client by its name, a shared key from `api-keys` as "Shared key" and the start of its hash. Only a request without a key is named by its program. Each account's page shows, under every meter, which clients used the current window and the last one; its coding sessions show their share of the current windows. The Usage page's **Top clients** ranks the clients in the selected range and filters by estimated cost, with each one's share of the cost, its tokens and its models; it names clients only, not the subscription accounts they used. A window that would run out before it renews at its pace so far appears in the faults menu, naming the client that used most of it; it is not pushed and does not count on the app badge. For a provider with several subscriptions the warning is for all of them together, and names the client that used most of their windows, each account counting as it does on the Weekly quota outlook. Weekly pool warnings use the same recent-consumption forecast as the outlook ([calculation](weekly-outlook.md)). The current and last windows and the bounded recent quota history used for forecasts survive restarts in `<auth-dir>/.quota-attribution.state`; the usage database keeps no history of them.

## Local setup and configuration

Usage storage is enabled by default for the proxy and is created beside the selected `config.yaml` as `usage.sqlite3`. Imports and collectors are opt-in. The SQLite library is built into the Fusebox binary; no database server or additional runtime is required.

```yaml
usage:
  enabled: true
  # database: /var/lib/fusebox/usage.sqlite3 # default: beside config.yaml
  retention-days: 90
  queue-capacity: 1024
  # pricing-overrides: /etc/fusebox/pricing.json

# Optional per-client inference credentials. Existing api-keys continue to work.
named-clients:
  - id: workstation
    label: Workstation
    key: replace-with-a-long-random-client-key
```

`usage.enabled` turns proxy usage capture and storage on or off. Database, retention, and queue changes require a restart. Retention is purged at startup and about once per hour while the server runs; choose a period that fits your local policy. `queue-capacity` bounds the asynchronous proxy-write queue. Proxy observations use a nonblocking enqueue: a full queue drops the observation rather than delaying the API response. Health distinguishes the current writer's dropped, rejected, and failed counts from a persisted `historical_gap` recorded by earlier writer sessions. Those counters are persisted best-effort and can lag during a busy transaction or disk failure. A prior unclosed session also raises a warning because it can mean an unclean shutdown or another concurrent writer. The queue and the batch being written are the crash-loss bound for observations not yet committed. `pricing-overrides` points to a JSON rate catalogue. Each catalogue has a `version`, `verified_at`, optional `currency` (USD), and `rates` array. A rate identifies `provider`, `models`, `source_url`, optional `effective_from`/`effective_until` dates, integer nanodollar-per-token input/output and optional cache rates, and optional `tier_multipliers` as tier-to-numerator/denominator pairs. Optional threshold rates cover models whose input length changes the rate. See the bundled [catalogue](../src/usage/rates.json), [pricing implementation](../src/usage/pricing.rs), and cited sources in [`usage-provenance.md`](usage-provenance.md). Each observation retains its selected rate snapshot. Catalogue updates repair recoverable unpriced estimates and explicitly documented rate errors; every changed estimate retains its prior cost, catalogue and snapshot in `usage_price_revisions`, also exposed as `pricing_revisions` in observation details. Other established prices remain unchanged. The bundled catalogue is a verified release snapshot; it does not refresh from the network.

Named clients add stable attribution to an inference credential. They are additive to `api-keys` and are not a machine identity mechanism: anyone who has the key can use that client identity. Keep them private and rotate them if shared. Client keys are compared as credentials and only their derived scope is used for attribution; old API-key behavior is unchanged.

With a running server, use **Usage** in the dashboard to inspect totals, source coverage, health, imports, collectors, and the pricing assumptions. `/api/usage/dashboard`, `/api/usage/observations`, `/api/usage/status`, and `/api/usage/export` use the dashboard's management authentication. `/api/usage/export` returns one page of records at a time, as JSON or CSV. Use a local database backup for a complete archive. The former `/api/usage/summary` endpoint returns a management-authenticated `410 Gone`; migrate to the dashboard contract rather than expecting its former per-source or request/attempt aggregate fields.

## Import Claude Code and Codex history

Native-history import is disabled until you explicitly enable a source root. Fusebox only reads the selected directory, not provider credentials or settings. It scans local JSONL history for usage-bearing records and stores a small metadata projection. Claude Code roots are normally under `~/.claude/projects` (or `$CLAUDE_CONFIG_DIR/projects`); Codex roots are normally under `$CODEX_HOME` or `~/.codex`, which contains `sessions` and `archived_sessions`. The importer recursively scans the root, including archived and compressed Codex JSONL files. When a plain `.jsonl` file and its `.jsonl.zst` sibling both exist, the plain file is preferred.

The source formats are version-sensitive. The importer recognizes Claude Code assistant rows carrying `message.usage`, and Codex `token_usage_record` events plus legacy cumulative `token_count` snapshots. Prompt history and generic token estimates are not usage records. Codex's durable response record is preferred over its overlapping cumulative snapshots. Copied Codex histories deduplicate by stable response identity when available; filename or ordinal alone is not treated as a reliable event identity. Claude Code message IDs are preferred, with the transcript row UUID as a fallback. The native Claude Code JSONL billing schema and universal request-ID guarantee are not published as a stable contract, so weaker identities can leave visible duplicates or conflicts. Imports never infer the account currently signed in to Fusebox as the account that produced old local history.

The parser is versioned as `native-2026-10-18-v3`. Its format references are the [Codex configuration and state locations](https://developers.openai.com/codex/config-advanced#config-and-state-locations), the pinned [Codex rollout event definitions](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/history/src/rollout_payload.rs) and [token usage protocol types](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/protocol/src/protocol.rs#L2295-L2326), plus Anthropic's [Agent SDK session locations](https://platform.claude.com/docs/en/agent-sdk/sessions) and [settings for `CLAUDE_CONFIG_DIR`](https://code.claude.com/docs/en/settings). Codex history is an implementation format that can change between CLI releases. Claude's public documentation establishes history locations and API usage fields, but not a complete stable Claude Code billing transcript schema. Fusebox's parser version and these source references describe the formats checked for this release; they do not promise that every installed client version records identical fields.

Use the CLI to see candidate locations, enable only the roots you consent to import, then scan:

```sh
fusebox usage status --database ./usage.sqlite3
fusebox usage enable --database ./usage.sqlite3 --source codex --root "$HOME/.codex/sessions"
fusebox usage enable --database ./usage.sqlite3 --source codex --root "$HOME/.codex/archived_sessions"
fusebox usage enable --database ./usage.sqlite3 --source claude_code --root "$HOME/.claude/projects"
fusebox usage scan --database ./usage.sqlite3
fusebox usage status --database ./usage.sqlite3
fusebox usage reprice --database ./usage.sqlite3
```

Use the actual Claude Code project root if you keep it in a different location. The CLI requires an absolute root path; repeat `enable` with the same source and root to re-enable it. `usage scan` can take `--source codex` or `--source claude_code`. `usage disable` stops future scans but keeps imported metadata. `usage backfill` clears scan checkpoints so enabled roots can be read again; stable events remain idempotent. `usage reprice` refreshes saved bundled rates to this release, retries recoverable unpriced observations (including newly supported models, tiers, dates and regions), and prints `{"repriced": N}`. Organization overrides are preserved. The server applies the same repairs at startup using its configured catalogue. The one established-price correction in this release repairs Sonnet 5.5 cached reads originally priced by bundled catalogue `2026-10-07.3`; organization overrides and other established prices remain unchanged. Each changed estimate is audited with its previous snapshot in the same transaction as its totals. Repeated runs without new repairable observations report zero. These commands open an existing usage database and do not start the proxy or create a new database.

Once a root is enabled, the running server scans it about every 30 seconds; `usage scan` requests a scan immediately. A source root stays opt-in, and the status command only reports candidate paths until one is enabled. Scanning is bounded to protect the machine: no symlink traversal or network filesystem roots, at most 4,096 files/directories, 32 MiB scanned per root per pass, 256 MiB per file, 512 KiB per line, and 2,000 usage observations per file per pass. A long history continues over later scans. A trailing incomplete line is retried from its start. Status reports imported, duplicate, skipped, unsupported, failed, or deferred rows. Unsupported rows are not silently translated into guessed charges.

Codex legacy cumulative snapshots are converted to component-wise differences. When a validated `last_token_usage` is available, the first snapshot records only that response; later last-response counts must match cumulative growth to support per-request pricing. Those prices stay partial estimates. Counter jumps covering multiple responses remain unpriced. Repeated snapshots do not add usage again. Counter resets, copied fork prefixes, estimates, and ambiguous records are treated as incomplete or deferred instead of manufacturing negative usage. The source parser/version is stored so changes to future import support do not rewrite existing history without a reimport.

## Reconciliation and overlap

Each observation retains its source and origin. Repeated ingestion is idempotent by source, origin, and source event identity. When a reliable provider response/message identity ties a local import to a proxy observation, those observations can be reconciled. Proxy evidence is preferred over local import evidence, which is preferred over collector evidence; differences in model, account, or reported counts remain visible as conflicts. The system does not match records by similar token counts or timestamps.

The Usage page shows one combined total with one entry per provider response. An imported Claude Code or Codex entry is left out when a proxy entry has the same provider response ID, wherever that proxy entry falls in time, so the proxy evidence wins and a call is not counted twice. Imported entries without a response ID cannot be matched to the proxy; the page counts them and reports how many there are, because they may still duplicate proxy traffic. The per-source totals remain available in the API and may overlap; do not add them together. Imported histories may contain activity outside Fusebox, may be incomplete, and may reflect subscription usage rather than API billing.

Retention purges old observations and source entries at startup and approximately hourly and leaves a watermark so an ordinary future scan does not resurrect manually purged history. `usage backfill` clears checkpoints for enabled roots and performs an explicit reimport, temporarily allowing those source records past the manual-purge watermark. It does not extend the configured age-retention period: older records can be purged again at startup or the next hourly sweep. Keep native source histories if you may want to import them later.

## Import previous CPA usage keeper history

The CLI can import the normalized `usage_events` table from CPA usage keeper v1.15.4 into an existing Fusebox database. Start with a read-only preview, using the same origin identity for every import of this database or one of its backups:

```sh
fusebox usage import-keeper --database /var/lib/fusebox/usage.sqlite3 \
  --input-database /var/lib/cpa-usage-keeper/data/app.db --origin keeper:production
```

Add `--apply` to insert the eligible records. Take a consistent SQLite backup first. The importer reads the source through a read-only snapshot and commits small batches through Fusebox's existing pricing and deduplication path, allowing native recording to continue. It preserves event times, separates keeper's inclusive input into uncached input/cache reads/cache writes, keeps reasoning inside output, and prefers the returned service tier over the requested tier. Repeated imports are idempotent by origin and keeper row ID; reused client request IDs do not merge distinct historical events. Imports overlapping native proxy history are refused because keeper did not retain reliable response identities to reconcile that overlap.

Imported observations have source `proxy`, the chosen `keeper:` origin, and a versioned keeper parser. Historical account references are namespaced hashes, without assigning them to today's accounts. Credentials, account email labels, IP addresses, user agents and raw messages are excluded. Unknown request/attempt grouping remains unknown. Failed all-zero usage remains unpriced. Keeper did not retain cache-write TTL, inference region or Claude fast-mode speed; those details are not inferred. Costs use the configured catalogue and retain partial/current-rate-equivalent labels, including unpriced cache writes whose TTL is unknown. Invalid, unsupported, non-generation and out-of-retention rows are reported separately. A failure after a committed batch can be retried safely with the same origin.

### Estimate historical cache writes

For an opted-in keeper history, the CLI can price calls whose only missing
pricing detail is cache-write lifetime. Preview the affected records first:

```sh
fusebox usage estimate-keeper-cache --database /var/lib/fusebox/usage.sqlite3 \
  --origin keeper:production
```

Add `--apply` after taking a consistent SQLite backup. This assumes that the
known cache-write tokens use the five-minute rate, calculates the remaining
charges from the recorded tokens and model/tier/context rates, and updates
the existing raw/canonical/source pricing entries in atomic 100-record batches.
It applies only to the named `keeper:` origin and verified keeper parser,
previously unpriced Claude calls, and records with complete required token
categories. Already-priced snapshots, native capture and missing-usage or
unsupported-model records are unchanged. Repeating the command is harmless.

The original payload, identity, token columns and unknown lifetime splits are
preserved. Pricing snapshots use `historical_cache_write_estimate`, are marked
partial, and include `historical_cache_write_lifetime_5_minutes_assumed`. The
`historical_estimate` metadata records the method/version, original snapshot
digest and basis, application time, and the corresponding five-minute/one-hour
cost bounds. These bounds vary only cache lifetime while holding other pricing
assumptions fixed. The dashboard's record detail displays the assumption.
Native pricing and automatic repricing never apply this estimation policy.

## Standalone collector

A collector runs on a workstation and sends only bounded usage metadata to Fusebox. It does not start the proxy, load Fusebox provider accounts, or upload transcript text. The server must be reachable at its management API; remote management access must already be configured and protected. Create a collector credential in the dashboard's Usage page. The credential is displayed once. It is stored hashed on the server, and the collector receives only Claude Code/Codex source claims; the server binds them to the enrolled collector and strips client, account, request, attempt, status, and requested-model claims that a collector must not assert.

Install the credential without placing it in command arguments or shell history. In Bash, paste the one-time credential at the hidden prompt; it is piped to the collector's standard input:

```sh
umask 077
read -r -s -p 'Collector credential: ' COLLECTOR_TOKEN
printf '\n'
printf '%s' "$COLLECTOR_TOKEN" | fusebox collector enroll \
  --state-dir "$HOME/.local/state/fusebox-usage" \
  --destination 'https://fusebox.example/api/usage-ingest' \
  --codex-root "$HOME/.codex/sessions" \
  --codex-root "$HOME/.codex/archived_sessions" \
  --claude-root "$HOME/.claude/projects"
unset COLLECTOR_TOKEN
```

Omit a root option if you do not want to import that history. `collector enroll` saves local state and an SQLite outbox under the state directory with owner-only permissions. It accepts credential input from stdin by default; `--credential-file` is also available for an existing file with mode `0600` or stricter. State directories must be absolute local paths and cannot be symlinks. Use a verified HTTPS destination except for loopback testing; redirects are disabled. Keep normal TLS certificate verification enabled.

Run the collector as a user service or start it manually:

```sh
fusebox collector status --state-dir "$HOME/.local/state/fusebox-usage"
fusebox collector sync --state-dir "$HOME/.local/state/fusebox-usage"
fusebox collector run --state-dir "$HOME/.local/state/fusebox-usage"
```

`run` scans enabled roots and synchronizes periodically; it retries unavailable destinations with a delay capped at five minutes. `run --once` performs one scan/sync cycle and exits. `sync` sends the durable outbox without starting the proxy. Batches are limited to 200 observations and 512 KiB, with server-side request limits. The local outbox is bounded to 10,000 records or 16 MiB. The standalone collector database uses SQLite DELETE journaling with FULL synchronization and a 64 MiB page allocation limit; the database plus transient rollback journal has a 130 MiB budget. This is separate from the server database, which uses WAL. See [collector storage and progress](usage-imports-collectors.md) for allocation-failure recovery and the scope of this bound. If it fills, scanning is deferred and status reports the pending backlog; free space or restore connectivity and run it again. Outbox records remain local until the server durably acknowledges them. A lost connection can cause a retry, which is safe because event ingestion is idempotent.

The server's collector status distinguishes contact from successful synchronization and reports offline collectors. It shows sources, observed time ranges, last scans and per-source import/duplicate/skipped/unsupported/failed counters. These counters describe work completed; total accessible historical coverage remains unknown, and no completion percentage is manufactured. Rotate or revoke credentials in the dashboard. Rotation immediately invalidates the prior token; enroll the collector again with the newly displayed token, keeping its existing state directory and outbox. Revocation stops future ingestion but does not delete already synchronized history or local unsent data. Delete the local state directory yourself when retiring a collector, after deciding whether to retain its outbox.

## Storage, backups, and recovery

The server usage database is a versioned SQLite file using WAL mode, full synchronous commits, foreign keys, and transactions. The current SQLite schema is version 5. Migrations run in a single transaction: version 1 to 2 adds the writer-session health table, version 2 to 3 adds the `usage_entries(association_key, source)` index used by combined accounting, and version 3 to 4 makes the database smaller and the Usage page faster (below), and version 4 to 5 adds OpenAI lifetime columns and rebuilds the rollups transactionally. After migrations the server prices again any rows saved as unpriced only because they predated the catalogue, in batches of at most 2,000 rows per transaction. Startup also corrects previously priced OpenAI rows only where stored lifetime evidence establishes the proper price, retaining previous price snapshots in revisions. Historical records without that evidence keep their existing estimates: the missing lifetime cannot be recovered. New positive writes with unknown lifetime remain unpriced. This database schema version is separate from the version 1 observation/batch envelopes and the versioned native importer parser. Fusebox applies supported migrations on startup and rejects a database from a newer schema version.

A normal upgrade from schema 4 has no stored OpenAI lifetime evidence to correct. The conditional repair applies to lifetime-bearing observations priced with the earlier generic-rate catalogue; it does not reconstruct lifetime counts for old proxy history or native Codex imports.

Proxy observations go through a bounded background writer so disk latency does not hold up a response. Queue acceptance is not yet a durable commit: a crash can lose queued events and the current uncommitted batch. A full queue increments the current writer's dropped count; a malformed observation increments rejected; a failed insert batch increments dropped and writer errors. Health reports the current session plus best-effort persisted counters from earlier writer sessions. A previous unclosed session can indicate a crash, forced stop, or concurrent process sharing the database. Counters can lag because health snapshots are persisted between commands and on an idle timer, and disk failure can prevent their update. A storage gap remains visible as degraded health rather than an implicit zero. Imports and collector ingestion wait for durable transaction acknowledgment; their data and checkpoints/outbox updates commit together so retries are safe.

### Size

Schema 4 stores each record's payload and price snapshot compressed, with a zstd dictionary trained on the database's own records once it holds 1,000 of them (and again after every 100,000 more); older records are compressed in the background in short steps between writes. It keeps 15-minute rollups of the accounting entries and of the filter values, maintained by triggers, so the Usage page reads a few thousand rollup rows instead of every record; partial buckets at the ends of a range and imported histories are still read from the records, so totals are exact in every timezone. It drops the per-source copy of the accounting entries and three indexes that no query used. A new database uses incremental vacuum, so each hourly retention purge returns its space to the filesystem; an existing database is vacuumed once, after its records are compressed, when there is free disk for a copy of it (otherwise its free pages are reused and `fusebox usage compact` can shrink it later). On one production database of 286,593 records this took the file from 1,913 MiB to 760 MiB and the 30-day Usage summary from 450 ms to 9 ms, with every answer unchanged.

`GET /api/usage/status` reports the database size, live and free bytes, bytes per record, an estimate of daily growth, the free disk space and whether compaction is still running.

Releases before 0.6 read only schema 3. Before going back to one, stop Fusebox and run `fusebox usage downgrade` (with `--config` or `--database`): it decompresses every record, removes the rollups and recreates the schema-3 tables and indexes. `fusebox usage compact` compresses and vacuums a stopped database in one go. Downgrade refuses before changing the database if any observation contains the new OpenAI lifetime fields, including explicit zeroes, because older readers cannot preserve them. A database containing only legacy fields can still be downgraded; unsupported lifetime-priced models are omitted from its active legacy catalogue, with the original catalogue retained for audit. Both refuse to run while a Fusebox process has the database open (a `usage.sqlite3.lock` file next to it records that).

```sh
fusebox --config config.yaml usage downgrade
```

A binary-only rollback to a schema-4 release cannot open schema 5 and leaves usage capture unavailable. Before upgrading, take a consistent backup and allow time and disk space for the rollup migration. Check `/readyz` and Usage health after either activation or rollback; a successful `/healthz` alone does not confirm accounting is running.

If lifetime observations have already been recorded, stop Fusebox and use the new binary to create a separate rollback database:

```sh
fusebox --config config.yaml usage rollback-copy ./usage-rollback.sqlite3
```

The output must be a new file. The command retains the original database with its full lifetime evidence and price audit. The copy preserves observations, identities, established costs and revision rows, while omitting OpenAI lifetime counts and unsupported lifetime rate fields from its documents. It uses the existing schema-3 layout, which schema-4 releases migrate on startup, and omits unsupported models from the active legacy rate catalogue rather than guessing a generic write price. Insufficient disk space or a conversion error fails the command and removes the new copy.

Point the older release's `usage.database` at the copy and verify accounting health before accepting traffic. Keep the original as the complete historical archive. When upgrading again, continue with the active copy so usage recorded during rollback is retained; the new release migrates it normally. Switching back to the archive would omit that interval. The existing in-place `usage downgrade` continues to refuse lifetime-bearing databases.

Back up the database through SQLite's backup API while Fusebox is running. `fusebox usage backup` does that for the database your config file uses (or the one `--database` names), writes a new private file, and checks its integrity before saying it's done:

```sh
fusebox --config config.yaml usage backup ./usage-backup.sqlite3
```

`sqlite3 ./usage.sqlite3 '.backup ./usage.sqlite3.backup'` produces the same consistent snapshot, including committed data in the WAL; for a custom path, replace both paths with the configured database location. Avoid copying only the main `.sqlite3` file while the service is running: recent committed rows may still be in `usage.sqlite3-wal`. For a filesystem backup, stop Fusebox cleanly first and preserve the database and any WAL/SHM files together. Keep backups as private as the database; although the database omits prompt content and credentials, it can contain account/client labels, models, timestamps, and activity patterns.

On a normal server shutdown, Fusebox stops the import poller, closes usage observation ingress, drains queued writer commands, marks its writer session clean after acknowledgment, and closes the usage database. A shutdown flush failure is logged. A forced kill or power loss cannot run this sequence; the prior session remains unclosed and uncommitted observations may be missing.

If the database is damaged, stop the service, preserve the damaged file and its companion WAL files for diagnosis, then restore a known-good SQLite backup to the configured database path and start Fusebox. Do not delete a WAL file from an active database. Check the Usage health view after startup for writer errors, dropped/rejected observations, historical gaps, queue depth, prior-unclosed warnings, and last commit time. If storage remains unavailable, proxy serving does not mean usage capture is healthy.

## HTTP API and export

The read API is under `/api/usage` and uses the existing dashboard management authentication. The collector posts to `/api/usage-ingest` with its separate collector bearer token. Dashboard, observations, and export accept date range, IANA timezone, and provider/model/account/client/source filters; the displayed end date is inclusive while the API's end boundary is the next local calendar midnight. Pagination is bounded. JSON and CSV exports contain real identifiers and labels and should be handled like the database. CSV formula-like values are escaped.

See [`usage-ui-contract.md`](usage-ui-contract.md) for the response and dashboard contract, and [`usage-provenance.md`](usage-provenance.md) for the native-format field references, pricing source links, and parser confidence boundaries.

Collector/client filters select named clients or `collector:<server-id>` identities. Source records include retained revisions and copies; selected accounting entries remain globally deduplicated. A filtered collector can have raw records whose preferred accounting evidence belongs to another collector or a local import. `/api/usage/observations` keeps the matching detail records.

The T3 Code comparison, calculation examples, implementation decisions, and production evidence are in [the usage research report](../notes/t3code-usage-research.md).

When running `usage downgrade`, the full current catalogue is retained in `usage_meta.catalogue_before_downgrade`. Rate combinations that older calculators cannot represent are omitted from the downgraded active catalogue; established observation prices and their revision audit remain intact. Restarting this release with its configured catalogue restores current rate support.
