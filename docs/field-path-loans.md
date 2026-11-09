# Field-path loan language

Status: exact path checker, checked scalar field loans, and field-address
lowering are integrated. Parent product references and partial capability
records remain open under WS-4 and WS-3.

A path is a finite sequence of product-field indices relative to one storage
root. The empty path names the whole root. A loan at path `p` covers `p` and
every descendant. A path language is a finite union of these subtrees:

```text
L(P) = union over p in P of { p · s | s in Σ* }
```

The compiler stores only prefix-minimal paths in `P`. Adding `p` discards any
existing path that starts with `p`; if an existing path is a prefix of `p`,
the new path is redundant. A CFG or call summary may combine alternatives by
union. This is a restricted regular-language representation chosen for the
field access patterns that Decision 0006 currently requires. It does not
parse regular expressions and does not impose a search-depth limit.

For two path languages, overlap is exact: `L(P) ∩ L(Q)` is nonempty iff some
`p` in `P` is a prefix of some `q` in `Q`, or vice versa. In the forward
direction, a shared word has both prefixes, so the prefixes are comparable.
In the reverse direction, the longer comparable prefix itself is a witness.
This also proves that the algorithm terminates after at most `|P| × |Q|`
finite prefix comparisons. A shared loan conflicts with an exclusive one only
when their languages overlap; two shared loans may overlap.

`crates/core/src/path_language.rs` implements this algebra. A separate bounded
enumeration test compares the checker with explicit language membership for
all pairs of unions over binary paths up to depth two. That test checks the
implementation for a finite domain; the prefix argument above establishes
the general property for the represented language. Neither is a proof that
source lowering chooses the correct storage roots or that native addresses
match those paths.

The verifier now carries paths on lexical and call loans. Source `&record.meta.id`
and `&mut record.meta.id` work for scalar `I64` and `Bool` leaves of `var`
products, including across suspension. The interpreter resolves the nested
value path; direct and retained Cranelift code use the same checked flattened
component offset. A shared borrow from a `val` product projects its closed
field into a scalar temporary, preserving the immutable parent. Call arguments
may borrow disjoint siblings, while a whole-place mutation or overlapping
field borrow conflicts with an active loan. Exact loan state must agree at CFG
joins. No reference may cross a task boundary.

The present reference types are scalar. Borrowing an entire product parent is
therefore unsupported, even though the path checker would detect its overlap
with a child. Mutable places containing rights are separately restricted, so
this implementation does not yet provide field loans in partial resource
records. Those source and Core surfaces, and a fieldwise capability transfer,
remain acceptance work for WS-4 and WS-3.
