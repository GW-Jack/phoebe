# Managed closed-word storage

Status: checked selection and direct native emission for copied 256-word arrays
implemented, 2026-11-09. Retained frames and arrays embedded in data still use
the inline ABI. This is a hidden implementation choice under Decision 0006.

`crates/runtime/src/managed_words.rs` implements an allocation with an atomic
owner count and contiguous `I64` payload. Retain adds an owner without copying
words. Release destroys storage after the final owner leaves. A unique owner
writes in place; a shared owner allocates and copies before writing. Failed
allocation leaves all existing owners and bytes unchanged. The safe
`SharedWords` API and versioned `phoebe_rt_v1_words_*` C ABI share this kernel.

The V1 generated-code ABI fixes the aligned offset of the immutable payload
after the header. Checked constant-index native reads load directly from that
payload while an owner or call-scoped borrow keeps it live. Mutation uses
`words_set`, which updates a unique buffer or performs copy-on-write.

`StoragePlan` selects managed storage only for a 256-word array type with a
checked copy site, and only when every function has direct return/jump/branch/match
control and no data declaration embeds an array. This includes an early `?`
return through a `Result` match. Other programs use inline
components in all backends. The narrow threshold follows the local
benchmark: 32-word sharing lost badly and 128 words was near parity. The
planner also recognizes one read-only array parameter whose uses are limited
to constant-index and length reads. The call borrows that buffer; an
immediately preceding inferred copy used only for that call does not retain. An
explicit `copy(x)` allocates independent storage. The
interpreter uses a bounded borrow marker into its caller frame. Direct JIT and
standalone code pass the same pointer without another owner.

Direct native code gives each owned managed value and place a zeroed slot.
Move, take, call, jump, and return transfer ownership by clearing the source
slot. Replacement and frame exit release nonempty slots. A private write uses
`words_set`, so another live value keeps its prior contents. Retained poll
frames, suspension, channel traffic, sums/products containing arrays, and
other unselected control paths use the inline ABI. They do not hold managed
array owners across cancellation or deadlock. Native allocation failure or
retain overflow currently traps; recoverable error reporting and cleanup on
that trap remain open before managed storage is selected for fallible task
paths.

`phoebe cost` reports static `storage.allocate`, `storage.borrow`,
`storage.retain`, `storage.release`, `storage.copy_words`, and `storage.cow` sites for selected
arrays. Release sites may find an empty slot, and a COW site copies only when
another owner exists. These are possible source-attributed costs, not dynamic
counts. Inline arrays still report `copy.value`. Selection does not yet
cover closed fields within capability-bearing records; their rights stay
linear.

Verification includes runtime allocation-failure tests, a C archive-link
exercise, interpreter and direct native value tests, cross-function returns,
branch and early-error cleanup, zero live managed allocations after JIT completion, and a
retained 256-word inline fallback across interpreter, JIT, and standalone.
On this machine's AArch64 JIT, `cargo run --example storage_bench` ran
200,000 calls to a function making ten read-only calls with a 256-word array.
Two repeated local runs measured about 47 ms with managed borrowing versus
72-73 ms with inline copying, with identical checksums. This is a bounded
local workload, not a general speedup claim; the benchmark's unreachable
yielding function forces the inline fallback for the same reachable functions.
