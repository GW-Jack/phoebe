use phoebe::core::{Operation, Terminator};
use phoebe::{core, native, syntax};
use std::process::Command;

#[test]
fn forked_sender_and_receiver_are_independent_rights_in_all_backends() {
    let source = include_str!("../examples/fork_channel.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let forks = program.functions[0]
        .blocks
        .iter()
        .filter(|block| matches!(block.terminator, Terminator::Fork { .. }))
        .count();
    assert_eq!(forks, 2);
    let failure_cleanup = program.functions[0]
        .blocks
        .iter()
        .filter_map(|block| match block.terminator {
            Terminator::Fork { failure, .. } => Some(failure),
            _ => None,
        })
        .map(|failure| {
            program.functions[0].blocks[failure.0]
                .instructions
                .iter()
                .filter(|instruction| matches!(instruction.operation, Operation::CloseHandle(_)))
                .count()
        })
        .collect::<Vec<_>>();
    assert_eq!(failure_cleanup, [2, 3]);
    assert!(program.to_string().contains("handle.fork"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-fork-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn direct_only_fork_uses_the_same_native_abi() {
    let source = r#"
fn direct() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val forked = sender.fork()?;
  sender.close();
  forked.close();
  receiver.close();
  Result<I64, ResourceError>.Ok(42)
}

fn main() -> I64 {
  when (direct()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-direct-fork-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn forked_sender_transfers_to_a_second_producer() {
    let source = r#"
fn produce(sender: Sender<I64>, value: I64) -> I64 {
  sender.send(value);
  sender.close();
  0
}
fn exchange() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val second = sender.fork()?;
  spawn produce(sender, 20)?;
  spawn produce(second, 22)?;
  val first = receiver.receive();
  val next = receiver.receive();
  receiver.close();
  when (first) {
    Receive<I64>.Value(a) -> {
      when (next) {
        Receive<I64>.Value(b) -> { Result<I64, ResourceError>.Ok(a + b) },
        Receive<I64>.End -> { Result<I64, ResourceError>.Ok(a) }
      }
    },
    Receive<I64>.End -> { Result<I64, ResourceError>.Ok(0) }
  }
}
fn main() -> I64 {
  when (exchange()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-fork-producers-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n");
}
#[test]
fn fork_rejects_closed_handle_and_unaccounted_result() {
    let closed = r#"
fn bad() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  sender.close();
  val forked = sender.fork()?;
  forked.close();
  receiver.close();
  Result<I64, ResourceError>.Ok(0)
}

"#;
    let error = core::lower(&syntax::parse(closed).unwrap()).unwrap_err();
    assert!(
        error.message.contains("capability") || error.message.contains("closed"),
        "{error:?}"
    );

    let leaked = closed
        .replace("  sender.close();\n", "")
        .replace("  forked.close();\n", "");
    let error = core::lower(&syntax::parse(&leaked).unwrap()).unwrap_err();
    assert!(
        error.message.contains("live")
            || error.message.contains("capability")
            || error.message.contains("leaks"),
        "{error:?}"
    );
}

#[test]
fn verifier_rejects_a_forged_fork_edge() {
    let source = include_str!("../examples/fork_channel.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let (block_id, failure) = program.functions[0]
        .blocks
        .iter()
        .enumerate()
        .find_map(|(id, block)| match block.terminator {
            Terminator::Fork { failure, .. } => Some((id, failure)),
            _ => None,
        })
        .unwrap();
    let mut forged = program.clone();
    if let Terminator::Fork { success, .. } = &mut forged.functions[0].blocks[block_id].terminator {
        *success = failure;
    }
    assert!(
        core::verify(&forged)
            .unwrap_err()
            .contains("fork edges must be distinct")
    );
}
