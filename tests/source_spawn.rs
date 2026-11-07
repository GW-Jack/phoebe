use phoebe::core::{Operation, Terminator};
use phoebe::runtime::ScopeLimits;
use phoebe::{core, native, syntax};

fn source() -> &'static str {
    r#"
fn worker(sender: Sender<I64>) -> I64 {
  sender.close();
  0
}

fn start() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  spawn worker(sender)?;
  receiver.close();
  Result<I64, ResourceError>.Ok(7)
}

fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#
}

#[test]
fn source_spawn_transfers_endpoint_and_failure_closes_parent_rights() {
    let program = core::lower(&syntax::parse(source()).unwrap()).unwrap();
    assert!(program.to_string().contains("spawn worker("));
    let spawn_failure = program.functions[1]
        .blocks
        .iter()
        .find_map(|block| match block.terminator {
            Terminator::Spawn { failure, .. } => Some(failure),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        program.functions[1].blocks[spawn_failure.0]
            .instructions
            .iter()
            .filter(|instruction| matches!(instruction.operation, Operation::CloseHandle(_)))
            .count(),
        2
    );
    assert_eq!(
        program
            .interpret_with_limits(
                "main",
                &[],
                ScopeLimits {
                    tasks: 2,
                    channels: 1,
                    replies: 0,
                },
            )
            .unwrap(),
        7
    );
    assert_eq!(
        program
            .interpret_with_limits(
                "main",
                &[],
                ScopeLimits {
                    tasks: 1,
                    channels: 1,
                    replies: 0,
                },
            )
            .unwrap(),
        0
    );
    assert!(
        native::run(&program, "main", &[])
            .unwrap_err()
            .contains("retained task frames")
    );
}

#[test]
fn source_spawn_rejects_ownership_and_signature_errors() {
    let missing_question = source().replace("spawn worker(sender)?;", "spawn worker(sender);");
    assert!(
        syntax::parse(&missing_question)
            .unwrap_err()
            .message
            .contains("? after fallible spawn")
    );

    let double_use = source().replace(
        "  receiver.close();",
        "  sender.close();\n  receiver.close();",
    );
    assert!(
        core::lower(&syntax::parse(&double_use).unwrap())
            .unwrap_err()
            .message
            .contains("not live")
    );

    let wrong_type = source().replace("Sender<I64>", "Sender<Bool>");
    assert!(
        syntax::parse(&wrong_type)
            .unwrap_err()
            .message
            .contains("only I64 channels")
    );

    let wrong_return = source().replace(
        "fn start() -> Result<I64, ResourceError>",
        "fn start() -> I64",
    );
    assert!(
        core::lower(&syntax::parse(&wrong_return).unwrap())
            .unwrap_err()
            .message
            .contains("requires Result<_, ResourceError>")
    );
}

#[test]
fn source_spawn_moves_closed_nominal_on_success_edge() {
    let source = r#"
data Badge { One }

fn worker(badge: Badge) -> I64 = 0

fn start() -> Result<I64, ResourceError> {
  val badge = Badge.One;
  spawn worker(badge)?;
  Result<I64, ResourceError>.Ok(1)
}

fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 1);

    let invalid = source.replace(
        "  Result<I64, ResourceError>.Ok(1)",
        "  val again = badge;\n  Result<I64, ResourceError>.Ok(1)",
    );
    assert!(
        core::lower(&syntax::parse(&invalid).unwrap())
            .unwrap_err()
            .message
            .contains("use after move")
    );
}
