# How Fusebox is tested, and why

Decided on 2026-11-13 from the measurements below. The rules agents follow are in `AGENTS.md`; this is the reasoning.

## The procedure

| When | What runs | Where | Who waits |
|---|---|---|---|
| After every change | `fast-test`: the checks of what changed | The Dragon; a wide change runs the whole suite on The Beast | The agent: 6 s to 1 minute |
| Every night, for `main` | The whole suite on Linux, and GitHub's CI on Linux, macOS and Windows | The Beast and GitHub | Nobody; one report in the morning |
| Before a release | GitHub's CI for the tag, beside the release builds | GitHub | Nobody; nothing is published unless it passes |
| A change about Windows or macOS | GitHub's CI for the branch, started by hand | GitHub | The agent: about 5 minutes |
| A pull request | GitHub's CI | GitHub | Nobody here |

One Rust version is used everywhere (`rust-toolchain.toml`), so a check that passes on The Dragon doesn't fail
elsewhere because a newer Clippy runs there.

## What it replaced

Until 2026-11-12 every push to `main` ran GitHub's CI on Linux, macOS and Windows, and the agent waited for all
three before finishing or deploying. From 2026-11-12 `fast-test` took over and the CI workflow was switched off, which
left three holes: nothing tested Windows or macOS although releases ship binaries for both, The Beast and The Dragon
ran different Rust versions, and one test could not pass on The Beast.

## The numbers

GitHub's CI, 173 runs from 2026-10-08 to 2026-11-12 (`gh run list --workflow ci.yml`):

- A run that passed took 4m53s (median); one in ten took more than 5m55s. Windows decides it: 5m39s, against 3m47s
  for macOS and 3m20s for Linux. That wait came about 5 times a day.
- 10 runs failed (6%), from 7 causes. Each was followed by a fix:
  - 3 were faults in Fusebox: the usage database was still open or locked after shutdown returned (twice, found on
    macOS and on a slow Linux runner), and a deleted account directory was handled wrongly (found on Windows).
  - 4 were tests that only worked on Linux: line endings in the golden files, file locks and timing on Windows, a
    refused connection that macOS reports as a timeout.
  - 2 of the runs also failed on a lint of a Rust version newer than The Dragon's.
- 5 of the 7 causes showed only on Windows or macOS, in 7 of the 173 runs.
- It used about 13 runner-minutes per run, free for a public repository.

`fast-test`, the same month's 170 changes on `main` sorted by what it runs for them:

| Change | Share | What runs | Wait |
|---|---|---|---|
| No Rust source | 24% | Dashboard or script tests, or nothing | Seconds |
| Rust source, not wide | 41% | Formatting, Clippy and the changed modules' tests on The Dragon | 41 s in a new worktree, 6 to 8 s after that |
| Wide (`Cargo.toml`, `main.rs`, `state.rs`, …) | 35% | The whole suite on The Beast | 55 s to 1 minute |

Measured on 2026-11-13 with The Dragon nearly idle; a busy Dragon has taken a new worktree's first run to 104 s.
Measured the same morning before this change, the same three were 66 to 76 s, 8 to 10 s, and 78 to 80 s: Clippy now runs beside the tests in a
build folder of its own, and The Beast's test build optimises the dependencies, which shortens the test run there
from 34 s to 22 s. Optimised dependencies stay out of `Cargo.toml` and off The Dragon: they add about 30 s to every
new worktree's first build.

Until 2026-11-13 no full run of a real commit had passed on The Beast. The night's run stopped at a lint of Rust
1.100 that The Dragon's 1.99 didn't have, and after that fix every run failed on
`pending_entry_precedes_dispatch_and_failed_save_never_dispatches`: it forced a failed save by taking write permission
from a directory, and The Beast's tests run as root, which writes there anyway. Neither was a fault in Fusebox.

## Why this and not the old procedure

- The old wait was about 5 minutes per push, and most pushes couldn't fail on what it added: the Linux part is what
  `fast-test` runs in under a minute, and the Windows and macOS part found something in 7 of 173 runs.
- What it found there matters, though: a release ships Windows and macOS binaries. So those tests stay, at the two
  points where nobody waits for them: every night, and before anything is published.
- A night's delay costs little. A failure names the commits since the last night that passed (about 5), and nothing
  reaches users in between: development and production run Linux, and a release can't be published past a failure.
- A change that is clearly about Windows or macOS can still be run there first, by hand.

## What each part catches and misses

| | `fast-test` | Nightly run | Release |
|---|---|---|---|
| Formatting, Clippy | Yes | Yes | No (checked when the change was made) |
| Tests of changed modules | Yes | Yes | Yes |
| Tests of unchanged modules | Wide changes only | Yes | Yes |
| Windows, macOS | No | Yes | Yes |
| Running as root | Wide changes only | Yes | No |
| A newer Rust than the pinned one | No | No | No |

The last row is on purpose. A newer Rust arrives when someone changes the version in `rust-toolchain.toml` and the
image names beside it, in a commit of its own that fixes what the new Clippy finds. The Client compatibility
workflow looks for a newer stable Rust every day and, on Mondays, opens one issue (label `rust-version`) while there
is one; the issue closes once `main` builds with it.
