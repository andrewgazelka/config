---
name: ci
description: Push the current branch, watch its CI, and fix failures until it is green. Say "merge" to also enable auto-merge once it passes.
---

# CI

1. Run the repo's own checks first (`just check`, `make check`, `nix flake check`,
   or whatever its CI runs). Local failures are faster to fix than remote ones.
2. Push: `git push -u origin HEAD`.
3. Find the PR with `gh pr view`. If there is none, create it with
   `gh pr create --fill`, adding `--draft` unless merging was asked for.
4. Watch: `gh pr checks --watch --fail-fast`.
5. On failure, read the log (`gh run view <run-id> --log-failed`), fix the
   cause, commit (amend if it belongs to the same change), push, and watch again.
6. Stop and report when:
   - CI is green,
   - the fix needs a design change,
   - the failure is external (outage, flaky network) and a retry failed too, or
   - the same error came back three times.
7. If merging was asked for: `gh pr merge --auto`. Once it merges, switch to
   the default branch and delete the local branch.

Report what you fixed and the final CI state.
