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
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n");

    let cache =
        std::env::temp_dir().join(format!("phoebe-source-scope-cache-{}", std::process::id()));
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
    let cached = Command::new(&path).output().unwrap();
    assert!(cached.status.success());
    assert_eq!(cached.stdout, b"42\n");
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir_all(cache).unwrap();
}

#[test]
fn shifted_source_reuses_core_with_current_spawn_and_join_spans() {
    let source = include_str!("../examples/scoped_spawn.phoebe");
    let before_ast = syntax::parse(source).unwrap();
    let before = core::lower(&before_ast).unwrap();
    let shifted_ast = syntax::parse(&format!("\n{source}")).unwrap();
    let hints = vec![true; before_ast.functions.len()];
    let deltas = vec![1; before_ast.functions.len()];
    let (reused, count) =
        core::lower_reusing(&shifted_ast, &before_ast, &before, &hints, &deltas).unwrap();
    assert_eq!(count, hints.len());
    let fresh = core::lower(&shifted_ast).unwrap();
    let mut seen_spawn = 0;
    let mut seen_join = 0;
    for (reused_function, fresh_function) in reused.functions.iter().zip(&fresh.functions) {
        for (reused_block, fresh_block) in reused_function.blocks.iter().zip(&fresh_function.blocks)
        {
            match (&reused_block.terminator, &fresh_block.terminator) {
                (Terminator::Spawn { span: reused, .. }, Terminator::Spawn { span: fresh, .. }) => {
                    seen_spawn += 1;
                    assert_eq!(reused, fresh);
                }
                (
                    Terminator::ScopeJoin { span: reused, .. },
                    Terminator::ScopeJoin { span: fresh, .. },
                ) => {
                    seen_join += 1;
                    assert_eq!(reused, fresh);
                }
                _ => {}
            }
        }
    }
    assert!(seen_spawn > 0);
    assert!(seen_join > 0);
    assert_eq!(reused.cost_events(), fresh.cost_events());
}

#[test]
fn scope_capacity_failure_faults_both_backends() {
    let program = lower(include_str!("../examples/scoped_spawn.phoebe"));
    let no_scopes = ScopeLimits {
        scopes: 0,
        ..ScopeLimits::default()
    };
    assert!(
        program
            .interpret_with_limits("main", &[], no_scopes)
            .is_err()
    );
    assert!(native::run_with_limits(&program, "main", &[], no_scopes).is_err());
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

#[test]
fn early_failure_closes_owned_place_before_joining_waiting_child() {
    let source = r#"
data Holder { sender: Sender<I64> }

fn child(receiver: Receiver<I64>) -> Unit {
  val outcome = receiver.receive();
  receiver.close();
}

fn work(capacity: I64) -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  var held = Holder(sender);
  scope {
    spawn child(receiver)?;
    val (extra_sender, extra_receiver) = channel<I64>(capacity = capacity)?;
    extra_sender.close();
    extra_receiver.close();
    val owner = held;
    owner.close_caps();
  }
  Result<I64, ResourceError>.Ok(42)
}

fn value(result: Result<I64, ResourceError>) -> I64 = when (result) {
  Result<I64, ResourceError>.Ok(number) -> { number },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
fn main() -> I64 = value(work(1))
fn fail() -> I64 = value(work(0))
"#;
    let program = lower(source);
    core::verify(&program).unwrap();
    for (entry, expected) in [("main", 42), ("fail", 0)] {
        assert_eq!(program.interpret(entry, &[]).unwrap(), expected);
        assert_eq!(native::run(&program, entry, &[]).unwrap(), expected);
    }
    let failure = &program.functions[1];
    let cleanup_before_join = failure.blocks.iter().any(|block| {
        matches!(block.terminator, Terminator::ScopeJoin { .. })
            && block.instructions.windows(2).any(|pair| {
                matches!(pair[0].operation, Operation::Take(_))
                    && matches!(pair[1].operation, Operation::CloseCaps(_))
            })
    });
    assert!(cleanup_before_join);
    for (capacity, expected) in [(1, b"42\n".as_slice()), (0, b"0\n".as_slice())] {
        let source = source.replace(
            "main() -> I64 = value(work(1))",
            &format!("main() -> I64 = value(work({capacity}))"),
        );
        let program = lower(&source);
        let path = std::env::temp_dir().join(format!(
            "phoebe-scope-place-cleanup-{}-{capacity}",
            std::process::id()
        ));
        native::build_executable(&program, &path).unwrap();
        let output = Command::new(&path).output().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected);
    }
}
