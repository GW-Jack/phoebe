# Phoebe

Phoebe is an early language design for native programs with task-local memory,
lightweight tasks, and value-passing channels. This repository currently contains
a draft language specification and a deliberately small executable formal model.
It does **not** contain a compiler or runtime.

## Current milestone

`docs/spec.md` states the proposed language contract and marks open decisions.
`docs/non-pessimization.md` defines cost events and example IR acceptance
criteria for the future compiler.
`docs/compiler-architecture.md` records the research-informed proof and
hackability plan.
`docs/decisions/0001-compiler-principles.md` records the accepted compiler
principles. `docs/pony-actors.md` explores an actor model without GC; its
semantics remain undecided.
`formal/core.maude` explores one task with nested regions and a reference.
`formal/two_task.maude` explores parent and child tasks, explicit handle fork,
transfer at spawn, channel delivery, close, and child failure. Both have
one-slot queues and finite state spaces. See `formal/README.md` for the exact
models and their limits. These checks do not prove the full language, compiler,
or native runtime safe.

Run the checks with:

```sh
python3 formal/check_models.py
```

The script checks the expected safety-search and witness results from Maude.
