use phoebe::{core, native, syntax};
use std::process::Command;

fn checked(source: &str) -> core::Program {
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    core::verify(&program).unwrap();
    program
}

fn assert_backends(program: &core::Program, entry: &str, expected: i64) {
    assert_eq!(program.interpret(entry, &[]).unwrap(), expected);
    assert_eq!(native::run(program, entry, &[]).unwrap(), expected);
}

fn assert_standalone(program: &core::Program, label: &str, expected: i64) {
    let path = std::env::temp_dir().join(format!("phoebe-{label}-{}", std::process::id()));
    native::build_executable(program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
}

#[test]
fn branches_can_transfer_the_same_field_to_one_join_value() {
    let source = r#"
data Bag { label: I64, sender: Sender<I64>, receiver: Receiver<I64> }
fn work(flag: Bool) -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  var bag = Bag(41, sender, receiver);
  val moved = if (flag) { bag.sender } else { bag.sender };
  val label = bag.label;
  moved.close();
  val remaining = bag.receiver;
  remaining.close();
  Result<I64, ResourceError>.Ok(label + 1)
}
fn unwrap(result: Result<I64, ResourceError>) -> I64 = when (result) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
fn main() -> I64 = unwrap(work(true))
fn other() -> I64 = unwrap(work(false))
"#;
    let program = checked(source);
    assert_backends(&program, "main", 42);
    assert_backends(&program, "other", 42);
}

#[test]
fn loop_restores_its_field_before_each_backedge() {
    let source = r#"
data Bag { label: I64, sender: Sender<I64>, receiver: Receiver<I64> }
fn work() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  var bag = Bag(41, sender, receiver);
  var count = 0;
  while (count < 3) {
    val moved = bag.sender;
    yield;
    bag.sender = moved;
    count = count + 1;
  }
  val final_sender = bag.sender;
  val final_receiver = bag.receiver;
  final_sender.close();
  final_receiver.close();
  Result<I64, ResourceError>.Ok(bag.label + count - 2)
}
fn main() -> I64 = when (work()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    let program = checked(source);
    assert_backends(&program, "main", 42);
    assert_standalone(&program, "field-loop", 42);
    let unbalanced = source.replace("    bag.sender = moved;", "    moved.close();");
    let error = core::lower(&syntax::parse(&unbalanced).unwrap()).unwrap_err();
    assert!(
        error
            .message
            .contains("capability ownership differs across incoming edges"),
        "{error:?}"
    );
    assert!(error.span.end > error.span.start, "{error:?}");
}

#[test]
fn owned_product_parameter_returns_one_field_after_closing_sibling() {
    let source = r#"
data Bag { sender: Sender<I64>, receiver: Receiver<I64> }
fn take_sender(bag: Bag) -> Sender<I64> {
  val receiver = bag.receiver;
  receiver.close();
  bag.sender
}
fn work() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val moved = take_sender(Bag(sender, receiver));
  moved.close();
  Result<I64, ResourceError>.Ok(42)
}
fn main() -> I64 = when (work()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    let program = checked(source);
    assert_backends(&program, "main", 42);
    assert_standalone(&program, "field-return", 42);
}

#[test]
fn moved_reply_field_can_cross_a_typed_channel() {
    let source = r#"
data Bag { label: I64, right: ReplyTo<I64> }
data Envelope { label: I64, right: ReplyTo<I64> }
fn work() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<Envelope>(capacity = 1)?;
  val (right, answer) = reply<I64>()?;
  var bag = Bag(42, right);
  val moved = bag.right;
  val sent = sender.send(Envelope(bag.label, moved));
  sender.close();
  when (sent) {
    Send<Envelope>.Sent -> {
      val incoming = receiver.receive();
      receiver.close();
      when (incoming) {
        Receive<Envelope>.Value(envelope) -> {
          val (label, returned_right) = envelope.unpack();
          returned_right.close();
          answer.close();
          Result<I64, ResourceError>.Ok(label)
        },
        Receive<Envelope>.End -> {
          answer.close();
          Result<I64, ResourceError>.Ok(-1)
        }
      }
    },
    Send<Envelope>.Closed(envelope) -> {
      val (label, returned_right) = envelope.unpack();
      returned_right.close();
      receiver.close();
      answer.close();
      Result<I64, ResourceError>.Ok(-2)
    }
  }
}
fn main() -> I64 = when (work()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { -3 },
}
"#;
    let program = checked(source);
    assert_backends(&program, "main", 42);
    assert_standalone(&program, "field-message", 42);
}

#[test]
fn multiword_request_right_can_return_from_a_function() {
    let source = r#"
fn relay(incoming: Request<I64>) -> Request<I64> { yield; incoming }
fn work() -> Result<I64, ResourceError> {
  val (right, answer) = reply<I64>()?;
  val returned = relay(request(42, right));
  val (number, returned_right) = returned.unpack();
  returned_right.close();
  answer.close();
  Result<I64, ResourceError>.Ok(number)
}
fn main() -> I64 = when (work()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    let program = checked(source);
    assert_backends(&program, "main", 42);
    assert_standalone(&program, "request-return", 42);
}
