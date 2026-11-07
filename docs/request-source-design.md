# Request source design (draft)

The concrete 0.0.1 request carries an `I64` payload and one `ReplyTo<I64>`
right. Its Core representation is two words with one owned capability slot.
The source surface should expose that ownership without admitting arbitrary
capabilities inside ordinary products or sum values.

Proposed source spelling:

```phoebe
// Illustrative fragments inside a scope with explicit endpoint cleanup.
val (sender, receiver) = request_channel<I64>(1)?;
val (right, answer) = reply<I64>()?;
when (sender.send(request(42, right))) {
  SendRequest.Sent -> { 0 },
  SendRequest.Closed(rejected) -> {
    rejected.close();
    0
  }
}
when (receiver.receive()) {
  ReceiveRequest.Value(incoming) -> {
    val (value, reply_to) = incoming.unpack();
    reply_to.complete(value);
    0
  },
  ReceiveRequest.End -> { 0 }
}
```

`request(value, right)` consumes `right`. `unpack()` consumes the request and
binds its two components. `close()` consumes a request and closes its nested
right. A successful send transfers the request to the queue. A pending send
keeps it in the task frame. A closed send binds the intact request in the
closed arm. Receive binds a request only on `Value`; `End` binds nothing.

`SendRequest` and `ReceiveRequest` are **control-flow outcomes**, rather than
ordinary storable data. The parser may reuse `when` syntax, but lowering must
create Core successor blocks directly. This preserves the existing rule that
general `Data` fields and variant payloads cannot contain capabilities. The
verifier should reject attempts to omit either arm or leave a rejected or
received request live at arm exit.

This is a design draft, not implemented source syntax. Before enabling it,
test parse and diagnostic behavior, cross-backend delivery and closed-send
recovery, pending send and receive, lexical cleanup, and resource limits.
