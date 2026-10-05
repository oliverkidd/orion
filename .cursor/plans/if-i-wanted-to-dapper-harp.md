# Test the protect-main ruleset with an empty commit

## Context
Repo oliverkidd/orion is public with ruleset `protect-main` (id 24509884) requiring PRs on main; admins bypass. Verify by pushing an empty commit to main without touching the worktree's uncommitted feature work.

## Steps
1. `git fetch origin main`
2. Build the commit without checking out main (keeps the dirty worktree intact):
   `C=$(git commit-tree origin/main^{tree} -p origin/main -m "Test ruleset bypass")`
3. `git push origin $C:refs/heads/main` — expect success with a "Bypassed rule violations" notice.

## Verification
- Push output mentions the bypassed rule.
- `git ls-remote origin main` equals `$C`.
