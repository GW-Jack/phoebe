use std::process::Command;

use phoebe::runtime::ScopeLimits;
use phoebe::{core, native, syntax};

fn all_backends(source: &str, label: &str, expected: i64) {
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), expected);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
    let path =
        std::env::temp_dir().join(format!("phoebe-with-group-{label}-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
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
fn grouped_channel_rights_close_on_normal_and_question_error_exits() {
    let source = include_str!("../examples/with_pair.phoebe");
    all_backends(source, "channel", 42);
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("fail", &[]).unwrap(), 0);
    assert_eq!(native::run(&program, "fail", &[]).unwrap(), 0);
}

#[test]
fn grouped_statement_closes_reply_and_request_channel_rights() {
    let source = r#"
fn acquire() -> Result<I64, ResourceError> {
  val (right, reply_receiver) = reply<I64>()?;
  with (send = right, receive = reply_receiver) {}
  val (sender, receiver) = request_channel<I64>(capacity = 1)?;
  with (output = sender, input = receiver) {}
  Result<I64, ResourceError>.Ok(42)
}

fn main() -> I64 = when (acquire()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    all_backends(source, "reply-request", 42);
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    for limits in [
        ScopeLimits {
            replies: 0,
            ..ScopeLimits::default()
        },
        ScopeLimits {
            channels: 0,
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
fn grouped_rights_track_spawn_transfer_and_pending_receive() {
    let source = r#"
fn worker(sender: Sender<I64>) -> Unit {
  val sent = sender.send(42);
  sender.close();
}
fn exchange() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val value = with (output = sender, input = receiver) {
    spawn worker(output)?;
    when (input.receive()) {
      Receive<I64>.Value(message) -> { message },
      Receive<I64>.End -> { 0 },
    }
  };
  Result<I64, ResourceError>.Ok(value)
}
fn main() -> I64 = when (exchange()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    all_backends(source, "spawn-receive", 42);
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let limits = ScopeLimits {
        tasks: 1,
        ..ScopeLimits::default()
    };
    assert_eq!(
        program.interpret_with_limits("main", &[], limits).unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], limits).unwrap(),
        0
    );
}

#[test]
fn grouped_binding_alias_closes_once_and_rejects_duplicate_names() {
    let source = r#"
fn consume(sender: Sender<I64>) -> I64 {
  with (outer = sender, inner = outer) {}
  42
}
fn main() -> I64 = 42
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let closes = program.functions[0]
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .filter(|i| matches!(i.operation, core::Operation::CloseHandle(_)))
        .count();
    assert_eq!(closes, 1);
    let duplicate = source.replace("inner = outer", "outer = outer");
    let error = core::lower(&syntax::parse(&duplicate).unwrap()).unwrap_err();
    assert!(error.message.contains("duplicate local outer"), "{error:?}");

    let escape = "fn bad(sender: Sender<I64>, receiver: Receiver<I64>) -> I64 { \
        with (output = sender, input = receiver) { output }; 0 }";
    let error = core::lower(&syntax::parse(escape).unwrap()).unwrap_err();
    assert!(
        error.message.contains("cannot return its endpoint"),
        "{error:?}"
    );

    let wrong_type = "fn bad(sender: Sender<I64>) -> I64 { \
        with (output = sender, number = 42) {} 0 }";
    let error = core::lower(&syntax::parse(wrong_type).unwrap()).unwrap_err();
    assert!(
        error
            .message
            .contains("requires a communication capability")
    );
    assert!(error.span.end <= wrong_type.len());
}

#[test]
fn grouped_binding_depth_is_source_bounded() {
    let bindings = (0..70)
        .map(|i| format!("c{i} = 1"))
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!("fn main() -> I64 {{ with ({bindings}) {{}} 42 }}");
    let error = syntax::parse(&source).unwrap_err();
    assert!(error.message.contains("source tree exceeds compiler limit"));
    assert!(error.span.end <= source.len());
}
