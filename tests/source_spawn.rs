use phoebe::core::{Operation, Terminator};
use phoebe::runtime::ScopeLimits;
use phoebe::{core, native, syntax};
use std::process::Command;

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
fn direct_parent_spawns_suspending_child_in_jit_and_object() {
    let source = include_str!("../examples/spawn_pair.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let suspendable = phoebe::frame_layout::suspendable_functions(&program).unwrap();
    assert!(suspendable[0]);
    assert!(suspendable[1]);
    assert!(!suspendable[2]);
    assert!(!suspendable[3]);
    let clif = native::clif(&program).unwrap();
    assert!(clif.contains("Phoebe function main"));
    assert!(clif.contains("Phoebe poll function producer"));
    assert!(clif.contains("Phoebe poll function consumer"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 7);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 7);
    let limited = ScopeLimits {
        tasks: 1,
        channels: 1,
        replies: 0,
        ..ScopeLimits::default()
    };
    assert_eq!(
        program.interpret_with_limits("main", &[], limited).unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], limited).unwrap(),
        0
    );
    let second_spawn_limited = ScopeLimits {
        tasks: 2,
        channels: 1,
        replies: 0,
        ..ScopeLimits::default()
    };
    assert_eq!(
        program
            .interpret_with_limits("main", &[], second_spawn_limited)
            .unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], second_spawn_limited).unwrap(),
        0
    );
    let path = std::env::temp_dir().join(format!("phoebe-mixed-spawn-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let mut session = native::NativeBuildSession::new();
    assert_eq!(session.build_executable(&program, &path).unwrap(), 0);
    assert_eq!(session.build_executable(&program, &path).unwrap(), 4);
    let shifted = core::lower(&syntax::parse(&format!("\n{source}")).unwrap()).unwrap();
    assert_eq!(session.build_executable(&shifted, &path).unwrap(), 4);
    let unrelated =
        core::lower(&syntax::parse(&format!("{source}\nfn unused() -> I64 = 9\n")).unwrap())
            .unwrap();
    assert_eq!(session.build_executable(&unrelated, &path).unwrap(), 4);
    let cache =
        std::env::temp_dir().join(format!("phoebe-mixed-spawn-cache-{}", std::process::id()));
    assert!(
        !native::build_executable_cached(&program, &path, &cache)
            .unwrap()
            .reused
    );
    assert!(
        native::build_executable_cached(&program, &path, &cache)
            .unwrap()
            .reused
    );
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir_all(cache).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"7\n");
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
                    ..ScopeLimits::default()
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
                    ..ScopeLimits::default()
                },
            )
            .unwrap(),
        0
    );
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 7);
    let native_main = native::compile_jit(&program, "main").unwrap();
    assert_eq!(
        native_main
            .run_with_limits(
                &[],
                ScopeLimits {
                    tasks: 1,
                    channels: 1,
                    replies: 0,
                    ..ScopeLimits::default()
                }
            )
            .unwrap(),
        0
    );
    let path = std::env::temp_dir().join(format!("phoebe-spawn-aot-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"7\n");
    std::fs::remove_file(path).unwrap();
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
            .contains("only I64 communication values")
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
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 1);

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
