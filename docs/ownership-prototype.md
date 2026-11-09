# Ownership semantics prototype

Status: isolated executable experiment plus compiler slices,
2026-11-09. Decision 0006 adopts the source model. Closed nested nominal
field and name reads now preserve their parent in source lowering and Core
verification, including closed siblings of resource-bearing records. Closed
values survive ordinary `T` calls. Two-field products with a later direct `?`
now use checked local destination construction with exact early-error cleanup
of initialized rights. Inferred sharing and partial right-transfer remain
prototype-only.

## Question

Can one value-oriented source model use inferred loans, caller-owned result
destinations, and path-sensitive borrow checking without asking programmers to
choose among several memory-management APIs?

The candidate model gives closed data logical value semantics: a function can
read it, receive its sole ownership, or retain another immutable owner when
both caller and callee need it. The compiler chooses the storage action from
use and liveness, and reports its cost. Communication rights remain linear:
they cannot be implicitly retained or copied. Mutation of shared closed data
must preserve the old value, possibly by copying on write. No source-level
destination or regular expression is exposed.

Decision 0006 makes that value behavior part of the language contract. The
current compiler realizes it with fixed-size component copies on name use and
CFG-aware last-use transfers for SSA values and `var` places. Checked Core
now selects shared storage for copied 256-word arrays in direct programs,
including a bounded call-scoped borrow for read-only callees. The interpreter,
direct JIT, and standalone build use that plan and report possible physical
cost sites. Retained frames and arrays in data still use inline components;
`managed-storage.md` records the selection boundary and measurements.

## Executable artifacts

- `examples/ownership_prototype.rs` is an isolated Rust model. It chooses
  `Borrow`, `Move`, or `Retain` for a finite sequence of uses, rejects retain
  of a capability, models write-once result destinations with partial cleanup,
  and checks overlap of finite field paths using regex derivatives. It now
  includes a nested field store: closed siblings can be retained while a
  capability field moves, loans block only overlapping updates, and a failed
  fieldwise result closes its moved right once. The regex search has a stated
  maximum depth; it is not a production checker.
- `examples/ownership_prototype_baseline.phoebe` is the same multi-call shape
  under the earlier explicit-ownership syntax. It has two `copy(current)` sites.
- `examples/ownership_value.phoebe` removes those copies under Decision 0006.
  Its nine-word result still uses the existing caller-owned return buffer.

Run the isolated model with:

```sh
rustc --edition=2024 --test examples/ownership_prototype.rs -o /tmp/phoebe-ownership-proto
/tmp/phoebe-ownership-proto
```

Run the current compiler baseline with:

```sh
cargo run --quiet -- cost examples/ownership_value.phoebe
cargo run --quiet -- clif examples/ownership_value.phoebe
cargo run --quiet -- run examples/ownership_value.phoebe --interp
cargo run --quiet -- run examples/ownership_value.phoebe
cargo run --quiet -- build examples/ownership_value.phoebe -o /tmp/phoebe-ownership-value
/tmp/phoebe-ownership-value
```

Observed on macOS AArch64: eleven isolated tests passed; the original baseline
returned 42 in interpreter, JIT, and standalone execution. After integration,
the no-copy source example also returned 42 in all three engines. The baseline
now reports two explicit `copy.value Record` sites; its final use moves. The new
example reports inferred use sites and no explicit copy sites. The CLIF for
`make_wide` had an `sret` argument
and stores for all nine words in a caller-owned 72-byte slot. No Phoebe
reference-count operation was implemented or measured. The isolated test
demonstrates that an O(1) retain can defer an O(n) copy until mutation of a
shared 4096-byte value; it is not a performance result for Phoebe. The
simulated suspension checks a boxed destination address but does not run the
Phoebe scheduler.

## Mechanism and obligations

| Mechanism | What the prototype establishes | Required Phoebe integration |
| --- | --- | --- |
| Inferred use | A read can borrow; a last owning use moves; an owning use with a later use retains closed data; a capability cannot be retained. | Source value semantics, CFG-aware last-use moves, and checked borrow/retain/transfer selection for bare 256-word arrays in direct programs are implemented across interpreter and native. Extend storage to retained frames and closed fields, with exact failure cleanup. |
| Destination construction | A write-once field cannot be filled twice; a result with a missing field cannot be published; dropping an incomplete destination drops initialized fields. | Checked local destinations cover nonempty products with a direct `?` in a later field or an owned right in an earlier field, including nested `?`, multiple initialized rights, and suspension. Other products retain SSA construction. Extend to caller storage, sums, and owned closed storage cleanup. |
| Regex paths | Named left/right fields are disjoint; a call result summarized as any subfield conservatively overlaps either; nested paths can be distinguished. | Add field loans and path roots to Core, implement exact terminating regex/automaton checks and CFG joins, prove or validate the checker, check actual native address and cleanup rules. |

## Field extension

The nested field store tests a capability-bearing `Record` with `payload`,
`sender`, and `meta.{label,version}`. It retains only `payload`, moves `sender`
out once, then updates `payload`. The earlier payload remains unchanged and
the update copies its 32 bytes. This refines the earlier whole-aggregate rule:
having a right somewhere in an aggregate does not prevent its closed sibling
from sharing. It does prevent retaining the whole aggregate as one closed
value. The right does not get an implicit second owner.

A shared loan of `meta.label` coexists with an exclusive loan of
`meta.version` and an update of `payload`. It blocks updates of `meta.label`;
a loan of `meta` or the whole record conflicts with either nested loan. A
parent loan also blocks moving `sender`. The model uses descendant path
languages so `meta` overlaps `meta.label`, while distinct siblings remain
disjoint.

On failed result construction, a caller-owned destination containing a
retained payload and the moved right closes that right once; the source has
an empty right slot and does not close it again. This is model behavior. The
actual Phoebe Core now preserves closed nominal name and field reads, scalar
field loans, and top-level right-field moves from owned `val` and `var`
products. The source and Core now also permit reinitializing an empty top-level
right field of a mutable product, restoring whole-record transfer once all
rights are present. A scoped `with` product can move top-level rights, and its
exit closes only the fields it still owns. The prototype's deeper nested
partial moves remain separate. Its `Right` closes via a Rust `Drop`
counter, not the Phoebe runtime.

The current source rule permits a partially moved owned record to read closed
siblings and move other live right fields. Whole-record transfer requires all
rights to be present. Reinitializing an empty top-level field restores a
complete record for an owning call. The checker still needs broader validation
of per-field capability state at CFG joins, and a public API cannot make transfer
depend on a callee's private body.

## Compatibility decisions before source adoption

1. **Public meaning.** If a function accepts closed `T` by value, callers must
   know whether they may use their value afterward without depending on the
   callee's private body. Inferred borrow/move/retain should be an optimization
   under stable value semantics, with cost differences visible in the ledger.
2. **Cost contract.** Implicit retain/release and copy-on-write revise the
   current no-implicit-copy rule. A retain is not a byte copy, but a later
   write may be. Define which operations are allowed to cause each event.
3. **Capabilities.** A value containing a right cannot take the closed-data
   retain fallback. It must still be consumed, returned, or explicitly forked
   where the right permits it. Failure paths retain exact ownership.
4. **Partial results.** A `Result` selects one initialized variant. On `?`,
   allocation failure, cancellation, or deadlock, initialized fields and
   capabilities need exact cleanup. A partially initialized result cannot
   cross a task or channel boundary.
5. **References.** The regex result relies on references being rooted paths,
   not fields stored inside data. It does not infer the cost or permissibility
   of an owning call. A conservative `.*` call summary can reject disjoint
   operations unless a stable, more precise interface summary is available.

## Next integration gate

Replace remaining eager inferred component copies with borrow/retain
selection and a real immutable shared representation, with source-attributed
retain, release, and copy-on-write costs. Add typed destination initialization
to Core for a two-field product, including success and `?` cleanup. Then add
fieldwise right transfer and named-field loans using an exact path checker,
with differential interpreter/native tests. Do not generalize to collections,
recursive data, or cross-task references until these cases preserve the same
value semantics and cleanup behavior.

Research sources: [automatic borrow inference in Morphic](https://www.languagesforsyste.ms/publication/fully-automatic-type-inference/),
[safe destination passing](https://arxiv.org/abs/2601.08529), and
[regex-based borrow tracking in Move](https://verse-lab.org/papers/regex-borrows-oopsla26.pdf).
