# Version 0.0.1 implementation contract

Status: work in progress. This is the implementation target for the first
usable Phoebe release, not a claim that the current repository meets it.

## Release gates

1. Parse and type-check a documented source subset with functions, nominal
   data, exhaustive branching, local bindings, explicit borrows and moves,
   `Result`/`?`, and bounded task/channel operations. Unsupported syntax must
   receive a source-located diagnostic, not an internal panic.
2. Lower that subset to typed, printable Core IR with block parameters,
   explicit places, regions, capability transitions, cleanup edges, and cost
   events. Provide an independently useful IR verifier and interpreter.
3. Generate native code only through Cranelift. Interpreter and native runs
   must agree on representative successful and failing programs, including
   ownership and channel behavior. A release build must be runnable without
   the compiler process.
4. Make task-local storage, borrows across suspension, bounded channels,
   `ReplyTo`, cancellation, failure cleanup, and scope joins safe by
   construction. The runtime must not need a general garbage collector.
5. Support a minimal manifest, pinned dependency graph, modules, and
   incremental interface-based rebuilds. The compiler must remain easy to
   inspect with textual IR, source spans, and isolated pass execution.
6. Run the formal checks, compiler tests, differential tests, and cost
   fixtures. State precisely which invariants are proved, model-checked,
   tested, or still assumptions. Measure compile and runtime behavior rather
   than claiming OCaml-like times or native speed from architecture alone.

These are release gates for a useful first version of the design. Not every
future surface feature or optimization must land in 0.0.1, but memory safety,
scoped concurrency, and the compiler's cost discipline cannot be represented
only by prose at release.

## Current executable slice

The Rust crate currently parses I64 function declarations, up to four I64
parameters, `val` bindings, integer and boolean literals, unary negation,
`+`, `-`, `*`, `==`, `<`, `if` expressions with block arms, acyclic direct
function calls, and an I64 tail expression. Recursion is
rejected until task stacks have safe growth limits. Arithmetic wraps at 64
bits in both interpreter and native execution. `phoebe check`, `phoebe ir`,
and `phoebe run` work on that subset. Native execution uses Cranelift JIT;
`--interp` selects the reference interpreter. Core IR has typed values,
conditional branches, and block parameters. Its verifier checks definition
uniqueness, dominance, block-argument types, call targets and arity, and
return types. Source spans survive parsing and lowering.

This slice establishes the parser → checked IR → interpreter/Cranelift path.
It does not yet implement the ownership, concurrency, module, or release
gates above. The `examples/cost/` files other than scalar `identity` remain
design fixtures, not executable acceptance tests.

## Commands

```sh
cargo run -- run examples/hello.phoebe
cargo run -- run examples/branch.phoebe
cargo run -- run examples/hello.phoebe --interp
cargo run -- ir examples/hello.phoebe
cargo test
python3 formal/check_models.py
```
