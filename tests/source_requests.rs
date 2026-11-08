use phoebe::runtime::ScopeLimits;
use phoebe::{core, native, syntax};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_OUTPUT: AtomicUsize = AtomicUsize::new(0);

fn program(source: &str) -> core::Program {
    core::lower(&syntax::parse(source).unwrap()).unwrap()
}

fn assert_engines(source: &str, expected: i64) {
    let program = program(source);
    assert_eq!(program.interpret("main", &[]).unwrap(), expected);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
    let path = std::env::temp_dir().join(format!(
        "phoebe-source-request-{}-{}",
        std::process::id(),
        NEXT_OUTPUT.fetch_add(1, Ordering::Relaxed)
    ));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
}

#[test]
fn source_request_service_transfers_reply_right_through_queue() {
    let source = include_str!("../examples/request_exchange.phoebe");
    let program = program(source);
    assert!(program.to_string().contains("channel.send.request.i64"));
    assert!(program.to_string().contains("channel.receive.request.i64"));
    assert!(program.to_string().contains("request.unpack"));
    assert_engines(source, 42);
    for limits in [
        ScopeLimits {
            channels: 0,
            ..ScopeLimits::default()
        },
        ScopeLimits {
            replies: 0,
            ..ScopeLimits::default()
        },
        ScopeLimits {
            tasks: 1,
            ..ScopeLimits::default()
        },
    ] {
        assert_eq!(
            program.interpret_with_limits("main", &[], limits).unwrap(),
            0
        );
        assert_eq!(
            native::run_with_limits(&program, "main", &[], limits).unwrap(),
            0
        );
    }
}

#[test]
fn source_closed_send_returns_request_for_explicit_cleanup() {
    assert_engines(
        r#"
fn exchange() -> Result<I64, ResourceError> {
  val (sender, receiver) = request_channel<I64>(1)?;
  val (right, answer) = reply<I64>()?;
  receiver.close();
  val outgoing = request(42, right);
  when (sender.send(outgoing)) {
    SendRequest.Sent -> { 0 },
    SendRequest.Closed(rejected) -> { rejected.close(); 0 }
  };
  sender.close();
  when (answer.receive()) {
    Reply<I64>.Value(value) -> { Result<I64, ResourceError>.Ok(0) },
    Reply<I64>.End -> { Result<I64, ResourceError>.Ok(42) }
  }
}
fn main() -> I64 {
  when (exchange()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#,
        42,
    );
}

#[test]
fn lexical_request_cleanup_closes_nested_reply_right() {
    let source = r#"
fn exchange() -> Result<I64, ResourceError> {
  val (right, answer) = reply<I64>()?;
  val outgoing = request(42, right);
  with owned = outgoing {
  }
  when (answer.receive()) {
    Reply<I64>.Value(value) -> { Result<I64, ResourceError>.Ok(0) },
    Reply<I64>.End -> { Result<I64, ResourceError>.Ok(42) }
  }
}
fn main() -> I64 {
  when (exchange()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let lowered = program(source);
    assert!(lowered.to_string().contains("close.request"));
    assert_engines(source, 42);
}

#[test]
fn source_spawn_moves_packed_request_into_direct_child() {
    assert_engines(
        r#"
fn service(incoming: Request<I64>) -> I64 {
  val (value, right) = incoming.unpack();
  right.complete(value);
  0
}
fn exchange() -> Result<I64, ResourceError> {
  val (right, answer) = reply<I64>()?;
  val outgoing = request(42, right);
  spawn service(outgoing)?;
  when (answer.receive()) {
    Reply<I64>.Value(value) -> { Result<I64, ResourceError>.Ok(value) },
    Reply<I64>.End -> { Result<I64, ResourceError>.Ok(0) }
  }
}
fn main() -> I64 {
  when (exchange()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#,
        42,
    );
}

#[test]
fn rejected_request_cannot_be_implicitly_discarded() {
    let source = r#"
fn exchange() -> Result<I64, ResourceError> {
  val (sender, receiver) = request_channel<I64>(1)?;
  val (right, answer) = reply<I64>()?;
  receiver.close();
  val outgoing = request(42, right);
  when (sender.send(outgoing)) {
    SendRequest.Sent -> { 0 },
    SendRequest.Closed(rejected) -> { 0 }
  };
  sender.close();
  answer.close();
  Result<I64, ResourceError>.Ok(0)
}
"#;
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(
        error
            .message
            .contains("capability ownership differs across incoming edges"),
        "{}",
        error.message
    );
}

#[test]
fn request_outcomes_require_both_arms_and_owned_cleanup() {
    let source = r#"
fn run() -> Result<I64, ResourceError> {
  val (sender, receiver) = request_channel<I64>(1)?;
  when (receiver.receive()) {
    ReceiveRequest.Value(incoming) -> { incoming.close(); Result<I64, ResourceError>.Ok(0) }
  }
}
"#;
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(
        error.message.contains("non-exhaustive when"),
        "{}",
        error.message
    );
}
