use phoebe::core::{self, Operation, Terminator};
use phoebe::runtime::ScopeLimits;
use phoebe::{native, syntax};
use std::process::Command;

fn lower(source: &str) -> core::Program {
    core::lower(&syntax::parse(source).unwrap()).unwrap()
}

#[test]
fn source_scope_joins_spawned_child_in_both_backends() {
    let program = lower(include_str!("../examples/scoped_spawn.phoebe"));
    core::verify(&program).unwrap();
    assert!(program.functions[1].blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .any(|instruction| matches!(instruction.operation, Operation::ScopeEnter(_)))
    }));
    assert!(
        program.functions[1]
            .blocks
            .iter()
            .any(|block| matches!(block.terminator, Terminator::ScopeJoin { .. }))
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-source-scope-{}", std::process::id()));
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
fn nested_scope_failure_joins_before_returning_error() {
    let program = lower(
        r#"
fn child() -> I64 = 7
fn start() -> Result<I64, ResourceError> {
  scope {
    scope {
      spawn child()?;
    }
  }
  Result<I64, ResourceError>.Ok(42)
}
fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#,
    );
    core::verify(&program).unwrap();
    assert_eq!(
        program.functions[1]
            .blocks
            .iter()
            .filter(|block| matches!(block.terminator, Terminator::ScopeJoin { .. }))
            .count(),
        4
    );
    let no_child = ScopeLimits {
        tasks: 1,
        ..ScopeLimits::default()
    };
    assert_eq!(
        program
            .interpret_with_limits("main", &[], no_child)
            .unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], no_child).unwrap(),
        0
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn scope_without_spawn_or_spawn_call_is_erased() {
    let program = lower(
        r#"
fn helper() -> I64 = 1
fn main() -> I64 {
  scope { val unused = helper(); }
  42
}
"#,
    );
    assert!(
        !program
            .cost_events()
            .iter()
            .any(|event| event.kind.starts_with("scope."))
    );
    assert!(!program.to_string().contains("scope.enter"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn transitive_spawn_call_keeps_scope_and_joins_on_error() {
    let program = lower(
        r#"
fn child() -> I64 = 7
fn launch() -> Result<I64, ResourceError> {
  spawn child()?;
  Result<I64, ResourceError>.Ok(1)
}
fn start() -> Result<I64, ResourceError> {
  scope { val unused = launch()?; }
  Result<I64, ResourceError>.Ok(42)
}
fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#,
    );
    core::verify(&program).unwrap();
    assert!(
        program.functions[2]
            .blocks
            .iter()
            .any(|block| matches!(block.terminator, Terminator::ScopeJoin { .. }))
    );
    let no_child = ScopeLimits {
        tasks: 1,
        ..ScopeLimits::default()
    };
    assert_eq!(
        program
            .interpret_with_limits("main", &[], no_child)
            .unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], no_child).unwrap(),
        0
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn early_channel_failure_joins_existing_child() {
    let program = lower(
        r#"
fn child() -> I64 { yield; 7 }
fn start() -> Result<I64, ResourceError> {
  scope {
    spawn child()?;
    val (sender, receiver) = channel<I64>(capacity = 1)?;
    sender.close();
    receiver.close();
  }
  Result<I64, ResourceError>.Ok(42)
}

fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#,
    );
    core::verify(&program).unwrap();
    let no_channels = ScopeLimits {
        tasks: 2,
        channels: 0,
        ..ScopeLimits::default()
    };
    assert_eq!(
        program
            .interpret_with_limits("main", &[], no_channels)
            .unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], no_channels).unwrap(),
        0
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn inner_scope_does_not_join_preexisting_sibling() {
    let program = lower(
        r#"
fn sibling(receiver: Receiver<I64>) -> I64 {
  val message = receiver.receive();
  receiver.close();
  0
}
fn inner() -> I64 = 1
fn start() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  spawn sibling(receiver)?;
  scope { spawn inner()?; }
  val sent = sender.send(7);
  sender.close();
  Result<I64, ResourceError>.Ok(42)
}
fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#,
    );
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn scope_statement_requires_semicolon_after_final_expression() {
    let source = "fn main() -> I64 { scope { 1 } 2 }";
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(error.message.contains("scope body cannot end"));
}

#[test]
fn incremental_reuse_rechecks_scope_when_callee_starts_spawning() {
    let before = r#"
fn child() -> I64 = 0
fn helper() -> Result<I64, ResourceError> = Result<I64, ResourceError>.Ok(1)
fn start() -> Result<I64, ResourceError> {
  scope { val unused = helper()?; }
  spawn child()?;
  Result<I64, ResourceError>.Ok(42)
}
"#;
    let after = before.replace(
        "fn helper() -> Result<I64, ResourceError> = Result<I64, ResourceError>.Ok(1)",
        "fn helper() -> Result<I64, ResourceError> { spawn child()?; Result<I64, ResourceError>.Ok(1) }",
    );
    let old_ast = syntax::parse(before).unwrap();
    let old = core::lower(&old_ast).unwrap();
    assert!(!old.functions[2].blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .any(|i| matches!(i.operation, Operation::ScopeEnter(_)))
    }));
    let new_ast = syntax::parse(&after).unwrap();
    let (new, _) = core::lower_reusing(&new_ast, &old_ast, &old, &[true; 3], &[0; 3]).unwrap();
    core::verify(&new).unwrap();
    assert!(new.functions[2].blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .any(|i| matches!(i.operation, Operation::ScopeEnter(_)))
    }));
}

#[test]
fn spawn_limit_failure_closes_owned_channel_before_join() {
    let program = lower(
        r#"
fn child() -> I64 { yield; 7 }
fn start() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  scope {
    spawn child()?;
    spawn child()?;
  }
  sender.close();
  receiver.close();
  Result<I64, ResourceError>.Ok(42)
}
fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#,
    );
    core::verify(&program).unwrap();
    let one_child = ScopeLimits {
        tasks: 2,
        channels: 1,
        ..ScopeLimits::default()
    };
    assert_eq!(
        program
            .interpret_with_limits("main", &[], one_child)
            .unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], one_child).unwrap(),
        0
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn borrowed_caller_place_survives_pending_scope_join() {
    let program = lower(
        r#"
fn child() -> I64 { yield; 0 }
fn update_after_join(value: &mut I64) -> Result<I64, ResourceError> {
  scope { spawn child()?; }
  value = value + 1;
  Result<I64, ResourceError>.Ok(value)
}
fn start() -> Result<I64, ResourceError> {
  var held = 41;
  val answer = update_after_join(&mut held)?;
  Result<I64, ResourceError>.Ok(answer)
}
fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#,
    );
    core::verify(&program).unwrap();
    let frames = phoebe::frame_layout::analyze(&program).unwrap();
    assert!(
        frames[2]
            .points
            .iter()
            .any(|point| point.places.contains(&core::PlaceId(0)))
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let no_child = ScopeLimits {
        tasks: 1,
        ..ScopeLimits::default()
    };
    assert_eq!(
        program
            .interpret_with_limits("main", &[], no_child)
            .unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], no_child).unwrap(),
        0
    );
    let path = std::env::temp_dir().join(format!("phoebe-scope-borrow-{}", std::process::id()));
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
