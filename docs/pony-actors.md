# Pony-inspired actors without garbage collection — design exploration

Status: open design, 2026-11-06. This document does not change Phoebe's
existing task/channel contract.

## What Pony actually provides

Pony actors receive asynchronous behavior calls and execute one behavior at a
time. Reference capabilities constrain what can cross actor boundaries:
isolated mutable data (`iso`), immutable data (`val`), and opaque identity
(`tag`) are sendable. Pony uses a per-actor heap and garbage collector; its
runtime also detects quiescent actor cycles. Thus Pony's concurrency model
cannot be copied wholesale into a no-GC Phoebe runtime.

Sources: [Pony actors](https://tutorial.ponylang.io/types/actors),
[reference capabilities](https://tutorial.ponylang.io/reference-capabilities/reference-capabilities),
[runtime and GC](https://www.ponylang.io/faq/runtime/),
[Orca paper](https://www.ponylang.io/media/papers/orca_gc_and_type_system_co-design_for_actor_languages.pdf).

## Proposed first adaptation

Keep Phoebe's task/channel core. An actor is a scoped task with private
persistent state, one mailbox receiver, and an `ActorRef<Message>` sender
capability. Sending to an actor is asynchronous. Its handler processes one
message to completion before the next handler runs. Other actors may run on
other scheduler threads. An actor handler cannot wait on its own mailbox or
perform a blocking operation; work needing suspension runs in an ordinary
task and sends a result back later.

```text
actor Counter {
  var count: I64 = 0

  on Add(n: I64) {
    count += n
  }
}

scope {
  val counter = actor Counter()
  counter.send(Add(1))
  counter.close()
}
```

The syntax is illustrative. `ActorRef` behaves like a sender capability:
ownership is explicit, `fork` creates another send right, and `close` consumes
one. An actor stops after the last send right closes and its mailbox drains;
the enclosing scope waits for it. The initial ordering guarantee is FIFO per
sender to one actor. Stronger causal ordering needs a separate semantics and
runtime cost argument. Mailboxes are bounded; backpressure may park the
sending task, while actor handlers themselves run to completion.

This design gives Pony-like sequential ownership of actor state while
retaining Phoebe's goroutine-style tasks for independent blocking work. It
reuses the task scheduler and channel machinery rather than creating a second
runtime protocol. The actor surface could later become primary if examples
show that its constraints fit typical programs. With bounded mailboxes, a
send from an actor handler must return `Full` or schedule a later continuation
rather than parking inside the handler; an ordinary task may park on send.

## No-GC memory rules to establish

- A handler's temporary region is reclaimed after the handler returns.
- Persistent actor state lives in a region owned by that actor. The whole
  region is reclaimed when the actor terminates. A long-lived actor that
  repeatedly replaces state needs explicit storage reuse or a safe pool;
  bulk reclamation at actor exit alone is insufficient.
- V1 messages are closed values copied into the mailbox and then into the
  actor's task-local storage. No sender-owned reference enters actor state.
- A future isolated-region transfer could avoid large copies only if the
  sender gives up all aliases, including inner references, and ownership of
  the region moves atomically. That is a new proof and allocation design,
  not implied by Pony's `iso` capability.
- Shared immutable object references (`val` in Pony) would need a lifetime
  mechanism such as scoped ownership or reference counting. V1 has no such
  sharing; immutable messages are copied.
- Actor references in actor state can form cycles. Scope-based explicit close
  may leave such programs waiting forever; no actor cycle detector is promised.

## Questions for a bounded model

1. Can `ActorRef` forks, sends, and closes preserve one live owner per handle
   and never deliver after the actor's state is freed?
2. Does actor exit require the last sender to close, the mailbox to drain,
   and no handler to be running? What happens on handler failure?
3. Can a bounded mailbox park a sender without deadlocking an actor handler?
4. What storage-reuse rule supports a long-lived actor without GC, while
   preserving references into its state?

The first three fit a finite Maude model. The fourth needs a concrete memory
representation and likely a separate ownership proof. Region-based memory
management research shows that no-GC inference can be sound, while also
documenting how lifetime shape affects programs:
[Tofte and Talpin, region-based memory management](https://www.sciencedirect.com/science/article/pii/S0890540196926139).
