---
name: implement-github-issue
description: Implement a GitHub issue end-to-end: review and self-assign it, require an isolated worktree, test the change, open a pull request, and report completion on the issue.
---

# Implement a GitHub Issue

Use this skill with an issue number or URL. Require `gh` authentication and stop if the repository or issue is ambiguous. Self-assignment is part of running this skill: always claim the issue on GitHub before starting work, so everyone can see someone is on it. Treat PR creation and issue comments as external writes: proceed only when the request explicitly authorizes them.

```sh
ISSUE="$1"
ISSUE_NUMBER="$(gh issue view "$ISSUE" --json number --jq .number)"
```

1. Load the issue and its discussion, then claim it by assigning yourself so others can see it is being worked on:
   ```sh
   gh issue view "$ISSUE" --comments
   gh issue edit "$ISSUE" --add-assignee @me
   ```
2. Before changing files, **prove this checkout is not the primary worktree**. Stop rather than work in the primary checkout:
   ```sh
   current="$(git rev-parse --show-toplevel)"
   primary="$(git worktree list --porcelain | awk '/^worktree / {print substr($0, 10); exit}')"
   test "$(cd "$current" && pwd -P)" != "$(cd "$primary" && pwd -P)" || {
     echo 'Refusing to work in the primary checkout; create or switch to a separate worktree.' >&2
     exit 1
   }
   git status --short --branch
   ```
3. Read relevant code and repository guidance, create or use an issue-specific branch, then implement the issue. Keep unrelated existing changes intact.
4. Review the diff (`git diff --check`) and run the relevant formatter, tests, and required checks. Fix failures caused by the change; record the commands as `CHECKS_RUN`.
5. Commit only the intended changes and create a PR that links the issue:
   ```sh
   gh pr create --fill --body "Fixes #$ISSUE_NUMBER"
   ```
6. Get the PR URL, then report completion on the issue with the PR link and checks run:
   ```sh
   gh issue comment "$ISSUE" --body "Implemented in $PR_URL. Checks: $CHECKS_RUN"
   ```

Do not create the PR or post the comment until implementation and checks are complete. Report the issue number, worktree verification, checks, and PR URL to the requester.
