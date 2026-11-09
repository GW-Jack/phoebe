# Phoebe work tracking

Phoebe has its own WhippleScript issue store. Use `scripts/issue` from any
Phoebe worktree, even when `WHIPPLESCRIPT_ITEMS_STORE` points to another
project's store in your shell. The wrapper resolves the shared Git directory
and uses the primary checkout's store, lock, and ordered event log at
`issues/issue-log.json`. This gives concurrent worktrees one issue sequence.
Commit the event log to preserve WS-1, WS-2, and later issue numbers across
fresh clones. Do not edit the event log or generated `tracker/` views by hand.

Before work that will span sessions or produce a substantial change, inspect
`scripts/issue ready phoebe` and `scripts/issue list --tracker phoebe --status open`.
Create a focused issue if the work is not represented, then claim it with
`scripts/issue claim <id> --actor <session> --ttl 2h`. Renew while working.
Finish with a commit or pull request reference. If work stops unfinished,
add a note about what remains and release the claim. Commit changes to
`issues/issue-log.json` alongside the related work.

The issue queue coordinates work. Accepted language semantics live in
`docs/decisions/`, and the release contract lives in `docs/version-0.0.1.md`.

## Trunk development

`main` is the integration branch. Start each substantial change from the
current `origin/main` in a short-lived worktree and claim its issue first.
Keep independent agents in separate worktrees; do not edit another agent's
uncommitted files. Push a focused pull request, rebase when trunk advances,
and merge it after its required checks pass. Delete the topic branch after
merge. Publish completed work frequently so later agents start from the same
trunk. Use the shared issue queue to record ownership and remaining work;
do not use a long-lived feature branch as a second backlog.
