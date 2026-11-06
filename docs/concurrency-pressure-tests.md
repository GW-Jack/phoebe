# Concurrency pressure tests

Status: design exploration, 2026-11-06. The syntax below is illustrative.
`! proposed` marks syntax or behavior not yet fully specified in draft 0.4.
These examples compare APIs, not measured performance or formal proofs.

## 1. Request/reply across an independently owned service

A service owns its state. A client wants a one-shot response.

```text
data Query { Get(key: Key, reply: ReplyTo<Option<Value>>) }

fn cache(inbox: Receiver<Query>) {
  var entries = SlotMap<Key, Value>()
  for (query in inbox) {
    when (query) {
      Get(key, reply) -> {
        reply.complete(entries.get(key)) // consumes one-shot capability
      }
    }
  }
  inbox.close()
}
```

Draft 0.4 permits the one-shot `ReplyTo` to move through a request message.
The earlier closed-values-only rule required request IDs and a shared reply
channel with routing code for this basic interaction.

An actor API makes the interaction read naturally:

```text
actor Cache {
  var entries = SlotMap<Key, Value>()
  on Get(key: Key, reply: ReplyTo<Option<Value>>) { // ! proposed
    reply.complete(entries.get(key))
  }
}
```

It has the same underlying capability-transfer requirement. `ReplyTo<T>` is a
one-shot, nonblocking capability; `complete` consumes it and reports if the
caller already stopped waiting. The queue owns it until delivery. Discard or
receiver failure closes it exactly once. The bounded
`formal/reply_capability.maude` model covers one such right and request; the
general sendability rule remains an unproved design obligation.

## 2. A chat room with dynamic membership and slow subscribers

```text
data RoomCommand {
  Join(user: UserId, sink: Sender<RoomEvent>) // ! proposed handle transfer
  Leave(user: UserId)
  Say(user: UserId, text: String)
}

fn room(commands: Receiver<RoomCommand>) {
  var members = SlotMap<UserId, Sender<RoomEvent>>()
  for (command in commands) {
    when (command) {
      Join(user, sink) -> members.insert(user, sink)
      Leave(user) -> members.remove(user).close()
      Say(user, text) -> {
        val slow = List<UserId>()
        for ((id, sink) in members.entries()) {
          when (sink.try_send(Said(user, text))) { // ! proposed
            Sent -> { }
            Full -> slow.push(id)
            Closed -> slow.push(id)
          }
        }
        for (id in slow) members.remove(id).close()
      }
    }
  }
  members.close_all()
  commands.close()
}
```

The actor version replaces the `for`/`when` dispatch with `on Join`, `on
Leave`, and `on Say`; its state and send policy are otherwise the same. A
blocking send to one slow subscriber stalls a single-owner room task. An actor
handler cannot park without weakening run-to-completion semantics. Both styles
need an explicit policy: drop, disconnect, reserve bounded output per member,
or defer the fan-out to independent tasks. `try_send` must return a result that
the program handles rather than silently losing a message.

Membership also illustrates the no-heap cost: `SlotMap` needs safe reuse;
actor syntax does not reclaim deleted members by itself.

## 3. Session with idle timeout and late events

Task form:

```text
fn session(events: Receiver<SessionEvent>, timer: Timer) {
  var state = SessionState()
  var epoch: U64 = 0
  for (event in events) {
    when (event) {
      Input(bytes) -> {
        state.accept(bytes)
        timer.cancel_pending()
        epoch += 1
        timer.after(30s, Expire(epoch))
      }
      Expire(tag) -> if (tag == epoch) { break }
    }
  }
  timer.cancel()
  events.close()
}
```

Actor form puts `Input` and `Expire` in separate handlers and keeps `state` and
`epoch` as actor fields. Both must ignore stale timer events and define who
owns the timer's send right while a callback is pending. Scope exit must
cancel or drain outstanding callbacks before freeing the session's storage.
Actors reduce dispatch code here; they do not remove lifecycle races.

## 4. Parallel worker pool with ordered completion

```text
scope {
  val (submit, jobs) = channel<Job>(64)
  val (emit, results) = channel<NumberedResult>(64)
  for (_ in 0..<4) spawn worker(jobs.fork(), emit.fork())
  jobs.close()
  emit.close()

  for ((sequence, job) in input.enumerated()) submit.send(Job(sequence, job))
  submit.close()

  var reorder = SlotMap<Sequence, Result>()
  for (result in results) reorder.insert(result.sequence, result.value)
  results.close()
  yield_in_order(reorder)
}
```

Channels and a scope fit homogeneous fan-out/fan-in work. A worker-actor
version needs an address for each worker, dispatch policy, completion routing,
and failure tracking. The actor style offers little here. Neither style gives
ordered results automatically; the reorder buffer needs boundedness or
backpressure if input is unbounded.

## 5. Two sequential remote steps

A suspended task can keep the control flow local:

```text
fn handle(req: Request) -> Reply {
  val identity = await auth.check(req.token) // ! proposed ask/await API
  val record = await store.load(identity.id)
  Reply(record)
}
```

An actor handler cannot wait while preserving run-to-completion. It sends an
auth request, returns, receives `AuthDone`, sends a store request, returns,
then receives `StoreDone`. That requires a correlation ID and retained
per-request state. The actor may serve many requests between steps, which is
useful, but the surface is a state machine. Phoebe would need generated
continuations or a separate task to recover the sequential syntax. That task
needs retained suspension storage even if actor handlers do not.

## 6. Read-heavy shared configuration

A single task or actor that owns a map serializes every lookup. Sending a
whole snapshot to each reader copies it. Replicating a snapshot per worker
trades memory and update cost for read throughput. Sharing one immutable
region across workers would need a lifetime mechanism absent from draft 0.4;
a managed heap or scoped immutable-region ownership could provide one. Actor
syntax alone does not solve the read bottleneck.

## Findings and design gates

| Workload | Simpler surface | Unresolved mechanism |
| --- | --- | --- |
| Independent request/reply | Actor call, once replies can move | Linear capability transfer in messages |
| Stateful session/timer | Actor handlers | Timer ownership, late callbacks, failure |
| Parallel worker pool | Scoped tasks/channels | Ordered, bounded fan-in |
| Chained remote operations | Suspended task | Suspension frame and ask/reply API |
| Read-heavy shared state | Neither alone | Snapshot replication or shared immutable lifetime |

The first design gate, **move-only communication capabilities in messages**,
is adopted in draft 0.4 and modeled for one `ReplyTo`. General handle transfer,
including the `Sender<RoomEvent>` in the chat-room sketch, needs a further
model. The remaining gates are a precise bounded-mailbox policy for sends
from actor handlers and an ownership and cost model for long-lived maps and
read-only snapshots.
