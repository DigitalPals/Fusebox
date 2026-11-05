# Release binary size

For the latest matching-source baseline and efficiency implementation, see the
[15 October results](efficiency-2026-10-15.md): the native release shrank from
16,606,928 to 15,783,216 bytes (4.96%) with opt-level 3 and panic unwinding retained.
The measurements below document earlier optimization passes.

## 5 November 2026

A council review traced the jump from 17 MB to 20.2 MB in production deploys
mostly to their build command: from 23 October they ran a plain `cargo build`
without the link flags the published builds pass. The link flags moved into
`build.rs`, so every release build gets them. Measured on one source snapshot
with rustc 1.96.1:

| Build | Executable bytes |
| --- | ---: |
| Plain `cargo build --release` before | 20,364,808 |
| The same with the published link flags | 19,110,448 |
| Plain `cargo build --release` after | 18,057,296 |

- **Link flags in `build.rs`:** 1.25 MB, mostly the 1.1 MB of `.rela.dyn` that
  packed relocations replace.
- **Time zones from jiff instead of chrono-tz:** `.data.rel.ro` went from 1.31 MB
  to 0.30 MB. chrono-tz kept a pointer per transition, which the loader relocated
  at every start; jiff keeps the tz database as compact TZif data (`src/zone.rs`).
  The bundled data is newer (2026e instead of 2025b), so days move where the
  rules changed, such as America/Vancouver from 1 November 2026 and Morocco
  (Africa/Casablanca, Africa/El_Aaiun) from 20 September 2026. Update it with
  `cargo update -p jiff-tzdb`.
- **No unwind tables for bundled C** (`.cargo/config.toml`): 146 KB.
- **Dependency sources remapped** to crate-relative paths in the release
  workflow and Docker build: 57 KB, and no build machine paths in the binary.
- Unused axum, tokio and futures features are off.

Proxy CPU per request, latency, usage imports and dashboard reads were measured
against the previous build in alternating runs and did not get slower; memory at
rest is about 1 MB lower.

The production executable measured on 2026-10-07 was 17,653,576 bytes
(17.65 MB, 16.84 MiB), SHA-256
`266da33eec9595c700df0afb927d29f6e7ff9c213356ee826dfb03e1a571f760`.
This was the live executable behind `cli-proxy-api.service`, not a debug build
or container image. Its checksum matched the local release artifact.

## Measured results

These comparisons use Rust 1.96.1 on Linux x86-64. Rebuilt candidates use the
same source snapshot and updated lockfile; the original production binary with
its previous dependencies is the baseline. All candidates retain the complete
feature set and panic unwinding. Full measurements, hashes, and timing ranges are in
[binary-size-results.json](binary-size-results.json).

| Build | Executable bytes | Smaller than production |
| --- | ---: | ---: |
| Original production build | 17,653,576 | — |
| Deduplicated dependencies, original thin LTO / level 3 | 17,629,512 | 0.14% |
| Full LTO / level 3 | 16,880,648 | 4.38% |
| Full LTO / level 3 + RELR (default Linux x86-64 packaging) | 15,828,080 | 10.34% |
| Full LTO / level s | 12,728,392 | 27.90% |
| Full LTO / level s + RELR | 11,671,728 | 33.88% |
| Full LTO / level z (`compact`) | 11,885,960 | 32.67% |
| Full LTO / level z + RELR | 10,812,912 | 38.75% |

Relocation packing alone removed approximately 1.05 MB from the default
candidate without changing its `.text` or `.rodata` section sizes. Duplicate
dependency removal is useful maintenance but accounts for only 24,064 bytes of
the observed reduction.

All 24 benchmark runs passed, with no dropped usage observations. These are the
medians across three runs per executable, with alternating execution order and
no concurrent comparison builds:

| Build | Proxy request, analytics on | Import 10,000 records | Summary query |
| --- | ---: | ---: | ---: |
| Original production build | 0.743 ms | 1.029 s | 119.48 ms |
| Default level 3 + RELR | 0.712 ms | 1.033 s | 114.90 ms |
| Level s + RELR | 0.766 ms | 1.137 s | 125.17 ms |
| Compact level z + RELR | 0.801 ms | 1.190 s | 123.09 ms |

No material regression was observed for the default build in this limited
synthetic workload; the overlapping HTTP timing ranges do not establish a
speedup. The compact build's medians were about 7.9% slower for proxy requests,
15.7% slower for imports, and 3.0% slower for summary queries than the original
binary. It is available explicitly rather than used for the default release.
These timings include Python HTTP clients/providers, filesystem and scheduler
noise, and do not measure concurrent production streaming throughput.

## Build choices

The default release profile uses full LTO, one codegen unit, stripped symbols,
and optimization level 3. This keeps runtime speed as the compiler's priority.
An optional `compact` profile inherits these settings and uses optimization
level `z` for smaller executables. Its runtime tradeoffs should be evaluated
against the intended workload before deployment.

The direct WebSocket dependency uses the same `tokio-tungstenite` version as
Axum, and the direct Zstandard dependency uses the same version as Reqwest's
compression stack. This avoids linking duplicate Rust implementations without
removing protocols, native TLS roots, or support for older Zstandard streams.
The native Zstandard library remains the same version.

Release builds for Linux x86-64 (glibc) pack relative relocations using RELR and
fold identical functions. `build.rs` adds both flags to the final executable only,
so dependencies, debug builds and other platforms keep their linker settings. It
first links an empty program with the same compiler, flags and linker and, on a
native build, runs it; where either fails (a linker other than lld, or a glibc
older than 2.36) the build keeps the toolchain's defaults and prints a warning.
The release workflow checks the ELF relocation tags and the glibc compatibility
marker, the Docker build checks the tags, and both run the executable.

```sh
cargo build --release --locked
```

To keep the toolchain's own link settings, for example for a binary that must
run on an older glibc than the one it is built on:

```sh
FUSEBOX_PLAIN_LINK=1 cargo build --release --locked
```

For the optional size-first build:

```sh
cargo build --profile compact --locked
target/compact/fusebox --version
```

The compact profile inherits the same link settings. To check a Linux x86-64
build:

```sh
readelf --dynamic target/release/fusebox | grep RELR
readelf --version-info target/release/fusebox | grep GLIBC_ABI_DT_RELR
target/release/fusebox --version
```

Rust's own lld, the default linker for this target since Rust 1.90, takes both
flags; GNU ld packs relocations from 2.38 but can't fold functions. RELR
requires glibc 2.36 or newer; the build environment may impose newer requirements
through other symbols. In particular, the local Debian 13 artifact requires
glibc 2.39, independently of RELR. The Docker build and the Linux release
archives compile on Debian 12, so they run on Debian 12's glibc 2.36. Linux
ARM64, macOS, and Windows do not receive the x86 linker flags.

Panic unwinding remains enabled. Aborting on any task panic would change the
proxy's failure behavior. All providers, compression formats, HTTP/2, SOCKS,
SQLite, analytics, imports, timezones, and dashboard assets remain available.

## Dependency trims and identical-code folding

A second pass on 2026-10-07 removed code that Fusebox linked but did not need,
and folds identical functions at link time. All builds below use rustc 1.96.1,
full LTO and level 3, and pack relative relocations.

| Build | Executable bytes | Smaller than the live build |
| --- | ---: | ---: |
| Live build before this pass | 16,135,696 | — |
| Dependency trims | 15,453,680 | 4.23% |
| Dependency trims + `--icf=all` (default Linux x86-64 packaging) | 15,345,520 | 4.90% |

Measured one change at a time against a 17,192,360-byte build without packed
relocations, the trims were:

| Change | Smaller |
| --- | ---: |
| Log filtering by target instead of `EnvFilter`, which removes a regex engine | 3.02% |
| No Brotli response decoding in the HTTP client | 1.20% |
| `--icf=all` | 0.68% |

- **Logging.** `RUST_LOG` still takes `target=level` directives such as
  `fusebox=debug,hyper=warn`. Span and field filters are no longer supported; a
  value that can't be read logs a warning and uses the default.
- **Brotli.** Upstreams still compress responses with gzip, zstd or deflate,
  which Fusebox advertises instead. Request bodies already accepted only gzip
  and zstd.
- **Identical-code folding.** lld merges functions whose machine code is
  identical. `--icf=safe` saved nothing, because rustc emits no
  address-significance table. Rust does not promise distinct function
  addresses, and the folded build passed all 12 real-process acceptance checks.
  The flag needs lld, which Rust uses by default on x86-64 Linux since 1.90;
  `build.rs` adds it with the relocation flag where a test link shows it works.

Two trims were measured and left out. Switching `idna` to its smaller-sounding
`unicode-rs` backend made this binary 0.7% larger, and dropping clap's colours
and suggestions saved 0.1%.

Sequential loopback benchmarks over two alternating rounds showed no
difference beyond noise:

| Build | Proxy request, analytics off | Proxy request, analytics on | Import 10,000 records | Dashboard query |
| --- | ---: | ---: | ---: | ---: |
| Live build | 0.796 ms | 0.790 ms | 0.744 s | 9.26 ms |
| Dependency trims | 0.802 ms | 0.872 ms | 0.746 s | 9.04 ms |
| Dependency trims + `--icf=all` | 0.847 ms | 0.837 ms | 0.743 s | 9.46 ms |

No run dropped or rejected a usage observation.

`cargo build --release --locked` builds the default Linux x86-64 package
locally; `build.rs` adds both link flags.

## Reproducing comparisons

Keep a copy of the old executable before rebuilding. Use the same compiler,
lockfile, architecture, and source snapshot for each candidate. Cargo's profile
environment overrides allow comparing compiler settings without editing the
manifest:

```sh
CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_OPT_LEVEL=3 \
  cargo build --release --locked
CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_OPT_LEVEL=3 \
  cargo build --release --locked
CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_OPT_LEVEL=s \
  cargo build --release --locked
CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
  cargo build --release --locked
```

Save each resulting executable under a separate name. Once compilation has
finished, compare them sequentially using the synthetic local benchmark:

```sh
python3 scripts/usage-benchmark.py --binary /path/to/candidate
python3 scripts/usage-e2e.py --binary /path/to/candidate
```

The benchmark uses temporary data and a loopback mock provider. It measures
sequential HTTP requests with analytics off/on, imports, and summary queries;
it does not use production credentials or call model providers. Repeat in
alternating order because scheduler, filesystem, and other host load affect
these timings. This does not replace benchmarking real streaming traffic at
production concurrency.

The installed ELF size, compressed download size, container size, and resident
memory are different measurements. Gzip compresses the old production binary to
7,192,201 bytes but does not reduce the installed executable. Embedded dashboard
assets were only 466,956 bytes, so asset compression is a smaller opportunity
than compiled code and relocations.

## Validation

The final native release and compact executables matched the measured sizes
and SHA-256 hashes exactly. Both passed all nine real-process usage/collector
acceptance checks, and ELF inspection confirmed the RELR tags and glibc marker.
The full Rust suite passed 282 tests, dashboard regressions passed 21 tests, and
operational regressions passed 30 tests. Formatting and Clippy with warnings
denied also passed, including all targets of this package on the native host.

The actual Dockerfile built successfully with `rust:1-bookworm` (Rust 1.99.0 at
verification time). Its executable was 15,568,952 bytes; this is a separate
compiler/platform measurement, not an additional measured optimization saving.
The Debian 12 distroless container passed version, `/healthz`, dashboard, and
`/v1/models` checks. Its highest numbered glibc symbol was 2.34, with RELR raising
the effective loader requirement to glibc 2.36. No image was published.

The existing development service was rebuilt, restarted, and health-checked.
Production was inspected read-only and was not redeployed. Other architectures
were not executed locally; their existing CI matrix remains in place.
