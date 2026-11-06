# Phoebe

Phoebe is an early language design for native programs with task-local memory,
lightweight tasks, and value-passing channels. This repository currently contains
a draft language specification and a deliberately small executable formal model.
It does **not** contain a compiler or runtime.

## Current milestone

`docs/spec.md` states the proposed language contract and marks open decisions.
`formal/core.maude` explores a bounded abstract machine with nested regions, one
reference, one sender, one receiver, a one-slot queue, and task failure. The model
checks safety invariants for all reachable states in this bounded machine. It
does not prove the full language, compiler, or native runtime safe.

Run the checks with:

```sh
maude -no-banner formal/check.maude
```

The expected result is `No solution.` for each bad-state search, followed by
reachable witnesses for send/receive, channel end, and failure.
