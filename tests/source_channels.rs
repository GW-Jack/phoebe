use phoebe::core::{Operation, Terminator};
use phoebe::{core, native, syntax};
use std::process::Command;

fn source(capacity: i64) -> String {
    format!(
        r#"
fn create_and_close() -> Result<I64, ResourceError> {{
  val (sender, receiver) = channel<I64>(capacity = {capacity})?;
  sender.close();
  receiver.close();
  Result<I64, ResourceError>.Ok(7)
}}

fn main() -> I64 {{
  when (create_and_close()) {{
    Result<I64, ResourceError>.Ok(value) -> {{ value }},
    Result<I64, ResourceError>.Err(error) -> {{ 0 }}
  }}
}}
"#
    )
}

#[test]
fn source_channel_creation_agrees_on_success_and_resource_failure() {
    for (capacity, expected) in [(1, 7), (0, 0)] {
        let ast = syntax::parse(&source(capacity)).unwrap();
        let program = core::lower(&ast).unwrap();
        assert_eq!(program.interpret("main", &[]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
        let path = std::env::temp_dir().join(format!(
            "phoebe-source-channel-{}-{capacity}",
            std::process::id()
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
}

#[test]
fn source_channel_rights_cannot_be_forgotten_or_closed_twice() {
    let missing_close = source(1).replace("  receiver.close();\n", "");
    let error = core::lower(&syntax::parse(&missing_close).unwrap()).unwrap_err();
    assert!(error.message.contains("normal return leaks"), "{error:?}");
    assert!(error.span.start > 0);

    let double_close = source(1).replace(
        "  receiver.close();",
        "  sender.close();\n  receiver.close();",
    );
    let error = core::lower(&syntax::parse(&double_close).unwrap()).unwrap_err();
    assert!(error.message.contains("is not live"), "{error:?}");
}

#[test]
fn source_channel_creation_requires_explicit_failure_contract() {
    let missing_question = source(1).replace("capacity = 1)?", "capacity = 1)");
    let error = syntax::parse(&missing_question).unwrap_err();
    assert!(error.message.contains("? after fallible channel creation"));

    let wrong_return = source(1).replace("-> Result<I64, ResourceError>", "-> I64");
    let error = core::lower(&syntax::parse(&wrong_return).unwrap()).unwrap_err();
    assert!(error.message.contains("requires Result<_, ResourceError>"));

    let wrong_element = source(1).replace("channel<I64>", "channel<Bool>");
    let error = syntax::parse(&wrong_element).unwrap_err();
    assert!(error.message.contains("only I64 channels"));
}

#[test]
fn early_resource_failure_closes_earlier_channel_rights() {
    let source = r#"
fn acquire(second_capacity: I64) -> Result<I64, ResourceError> {
  val (first_sender, first_receiver) = channel<I64>(capacity = 1)?;
  val (second_sender, second_receiver) = channel<I64>(capacity = second_capacity)?;
  second_sender.close();
  second_receiver.close();
  first_sender.close();
  first_receiver.close();
  Result<I64, ResourceError>.Ok(7)
}

fn main() -> I64 {
  when (acquire(0)) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let printed = program.to_string();
    assert!(printed.contains("close.handle v"));
    let creation_failures = program.functions[0]
        .blocks
        .iter()
        .filter_map(|block| match block.terminator {
            Terminator::ChannelNew { failure, .. } => Some(failure),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(creation_failures.len(), 2);
    let closes = |block_id: core::BlockId| {
        program.functions[0].blocks[block_id.0]
            .instructions
            .iter()
            .filter(|instruction| matches!(instruction.operation, Operation::CloseHandle(_)))
            .count()
    };
    assert_eq!(closes(creation_failures[0]), 0);
    assert_eq!(closes(creation_failures[1]), 2);
    assert_eq!(program.interpret("main", &[]).unwrap(), 0);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 0);
    let path = std::env::temp_dir().join(format!(
        "phoebe-source-channel-cleanup-{}",
        std::process::id()
    ));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"0\n");
}

#[test]
fn ordinary_try_failure_closes_live_channel_rights() {
    let source = r#"
fn fail() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 0)?;
  sender.close();
  receiver.close();
  Result<I64, ResourceError>.Ok(1)
}

fn acquire() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val value = fail()?;
  sender.close();
  receiver.close();
  Result<I64, ResourceError>.Ok(value)
}

fn main() -> I64 {
  when (acquire()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 0);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 0);
}
