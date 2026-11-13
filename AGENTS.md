## How agents work here

- Work in a worktree of your own, on a new branch from `main`; never in this folder itself.
- When a check fails, fix it and try again, at most 3 rounds.
- When you change what people see, add before and after screenshots.
- Commit in small commits, merge your branch into `main` here (`git merge --no-ff`) and push `main`.
- Then deploy it to the development environment.
- Once your work is merged, remove your worktree and branch.
- Never deploy to production.

## Testing (fast testing)
- After a change, run `fast-test` in the repository folder. It tests only what changed since `main` (commits,
  staged and unstaged edits, new files): quick tests here on The Dragon, heavy ones (VMs, Nix, databases, wide
  changes) on The Beast by itself. `fast-test --dry-run` shows what it would run and why.
- Exit 0 passed, 1 failed, 2 couldn't run (say so; don't run the full suite here instead). When it fails, its output
  ends with the failing step's last lines; the whole log is in `.git/fast-test/`.
- Don't run the full test suite and don't wait for GitHub CI. `fast-test --full` runs everything on The Beast, only
  when someone asks for it.
- A failing test runs once more before it counts as failing (fast-test does this itself). Never skip or weaken a test
  to make it pass.
- Every night The Beast runs the full suite of `main`; failures are reported in the morning.

## Rust version, Windows and macOS, releases

Why it is set up this way, with the numbers: `docs/testing-procedure.md`.

- One Rust version everywhere. `rust-toolchain.toml` names it; The Dragon, The Beast, GitHub's workflows and the
  Docker image all build with it, and `tests/test_rust_version.py` fails when one of them names another. To move to a
  newer Rust, change the version there and in the image names that test lists, in one commit, and fix what the new
  Clippy finds.
- `fast-test` tests on Linux. GitHub's CI workflow (`.github/workflows/ci.yml`) tests Linux, macOS and Windows. It
  doesn't run on a push and you don't wait for it. It runs every night for `main` (started and reported by the nightly
  run above), before a release, on pull requests, and by hand.
- When your change is about something those systems do differently (paths, line endings, file permissions and locks,
  sockets and timeouts, `cfg(windows)` or `cfg(unix)` code), run it there before you merge: push your branch, then
  `gh workflow run ci.yml --ref <branch>` and `gh run watch`. About 5 minutes.
- The Beast's tests run as root, which reads and writes whatever the file permissions say. A test that needs a file
  operation to fail puts a directory where the file goes; it doesn't take write permission away.
- A release is a `vX.Y.Z` tag on `main`. The Release workflow tests the tag on the three systems and publishes only
  when they pass, so there is nothing to wait for before tagging.
