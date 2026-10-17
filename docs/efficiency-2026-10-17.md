# Efficiency and quota attribution, 17 October 2026

This implements the improvement study of the same date: a smaller and faster
usage database, lighter dashboard traffic and rendering, the remaining hot-path
copies and parses, an allocator setting chosen by measurement, client identities
that keep up by themselves, and quota attribution. Measurements below were taken
on Linux x86-64 (64 hardware threads) with release builds of `main` at `101f84b`
(0.5.1) and of this branch, both built locally with rustc 1.96.1 and without
the release workflow's RELR packing.

## Usage database (schema 4)

- Each record's payload and price snapshot are stored as zstd frames with a
  64 KiB dictionary trained on the database's own records (`src/usage/store/codec.rs`).
  On production rows the two documents shrank from 1,999 to 212 bytes on average.
  Older rows are compressed in the background in 2,000-row steps; the compactor's
  cursor is persisted, so a caught-up database is never rescanned.
- Fifteen-minute rollups of proxy accounting entries and of filter values are kept
  exact by triggers (`src/usage/store/rollup.rs`). The summary reads whole buckets
  from them and scans only the partial buckets at either end and imported entries.
  A bucket that would cross a local midnight, or a sum past 64 bits, falls back to
  the full scan.
- The per-source winner table (read only by the retired summary) and three unused
  indexes are dropped. New databases use incremental vacuum; an existing one is
  vacuumed once after compression when there is disk for a copy.
- `fusebox usage downgrade` restores the schema-3 layout for releases before 0.6;
  `fusebox usage compact` does compression and vacuum offline. A shared/exclusive
  lock file keeps both from running beside a live writer.
- `/api/usage/status` reports size, growth and free disk; a `storage` fault warns
  past `usage.warn-database-mb` (2,048 by default) and errs below 1 GiB free disk.

On a consistent copy of the production database (286,593 records), the current
release and this branch answered 14 Usage queries (seven timezones and ranges,
filters, record pages). With the new `quota` section set aside, every answer was
identical before compaction, after it, and from 0.5.1 again after
`usage downgrade`.

| Measure | 0.5.1 | Schema 4 |
| --- | ---: | ---: |
| Database on disk | 1,913 MiB | 653 MiB |
| Usage summary, last 30 days | 457 ms | 9.6 ms |
| Usage summary, all history, Amsterdam | 573 ms | 8.7 ms |
| Usage summary, all history, Kathmandu, by model | 526 ms | 12.8 ms |
| Startup (including the migration) | 0.7 s | 3.3 s |
| Background compression and vacuum | — | 55 s |
| `usage downgrade` | — | 44 s |

Growth at the current rate is about 12 MB a day (from 2,391 stored bytes per
record including indexes and rollups), so 90 days of retention settles near 1 GB.
The synthetic usage benchmark (`scripts/usage-benchmark.py`, 10,000 imported
records, two runs each) shows the cost of the triggers and compression on
ingestion: imports took 0.90–0.93 s instead of 0.73–0.77 s. The same runs measured
sequential proxy requests at 0.74–0.77 ms instead of 0.91–0.93 ms.

## Dashboard and management API

- `/api` responses are gzip-compressed (the 300-entry request log is 194 KB, 9.6 KB
  compressed); `/api/requests?limit=` lets the first paint fetch 6 entries.
- One live ticker computes account load and faults once per second for every
  dashboard and sends only changes (it starts with the first dashboard); totals
  are sent only when they change. A `models` message replaces refetching the model
  list on every account change, decided by a fingerprint of `/api/models`.
- The dashboard draws the Usage page once and patches sections, coalesces live
  request updates into one frame, pauses its timers while hidden, caches Chinese
  lookups, uses the server's fault list, and loads deferred scripts with hashed,
  immutable asset URLs.

## Hot path

- Responses continuation history parses only finished items and terminal events;
  deltas are buffered unparsed and replayed only when a backend sends neither
  output nor items. The native Codex WebSocket parses each message once.
- Ready SSE frames leave in one write, encoded straight into the buffer, with one
  keepalive timer per stream.
- The session registry sweeps every five seconds and checks the looked-up
  assignment exactly; a lease releases through a session index. New assignments are
  written while the provider is contacted and are durable before the client sees
  output.
- The caller's cache intent is read once per request instead of per attempt;
  history keeps the request's input instead of copying it; repeated warnings are
  logged once per ten minutes; `Pool::canonical` checks listings without building
  them.
- Not changed: pass-through still clones the body per attempt, because preparation
  rewrites it per account; and the SSE decoder still allocates each event's data,
  because events own it. Pass-through streams were confirmed not to retain request
  bodies.

## Memory

mimalloc doubled idle memory (32–34 MiB against 15 MiB) without lowering the peak,
so it was not adopted. glibc tunables were measured instead, with sixteen
concurrent 1.1 MB streaming requests and with a CPU-bound load of 200 KB requests:

| glibc setting | RSS, large streams | CPU per request |
| --- | ---: | ---: |
| Default, 64 workers | 206–227 MiB | 0.81 ms |
| 4 arenas + 256 KiB mmap threshold, 64 workers | 106–125 MiB | 0.86–0.88 ms |
| 2 arenas + 256 KiB mmap threshold, 64 workers | 79–92 MiB | 1.04–1.12 ms |
| Default, 2 workers | 91–103 MiB | 0.52–0.55 ms |
| 2 arenas + 256 KiB, 2 workers | 84–91 MiB | 0.53–0.55 ms |

Fusebox now sets four arenas and a 256 KiB mmap threshold before its runtime
starts (glibc's own environment variables still win). Under the large-stream load
the final binary held 104–124 MiB against 178–191 MiB for 0.5.1.

## Client identities

Cloaked and translated requests claim the newest Claude Code and Codex release
seen from real clients, or one set in `client-identities` (also Grok Build, the
Devin CLI and Antigravity), including Claude Code's billing fingerprint. Claude
Code's runtime headers are learned from its newest release. The daily
client-versions issue now names the `client-identities` line that catches up
without a release.

## Quota attribution

Each rise in a subscription window's reported use is split over the attempts that
finished since the previous rise, weighted by list price (or weighted tokens), per
named client or program, session and model; unexplained rises count as outside
Fusebox. Account pages and coding sessions show it; a `pace` fault names the
client using most of a window that would run out early. State survives restarts in
`<auth-dir>/.quota-attribution.state`; the usage database keeps no history of it.

## Validation

- 493 Rust tests, 72 dashboard tests and 30 Python tests pass; Clippy with warnings
  denied and rustfmt are clean.
- New tests cover rollup equivalence with a full scan through inserts, suppression,
  a mid-bucket purge and repricing in six timezones; trigger-maintained rollups
  against a rebuild; compression invisibility; downgrade restoring the original
  text and schema-3 objects; offline-command locking; attribution splitting,
  scoping, renewal and persistence; and an end-to-end attribution test through the
  router.
- The dashboard views were checked in a browser against a mocked ChatGPT backend.
- Deployed to production on 17 October. The migration and compression of 289,958
  records took about a minute, including a 4.7 s vacuum: the file went from 2,000 to
  668 MB (276 MB on the server's compressed filesystem), and the 30-day summary
  answers in 8 ms. Two faults showed there and were fixed: the budget totals for
  named clients read a record's status with `json_extract` on the now-compressed
  payload (they now use `usage_document()`, a SQL function every usage connection
  registers), and attribution read a one-point dip between the usage endpoint and
  response headers as a renewed window (a reset now needs a drop of more than five
  points, or to zero). A rollback to 0.5.1 needs `fusebox usage downgrade` run with
  this release first.
