# Cost acceptance examples

`identity.phoebe`, `product_identity.phoebe`, `unit_identity.phoebe`, and
`empty_scope.phoebe` compile today. Their executable acceptance tests check
zero Phoebe cost events and no runtime calls in Cranelift IR. The other files
use draft syntax and cannot compile yet. The snippets below describe the
checked IR shape required by the non-pessimization contract.
`cost.*` denotes a logical event with source and reason metadata. Ordinary
loads, stores, branches, and scalar arithmetic are omitted from the snippets.

## `identity.phoebe`

```text
entry(x: I64):
  return x
```

No `cost.*` event, runtime call, retained task frame, or variable-size copy.

## `product_identity.phoebe`

```text
entry(pair: Pair):
  return pair
```

The two-word product crosses the native function boundary as two scalar
components. Its identity body has one logical Core move for ownership, but
no native computation, cost event, runtime call, or stack slot. The acceptance
test checks the emitted Cranelift IR and interpreter/native agreement for a
call through that body.

## `unit_identity.phoebe`

The `Unit` parameter and result have zero native ABI components. The direct
Cranelift function has an empty signature and a bare return, with no scalar
constant, stack slot, runtime call, or cost event.

## `borrow_length.phoebe`

```text
entry(xs: Ref<List<I64>, region>):
  n = list.length xs
  return n
```

No `cost.copy.bytes`, `cost.storage.grow`, reference-count operation, or
runtime call. The reference's root is checked statically.

## `build_one.phoebe`

```text
entry(result_place: Place<List<I64>>):
  list.construct result_place
  cost.storage.grow result_place, 1  // if capacity must grow
  list.push result_place, 7
  return.place result_place
```

The result is built in the caller-provided destination. Growth is conditional
on capacity, so the ledger records the *site*, not a promise of a physical
allocation on every execution. There is no full-size copy on return.

## `checked_index.phoebe`

```text
entry(xs: Ref<List<I64>, region>, i: USize):
  cost.check.bounds i, list.length(xs)
  value = list.load xs, i
  return value
```

One logical bounds check guards the unconstrained index. A failed check has
the language's specified safe failure behavior, to be fixed with numeric and
failure semantics. No second check is introduced by lowering the load.

## `range_loop.phoebe`

```text
entry(xs: Ref<List<I64>, region>):
  n = list.length xs
  loop(i, total):
    if i == n: return total
    proof i < n from loop range and unchanged xs.length
    value = list.load.proven xs, i
    jump loop(i + 1, total + value)
```

There is no `cost.check.bounds` in the loop body. The range proof is an IR
fact verified before lowering. The loop test remains; it is not a bounds
check inserted for the indexed access.

## `empty_scope.phoebe`

```text
entry():
  return 7
```

No `cost.task.create`, `cost.task.park`, scheduler call, or task frame. The
empty scope has no runtime effect.

## `send_value.phoebe`

```text
entry(tx: Sender<List<U8>>, msg: Ref<List<U8>, region>):
  result = channel.reserve_or_closed tx, list.length(msg)
  if result is Closed:
    channel.close tx
    return Error(Closed)
  if result is Parked:
    suspend with tx and msg retained in this task
  cost.copy.bytes msg -> reserved_queue_slot  // once on successful send
  channel.publish reserved_queue_slot
  channel.close tx
  return Ok(Unit)
```

The buffered success path has one source-to-queue variable-size copy and no
full-size temporary. The closed path has zero copies. Parking preserves the
source reference in the sending task; resumption must recheck channel state
before copying. This sketch leaves the precise reservation and wake protocol
to the runtime design. A later receive-side copy is outside this function.
