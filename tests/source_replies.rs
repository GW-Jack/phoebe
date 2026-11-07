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
        "phoebe-source-reply-{}-{}",
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
fn source_reply_moves_to_child_and_wakes_parent() {
    let source = include_str!("../examples/reply_exchange.phoebe");
    let program = program(source);
    assert!(program.to_string().contains("reply.new.i64"));
    assert!(program.to_string().contains("reply.complete.i64"));
    assert!(program.to_string().contains("reply.receive.i64"));
    assert_engines(source, 42);
    let unavailable = ScopeLimits {
        replies: 0,
        ..ScopeLimits::default()
    };
    assert_eq!(
        program
            .interpret_with_limits("main", &[], unavailable)
            .unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], unavailable).unwrap(),
        0
    );
}

#[test]
fn canceled_receiver_returns_closed_value() {
    assert_engines(
        r#"
fn run() -> Result<I64, ResourceError> {
  val (right, receiver) = reply<I64>()?;
  receiver.close();
  val outcome = right.complete(42);
  when (outcome) {
    Complete<I64>.Delivered -> { Result<I64, ResourceError>.Ok(0) },
    Complete<I64>.Closed(value) -> { Result<I64, ResourceError>.Ok(value) }
  }
}
fn main() -> I64 {
  when (run()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#,
        42,
    );
}

#[test]
fn lexical_close_of_reply_right_wakes_receiver_with_end() {
    assert_engines(
        r#"
fn run() -> Result<I64, ResourceError> {
  val (right, receiver) = reply<I64>()?;
  with owned = right {
    // Leaving this block closes the unspent right.
  }
  val outcome = receiver.receive();
  when (outcome) {
    Reply<I64>.Value(value) -> { Result<I64, ResourceError>.Ok(value) },
    Reply<I64>.End -> { Result<I64, ResourceError>.Ok(42) }
  }
}
fn main() -> I64 {
  when (run()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#,
        42,
    );
}

#[test]
fn failed_later_allocation_closes_both_reply_capabilities() {
    let source = r#"
fn run() -> Result<I64, ResourceError> {
  val (right, receiver) = reply<I64>()?;
  val (sender, input) = channel<I64>(capacity = 1)?;
  sender.close();
  input.close();
  right.close();
  receiver.close();
  Result<I64, ResourceError>.Ok(42)
}

fn main() -> I64 {
  when (run()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 7 }
  }
}
"#;
    let program = program(source);
    assert!(program.to_string().contains("close.reply"));
    let limits = ScopeLimits {
        channels: 0,
        ..ScopeLimits::default()
    };
    assert_eq!(
        program.interpret_with_limits("main", &[], limits).unwrap(),
        7
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], limits).unwrap(),
        7
    );
}

#[test]
fn right_cannot_be_completed_twice() {
    let source = r#"
fn run() -> Result<I64, ResourceError> {
  val (right, receiver) = reply<I64>()?;
  val first = right.complete(1);
  val second = right.complete(2);
  receiver.close();
  Result<I64, ResourceError>.Ok(0)
}
"#;
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(
        error.message.contains("right") || error.message.contains("capability"),
        "{error:?}"
    );
}
