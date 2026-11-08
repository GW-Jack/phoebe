# Language features — draft 0.5 design sketch

Status: baseline accepted by Decision 0003, 2026-11-06; the listed details
remain design questions. Most syntax is illustrative; the executable subset
is tracked in `version-0.0.1.md`. Decision 0002 governs the concurrency core.

## Types and values

Types are nominal. `data` defines product and sum types; `when` must cover
every variant. Generic type and function parameters are explicit in public
signatures, while local expression types may be inferred. Tuples and function
types are built in. There is no class inheritance, implicit numeric
conversion, or general subtyping. `Option<T>` represents absence;
`Result<T, E>` represents expected failure. Neither is implicitly converted
to the other.
`Unit` is the single value for a computation without a useful result. Its
native ABI shape is empty; a `Unit` function may omit a final expression.
Scoped task bodies may return `Unit` instead of a dummy integer.
Concrete `Option<T>` applications are executable for finite primitive and
nominal payloads. They use `None` and `Some(T)` and require exhaustive `when`;
generic function definitions remain a separate compiler milestone.

`trait` describes a small set of named operations. Implementations obey a
coherence rule: one implementation for a given trait and nominal type in the
resolved package graph. Initially, a package may implement a trait only if it
defines that trait or the type. There are no overlapping implementations,
specialization, or trait inheritance in the first version. These restrictions
make method lookup local and deterministic and keep generic interfaces small.

```text
pub data ParseError { Missing(name: String), Invalid(text: String) }
pub data Config { port: U16, label: String }

pub trait Show {
  fn show(self: &Self) -> String
}

impl Show for Config {
  fn show(self: &Config) -> String {
    "Config(port = ${self.port})"
  }
}
```

`val` fixes a binding; `var` permits reassignment. Either may own mutable
task-local storage. Mutation needs an exclusive access path; a `val` owner may
mutate its object without rebinding the name. A deeply immutable value is a
separate type property, not inferred from `val`. Closed values can be copied
across tasks, while mutable task-local references cannot cross tasks.

## Borrow and ownership syntax

Function parameters show the ownership transition:

| Parameter | Meaning |
| --- | --- |
| `x: T` | Callee owns `T`; a linear or large value moves. Small `Copy` values copy. |
| `x: &T` | Read-only borrow rooted in the caller's region. |
| `x: &mut T` | Exclusive mutable borrow rooted in the caller's region. |

Call sites write `&x` or `&mut x` for borrows. `copy(x)` requests an independent
copy and is required for a large closed value if the caller must keep it after
an owning call. Borrow lifetimes are inferred from use and constrained by
regions; no source lifetime parameter is required initially. A borrow may
survive a task's suspension if its root remains in that task's retained frame.
No borrow may enter a child task or channel message. Distinct named fields may
be borrowed separately; dynamic indices are conservatively treated as
possibly overlapping until a cheap local proof establishes otherwise.

```text
fn rename(config: &mut Config, label: &String) {
  config.label = copy(label)
}

val config = Config(8000, "old")
val next = "new"
rename(&mut config, &next) // legal: config is not rebound
```

This supersedes the earlier shorthand that same-task calls implicitly borrow
variable-size values. Explicit parameter modes are the source rule;
the checked IR still records the same borrow, move, and copy operations.

## Operator policy

Only a fixed, documented set of operators may be overloaded. Each maps to a
named trait method with the same ownership and failure rules as a normal call:
for example `+` to `Add.add`, `==` to `Eq.eq`, and `<` to `Compare.lt`.
Implementations cannot define new operator tokens or precedence. There are no
implicit conversions in operator lookup. The compiler reports the selected
method and any required copy or move in diagnostics and IR dumps. An operator
cannot hide a parking operation; fallible operations should use named methods
returning `Result` initially. Index syntax is deferred until its bounds,
borrow, and failure behavior are specified.
The current executable subset has `==` and `!=` for I64, Bool, and payload-free
sum types, plus `<`, `<=`, `>`, and `>=` for I64. These comparisons preserve
left-to-right evaluation and lower to direct Core and Cranelift comparisons.
`!`, `&&`, and `||` are built-in Bool operators, not overloadable trait
calls. The binary operators evaluate the right operand only when needed;
`?` in that operand
therefore runs only on that path. Their precedence is `||`, then `&&`, then
comparison, arithmetic, and unary operators. `!` lowers to one typed Core
operation; the binary operators lower to branches with a Bool block parameter.

## Errors and capability cleanup

Expected failure is a return value. `expr?` inside a function returning
`Result<T, E>` unwraps `Ok` or returns the same `Err<E>` after normal-exit
cleanup. It does not convert `Option` to `Result`, choose a new error type, or
catch task failure. Conversions use an explicit operation such as
`option.ok_or(ParseError.Missing("port"))?` or `error.map_err(...)`.

Linear values must be consumed, returned, transferred, or covered by a lexical
cleanup owner on every normal exit. `with name = value { ... }` owns a built-in
communication capability or an aggregate containing them until it is moved or
closed. On fall-through, `return`, or `?`, the compiler structurally closes
any still-owned capabilities in reverse lexical order. An explicit `close()`
or `close_caps()` consumes the owned value and suppresses that cleanup. A move
transfers responsibility to the destination. The generated cleanup visits
only active, owned fields; it invokes no user destructor and cannot fail.
The executable subset also accepts `with (a = sender, b = receiver) { ... }`
for several bare capabilities. It nests the existing single-owner cleanup
regions, preserving left-to-right binding and reverse-order cleanup. Owned
aggregates and `close_caps()` remain design targets.

```text
fn publish(out: Sender<Count>, input: &String)
    -> Result<Unit, PublishError> {
  with sender = out {
    val count = parse_count(input).map_err(PublishError.Parse)?
    when (sender.send(Count(count))) {
      Sent -> Ok(())
      Closed(_) -> Err(PublishError.OutputClosed)
    }
  }
}
```

If a rejected request send returns a message containing a `ReplyTo` right,
that right must be closed or otherwise consumed before `?` may return.
`map_err` alone does not silently discard it. The compiler derives
`close_caps()` for an aggregate, recursively closing its communication
capabilities without running user code. The ordinary send returns the intact
message; a named `send_or_close` convenience operation may explicitly discard
it on rejection.

```text
val (reply_to, reply_from) = reply<Value>()
with receiver = reply_from {
  when (service.send(Get(key, reply_to))) {
    Sent -> receiver.receive()
    Closed(request) -> {
      request.close_caps() // closes the returned ReplyTo
      Err(ServiceClosed)
    }
  }
}
```

The compiler must track the active variant and owned fields if an aggregate
is destructured. For failure cleanup, each suspension or failure point must
identify its live capabilities. Capability-free messages need no capability
cleanup call. The first implementation may restrict partial moves from a
capability-bearing aggregate if precise per-field tracking proves costly;
whole-value moves and complete destructuring are enough for the examples so
far.

Task failure is distinct from `Err`: the runtime closes owned communication
capabilities, including those nested in aggregates, wakes peers, and reports
failure to the scope. It does not run user code or user destructors. This
means `with` has a static normal-exit lowering plus compiler-provided runtime
failure cleanup, not an arbitrary unwind stack.

## Modules and package builds

One source file declares one module. Imports and exported names are explicit;
declarations are private by default. Modules have no executable initialization
at import time. Public type signatures and visibility make a compact interface
that can be checked without compiling a dependency's body. The import graph
is acyclic initially; mutually recursive functions stay within one module.

```text
// src/config.ph
module app.config
import std.text.parse_u16
pub fn parse(input: &String) -> Result<Config, ParseError> { ... }
```

A package has `phoebe.toml`, a committed `phoebe.lock`, `src/`, and `tests/`.
The manifest names direct dependencies and version requirements. The lockfile
pins exact versions and content hashes; builds do not resolve versions while
compiling source. The compiler builds a deterministic module dependency graph
and caches each module's checked interface and native object by input hash.
Private body edits do not rebuild dependents when their needed interface and
codegen facts are unchanged. Cross-module inlining is off by default.

An interface fingerprint includes exported signatures, visibility, trait
implementations, and representation facts needed by dependents. It is a build
invalidation key, not a promise of semantic compatibility. Package version
policy can follow semantic versioning, but the compiler can only report
detectable source changes, such as removed exports or changed signatures.
Phoebe makes no stable native ABI promise in its first version. A dependency
upgrade is an explicit lockfile operation; existing builds remain reproducible
from the pinned graph.

`core` contains primitive types, `Option`, `Result`, and fundamental traits.
`std` is split into opt-in modules for collections, text, channels, files,
networking, and process interaction. FFI resides behind an explicit unsafe
boundary. Library signatures should reveal allocation, copies, parking, and
capability transfer; compiler cost events record their source reasons.

## Generic compilation policy

Concrete functions use direct calls. A generic function gets a canonical
body shared by compatible ABI shapes, with type/operation dictionaries for
facts that the body cannot know statically. A trait call through a dictionary
is an explicit `call.indirect` event attributed to the source trait bound.
This avoids making every type instantiation a new compiler job. A later
runtime-optimized compilation mode may specialize selected hot generic
functions after measurement; specialization cannot change behavior or
ownership effects. The initial mode does not expose a specialization
annotation. Exact shape classes and the dictionary ABI remain open.

This policy favors predictable incremental compilation. It does not claim
that every generic call is as fast as a hand-written concrete call. The cost
ledger and native benchmarks must expose that tradeoff.

## IR obligations added by this sketch

The checked IR needs explicit `borrow_shared`, `borrow_exclusive`, `move`,
`copy`, `close`, and `return_err` operations. Every control-flow edge leaving
a lexical cleanup region carries the required closes. For an owning place
`p`, its abstract state is exactly one of `Owned`, `Moved`, or `Closed` at each
program point; active shared or exclusive loans are tracked separately.
Joins reject incompatible states unless control flow resolves them
explicitly. A mutable borrow excludes all
overlapping accesses until its last use. A `?` edge must preserve the same
ownership and region invariants as an ordinary return edge.

The build system needs a separate interface hash and implementation hash.
The former controls downstream invalidation; the latter controls rebuilding
the defining module's object. A proof of interface extraction should show
that equal interface hashes imply equal facts observed by a downstream type
check and code generation, subject to collision-resistant hashing of a
canonical serialization. Native ABI stability across compiler versions is
not part of this obligation.

## Questions to test next

1. How should partial moves and complete destructuring of an aggregate with
   multiple capabilities appear in the checked IR and failure cleanup table?
2. Which ABI shapes can share a body, and which generic collection operations
   need a runtime-optimized mode to reach their performance goals?
3. Can field-sensitive borrows remain a small, local analysis when a task
   suspends inside a nested region?
4. Which public representation facts must be fingerprinted so private edits
   never produce stale downstream machine code?
