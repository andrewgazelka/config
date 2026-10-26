---
name: pr-comments
description: Work through the unresolved review comments on the current branch's PR, fixing the simple ones and replying to the rest. Optionally filtered by reviewer, file or topic.
---

# PR comments

List the unresolved comments with `gh pr view --comments` (or the review
threads API), applying any filter the user gave.

For each one:

- **A clear, small fix** (typo, naming, formatting, a missing doc, a small
  refactor): make it as its own commit, push, and reply with the commit hash
  ("Fixed in abc1234"). If there's a reasonable reading, just do it.
- **Anything that needs a decision** (design, scope, unclear intent, or you
  disagree): don't change code. Reply explaining the trade-off, and say in
  the reply that it was written by an agent.

Finish with a list of what was fixed and what is waiting on the author.
