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

The Rust crate currently parses functions over I64, Bool, nominal sums with
at most one primitive payload per variant, and nominal products with I64/Bool
fields. It supports up to four owned parameters (I64 borrows only), `val` and
`var` bindings, assignment, `while`, integer and boolean literals, unary
negation, `+`, `-`, `*`, `==`, `<`, `if` expressions with block arms, exhaustive
`when` expressions, acyclic direct calls, call-site `&I64` and `&mut I64`
arguments, and a matching tail expression. Recursion is
rejected until task stacks have safe growth limits. Arithmetic wraps at 64
bits in both interpreter and native execution. `phoebe check`, `phoebe ir`,
`phoebe run`, and `phoebe build <file> -o <output>` work on that subset.
Native execution uses Cranelift JIT or Cranelift object emission linked with
the host `cc`; `--interp` selects the reference interpreter. The built
executable runs without the compiler process and prints the I64 returned by
zero-argument `main`. External JIT and interpreter entries also require I64
signatures; calls within Phoebe can pass and return Bool or nominal data.
Primitive-field products also cross calls and returns by flattened scalar
components, without native heap allocation. `docs/abi-shapes.md` records the
current internal calling convention and its missing cost threshold. Core IR
has typed values, conditional branches, variant matches, block parameters, and task-local scalar places for
`var`. Its verifier checks definition uniqueness, dominance, place
initialization, block-argument types, call targets and modes, and return
types. Source spans survive parsing and lowering. `docs/core-ir.md` and
`docs/call-borrows.md` explain the current operations and their evidence
boundary. Borrows are confined to synchronous calls; general reference
lifetimes and regions are not implemented.

This slice establishes the parser → checked IR → interpreter/Cranelift path.
It does not yet implement the ownership, concurrency, module, or release
gates above. The `examples/cost/` files other than scalar `identity` remain
design fixtures, not executable acceptance tests.

## Commands

```sh
cargo run -- run examples/hello.phoebe
cargo run -- run examples/branch.phoebe
cargo run -- run examples/loop.phoebe
cargo run -- run examples/borrow.phoebe
cargo run -- run examples/variant.phoebe
cargo run -- run examples/product.phoebe
cargo run -- run examples/payload.phoebe
cargo run -- run examples/hello.phoebe --interp
cargo run -- ir examples/hello.phoebe
cargo run -- build examples/borrow.phoebe -o /tmp/phoebe-borrow
/tmp/phoebe-borrow
cargo test
python3 formal/check_models.py
```
