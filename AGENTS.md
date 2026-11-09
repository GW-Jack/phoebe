# Phoebe work tracking

Phoebe has its own WhippleScript issue store. Use `scripts/issue` from this
checkout, even when `WHIPPLESCRIPT_ITEMS_STORE` points to another project's
store in your shell. The wrapper selects Phoebe's store and keeps the ordered
event log in `issues/issue-log.json` current. This preserves WS-1, WS-2, and
later issue numbers across fresh checkouts. Do not edit the event log or the
generated `tracker/` views by hand.

Before work that will span sessions or produce a substantial change, inspect
`scripts/issue ready phoebe` and `scripts/issue list --tracker phoebe --status open`.
Create a focused issue if the work is not represented, then claim it with
`scripts/issue claim <id> --actor <session> --ttl 2h`. Renew while working.
Finish with a commit or pull request reference. If work stops unfinished,
add a note about what remains and release the claim. Commit changes to
`issues/issue-log.json` alongside the related work.

The issue queue coordinates work. Accepted language semantics live in
`docs/decisions/`, and the release contract lives in `docs/version-0.0.1.md`.
