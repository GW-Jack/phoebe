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
principles. `docs/decisions/0002-concurrency-core.md` adopts scoped tasks and
channels with move-only communication capabilities in messages, and actors as
an abstraction over that core. `docs/pony-actors.md` explores the actor details.
`docs/decisions/0003-language-baseline.md` records the agreed type, borrow,
error, module, and ABI-shape generic policies; `docs/language-features.md`
develops them and lists the next questions.
`docs/decisions/0004-structural-capability-cleanup.md` specifies cleanup of
communication capabilities inside rejected or discarded messages.
`docs/concurrency-pressure-tests.md` compares scoped tasks and actors on
stateful and asynchronous workloads and records the resulting design gates.
`formal/core.maude` explores one task with nested regions and a reference.
`formal/two_task.maude` explores parent and child tasks, explicit handle fork,
transfer at spawn, channel delivery, close, and child failure.
`formal/reply_capability.maude` explores a one-shot reply right moving through
a request, cancellation, discard, and failure. All three models have finite
state spaces. See `formal/README.md` for the exact
models and their limits. These checks do not prove the full language, compiler,
or native runtime safe.

Run the checks with:

```sh
python3 formal/check_models.py
```

The script checks the expected safety-search and witness results from Maude.
