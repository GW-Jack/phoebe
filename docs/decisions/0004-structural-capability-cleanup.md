# Decision 0004: Structural cleanup of communication capabilities

Status: accepted, 2026-11-06. Exact aggregate move syntax and runtime cleanup
tables remain open.

`send(message)` returns `Sent` after an atomic enqueue or `Closed(message)`
without taking ownership. The returned message remains intact and owned by
the caller, including every linear communication capability it contains.

The compiler derives `close_caps()` structurally for a type containing such
capabilities. It consumes the value, visits only its active variant and owned
fields in reverse declaration order, and closes nested communication
capabilities exactly once. It runs no
user code and cannot fail. For a capability-free type it does no capability
work. Closing a queued message on discard, cleaning task-owned values on
failure, and explicit `message.close_caps()` use the same structural rule.

`with` may own a built-in capability or an aggregate containing them. On a
normal exit, including `?`, it closes any value it still owns. A move or an
explicit `close_caps()` consumes that ownership. The ordinary `send` API
returns a rejected message so the caller can recover it. A named
`send_or_close` convenience operation may consume and structurally close a
rejected message before returning a simple `SendClosed` error; its discard is
explicit in the source name and checked IR.

The checked IR tracks ownership of each capability-bearing value and the
active fields after destructuring. It must generate cleanup on every normal
exit and identify live capabilities at each runtime failure point. Queue
discard and failure cleanup use compiler-provided shape information, with no
user destructor. A proof obligation is that every capability has exactly one
owner before and after enqueue, rejection, dequeue, discard, destructuring,
and cleanup, and is closed at most once.

The existing `formal/reply_capability.maude` model explores one `ReplyTo` in
one request. It does not establish structural cleanup for arbitrary nested
types or multiple capabilities; that needs a separate model and typed IR
argument.
