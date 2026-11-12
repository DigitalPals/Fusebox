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
