# Managed closed-word storage

Status: runtime kernel implemented; Core storage selection and native emission
pending, 2026-11-09. This is one implementation component of Decision 0006,
not a source-visible storage option.

`crates/runtime/src/managed_words.rs` implements an opaque allocation with an
atomic owner count, a length, and contiguous `I64` elements. A successful
allocation owns one reference. Retain adds one owner without copying words;
release destroys the allocation when the final owner leaves. A unique owner
updates an element in place. A shared owner allocates and copies first, then
updates its own new allocation and releases its old reference. Failed allocation
leaves the original owner, all other owners, and their bytes unchanged. The
safe Rust `SharedWords` API and the `phoebe_rt_v1_words_*` native ABI exercise
the same implementation.

The native ABI uses one opaque pointer per owner. `words_new` returns null on
allocation failure. `words_retain` returns zero on success and `-1` on count
overflow. `words_release` accepts null for an empty owner slot. `words_set`
returns zero for a unique write, one for a copy-on-write, `-1` for allocation
failure, and `-2` for an invalid index. A failed `words_set` leaves its pointer
slot unchanged. Generated code must pass only live owners and checked indices;
it must clear a slot when ownership transfers and release every still-owned
slot exactly once. Calls through this ABI are not yet emitted by Phoebe.

The compiler still represents fixed arrays as inline components in the
interpreter, direct JIT, retained JIT, and standalone builds. It records
`copy.value` for their inferred or explicit copies. The runtime kernel does
not change those costs until a checked Core storage plan selects it and all
backends honor that plan. The integration must add source-attributed possible
allocation, retain, release, and copy-on-write sites, including cleanup on
normal return, error, cancellation, and deadlock. It also needs to account for
loop redefinition and branch joins so a suspended frame never leaks or
releases an owner twice. Capability-bearing aggregates cannot retain an
entire record merely because a closed field uses this storage.

Verification so far: four runtime unit tests cover unique and shared writes,
allocation failure atomicity, and raw owner transfer. A C program linked
against the Cargo-built runtime archive exercised new, retain, set, get, and
release. These checks establish the kernel contract, not compiler integration
or performance benefit in Phoebe programs.
