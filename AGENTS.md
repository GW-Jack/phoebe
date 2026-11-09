# Phoebe work tracking

Use the shared WhippleScript issue queue named `phoebe` for work that will span
sessions or produce a substantial change. The issue store is selected by
`WHIPPLESCRIPT_ITEMS_STORE`. If that variable is unset, do not run `whip issue`:
it would create a private store that other sessions cannot see.

Before starting, inspect `whip issue ready phoebe` and
`whip issue list --tracker phoebe --status open`. Create a focused issue if the
work is not represented. Claim it with
`whip issue claim <id> --actor <session> --ttl 2h`; renew while working.
Finish with a commit or pull request reference.
If work stops unfinished, add a note about the remaining work and release the
claim. Do not edit generated `tracker/` views; the shared issue store decides
status and ownership.

The issue queue coordinates work. Accepted language semantics live in
`docs/decisions/`, and the release contract lives in `docs/version-0.0.1.md`.
