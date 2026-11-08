# Concrete request source surface

The concrete 0.0.1 request carries an `I64` payload and one `ReplyTo<I64>`
right. Its Core representation is two words with one owned capability slot.
The source surface exposes that ownership without admitting arbitrary
capabilities inside ordinary products or sum values.

Source spelling:

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
};
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

`Request<I64>`, `RequestSender<I64>`, and `RequestReceiver<I64>` may be owned
function parameters and spawn arguments. `with owned = request { ... }`
closes a request that remains live at lexical exit. Creation uses the same
`Result<_, ResourceError>` propagation contract as scalar channels.

`SendRequest` and `ReceiveRequest` are **control-flow outcomes**, rather than
ordinary storable data. They are recognized only by an exhaustive `when`
whose subject is a direct request endpoint `send` or `receive` call. Lowering
creates Core successor blocks directly. This preserves the existing rule that
general `Data` fields and variant payloads cannot contain capabilities. The
verifier rejects missing arms and live rejected or received requests at arm
exit.

The concrete `I64` request syntax is implemented. General request payloads,
capabilities inside arbitrary aggregates, and storable request outcomes remain
open. The example in `examples/request_exchange.phoebe` runs through the
interpreter, JIT, and standalone executable.
