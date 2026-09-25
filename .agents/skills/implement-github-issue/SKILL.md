---
name: implement-github-issue
description: Implement a GitHub issue end-to-end: review and self-assign it, require an isolated worktree, test the change, push the branch and open a pull request, and report completion on the issue.
---

# Implement a GitHub Issue

Use this skill with an issue number or URL. Require `gh` authentication and stop if the repository or issue is ambiguous. Self-assignment is part of running this skill: always claim the issue on GitHub before starting work, so everyone can see someone is on it.

A pull request is the deliverable, not an optional extra. Running this skill -- or being handed an issue as a Factory task -- is the authorization to push the issue branch and open a pull request for it. Work that stays on a local branch is invisible to the people who have to review it, and the run's worktree is not a place anyone will look. Merging is the one step that is never yours: a person merges, after review.

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
5. Commit only the intended changes -- never untracked scratch such as `.irrlicht-cowork-probe/` -- then push the branch and open a pull request against `main`. Always; do not stop at a local commit. Link the issue with `Fixes` only when the pull request completes the whole issue; for one phase of it (a "v1", a "Factory side") use `Part of`, so merging does not close an issue that still has work in it. The body says what was built, the checks and their results, and the decisions a reviewer should question:
   ```sh
   BRANCH="$(git branch --show-current)"
   git push -u origin "$BRANCH"
   gh pr create --base main --head "$BRANCH" --title "<summary> (#$ISSUE_NUMBER)" --body "Part of #$ISSUE_NUMBER ..."
   PR_URL="$(gh pr view "$BRANCH" --json url --jq .url)"
   ```
   Never merge it, approve it, or enable auto-merge.
6. Report completion on the issue with the PR link and checks run:
   ```sh
   gh issue comment "$ISSUE" --body "Implemented in $PR_URL. Checks: $CHECKS_RUN"
   ```

Do not open the PR or post the comment until implementation and checks are complete; if the checks cannot be made green, open the PR as a draft (`gh pr create --draft`) and say what is failing rather than leaving the work unpushed. Report the issue number, worktree verification, checks, and PR URL to the requester -- in a Factory run, `PR_URL` goes in the `--result` of `factory task report --status done`.
