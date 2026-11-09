# Decision 0003: Language feature baseline

Status: accepted, 2026-11-06. Exact grammar, ABI shape classes, standard
library APIs, and implementation proofs remain open.
Decision 0006 supersedes the ownership and copy rule below for ordinary
closed data; communication capabilities retain explicit transfer and cleanup.

Phoebe starts with nominal product and sum types, exhaustive matching,
parametric generics, coherent traits, `Option` and `Result`, and no implicit
numeric or error conversions. `val` fixes a binding but permits mutation of
its owned object through exclusive access; `var` permits rebinding.

Function boundaries make ownership explicit: `T` is owning (or a small
`Copy` value), `&T` is a shared borrow, and `&mut T` is an exclusive borrow.
Large independent copies use `copy(x)`. A fixed set of operator symbols maps
to named traits. Operators cannot hide suspension, implicit conversions, or
unreported ownership effects.

`?` propagates an existing `Err` along a normal return edge. A lexical `with`
closes still-owned built-in capabilities on normal exits, including `?`.
Runtime failure closes its owned built-in capabilities without invoking user
destructors. A rejected send returns its message, including any capabilities,
to the caller for explicit handling.

Modules have explicit imports and exports, default-private declarations, and
no executable import-time initialization. Packages use a manifest and pinned
lockfile. Interface fingerprints drive downstream invalidation; they do not
assert semantic compatibility or a stable native ABI.

Initially, generic functions compile once per compatible ABI shape, using
dictionaries for trait operations not known in that body. Concrete functions
use direct calls. A runtime-optimized compilation mode may later specialize
selected functions, informed by measurement; specialization is not a default
source-language promise.

`../language-features.md` develops these policies and lists remaining design
questions. This decision does not claim that the compiler or type system is
implemented.
