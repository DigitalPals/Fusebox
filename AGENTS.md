<!-- whombat:rules start · written by Whombat: change these rules in Whombat’s Repositories, not here -->
## How agents work here

These rules come first. Where the rest of this file says otherwise, follow these. Only the person can allow more, for one Whombat job; firm limits still apply.

- Work in a worktree of your own, on a new branch from `main`; never in this folder itself.
- While you work, run the quick checks for what you changed. The full check runs in CI after you push.
- Never run two full checks at the same time on this server.
- When a test fails, run it once more before you treat it as failing.
- When a check fails, fix it and try again, at most 3 rounds. Never skip or weaken tests to make them pass.
- When you change what people see, add before and after screenshots.
- Wait for every CI check to finish and pass before you finish or deploy.
- Commit in small commits, merge your branch into `main` here (`git merge --no-ff`) and push `main`.
- Then deploy it to the development environment.
- Once your work is merged, remove your worktree and branch.
- Never deploy to production.

<!-- whombat:rules {"v":1,"rules":{"workIn":"worktree","testing":"focused_then_full","checkWhere":"ci","ciWait":"always","finish":"deploy","mergeHow":"local","review":false,"tidy":true,"mainFirst":false,"retryFlaky":true,"oneCheckAtATime":true,"pictures":true,"fixRounds":3,"checkCommand":null,"quickCommand":null,"deployCommand":null,"notes":null}} -->
<!-- whombat:rules end -->
