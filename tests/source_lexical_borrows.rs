use phoebe::runtime::ScopeLimits;
use phoebe::runtime::{NativePollTask, Scheduler};
use phoebe::{core, frame_layout, native, syntax};
use std::process::Command;

fn assert_standalone(program: &core::Program, expected: i64, label: &str) {
    let path =
        std::env::temp_dir().join(format!("phoebe-source-loan-{label}-{}", std::process::id()));
    native::build_executable(program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
}

#[test]
fn sibling_field_loans_coexist_and_write_through_nested_path() {
    let source = r#"
data Inner { left: I64, right: I64 }
data Outer { head: I64, inner: Inner }
fn increment(value: &mut I64) -> Unit { value = value + 1; }
fn main() -> I64 {
  var record = Outer(1, Inner(20, 20));
  with left = &record.inner.left {
    with right = &mut record.inner.right {
      increment(&mut right);
      left + right + record.head
    }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let ir = program.to_string();
    assert!(ir.contains("borrow.begin &p0.[1, 0]"), "{ir}");
    assert!(ir.contains("borrow.begin &mut p0.[1, 1]"), "{ir}");
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "field-siblings");
}

#[test]
fn direct_field_call_borrows_and_retained_field_loan_agree() {
    let direct = r#"
data Pair { left: I64, right: I64 }
fn combine(a: &I64, b: &mut I64) -> I64 { b = b + 1; a + b }
fn main() -> I64 { var pair = Pair(20, 20); combine(&pair.left, &mut pair.right) }
"#;
    let direct = core::lower(&syntax::parse(direct).unwrap()).unwrap();
    assert_eq!(direct.interpret("main", &[]).unwrap(), 41);
    assert_eq!(native::run(&direct, "main", &[]).unwrap(), 41);
    assert_standalone(&direct, 41, "field-direct-call");

    let source = r#"
data Pair { left: I64, right: I64 }
fn combine(a: &I64, b: &mut I64) -> I64 { b = b + 1; a + b }
fn main() -> I64 {
  var pair = Pair(20, 20);
  val first = combine(&pair.left, &mut pair.right);
  with right = &mut pair.right { yield; right = right + 1; };
  first
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 41);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 41);
    assert_standalone(&program, 41, "field-call-yield");
}

#[test]
fn field_loan_state_is_exact_at_cfg_join() {
    let source = r#"
data Pair { left: I64, right: I64 }
fn main() -> I64 {
  var pair = Pair(20, 22);
  with left = &pair.left {
    val selected = if (true) { left } else { left };
    selected + pair.right
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "field-cfg-join");
}

#[test]
fn same_field_loan_and_whole_place_mutation_conflict() {
    let accepted = r#"
data Pair { left: I64, right: I64 }
fn main() -> I64 {
  var pair = Pair(20, 22);
  with left = &pair.left { with right = &mut pair.right { left + right } }
}
"#;
    assert!(core::lower(&syntax::parse(accepted).unwrap()).is_ok());
    let overlapping = accepted.replace("&mut pair.right", "&mut pair.left");
    let error = core::lower(&syntax::parse(&overlapping).unwrap()).unwrap_err();
    assert!(
        error.message.contains("overlapping lexical borrows"),
        "{error:?}"
    );
    let mutation = accepted.replace(
        "with right = &mut pair.right { left + right }",
        "pair = Pair(1, 2); left",
    );
    let error = core::lower(&syntax::parse(&mutation).unwrap()).unwrap_err();
    assert!(
        error
            .message
            .contains("mutation conflicts with lexical loan"),
        "{error:?}"
    );

    let alias = r#"
data Pair { left: I64, right: I64 }
fn combine(a: &I64, b: &mut I64) -> I64 { a + b }
fn main() -> I64 { var pair = Pair(20, 22); combine(&pair.left, &mut pair.left) }
"#;
    let error = core::lower(&syntax::parse(alias).unwrap()).unwrap_err();
    assert!(
        error.message.contains("overlapping call borrows"),
        "{error:?}"
    );

    let unknown = accepted.replace("&pair.left", "&pair.missing");
    let error = core::lower(&syntax::parse(&unknown).unwrap()).unwrap_err();
    assert!(error.message.contains("unknown product field"), "{error:?}");
}

#[test]
fn forged_field_path_is_rejected_before_interpretation_or_codegen() {
    let source = "data Pair { left: I64, right: I64 } fn main() -> I64 { var pair = Pair(20, 22); with left = &pair.left { left } }";
    let mut program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let operation = program.functions[0]
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
        .find(|instruction| {
            matches!(
                instruction.operation,
                core::Operation::BorrowBeginPath(_, _, _)
            )
        })
        .unwrap();
    let core::Operation::BorrowBeginPath(_, path, _) = &mut operation.operation else {
        unreachable!()
    };
    *path = vec![99];
    let error = core::verify(&program).unwrap_err();
    assert!(error.contains("field path has no product field"), "{error}");
}

#[test]
fn immutable_field_borrow_preserves_parent_value() {
    let source = r#"
data Pair { left: I64, right: I64 }
fn read(value: &I64) -> I64 { value }
fn main() -> I64 {
  val pair = Pair(20, 22);
  val first = read(&pair.left);
  with right = &pair.right { first + right + (pair.left - 20) }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "immutable-field");
}

#[test]
fn nested_source_reborrow_survives_yield() {
    let source = include_str!("../examples/lexical_borrow.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let ir = program.to_string();
    assert!(ir.contains("borrow.begin &mut p0"));
    assert!(ir.contains("borrow.reborrow &mut v1"));
    assert!(ir.contains("borrow.end v2"));
    assert!(ir.contains("borrow.end v1"));
    let frames = frame_layout::analyze(&program).unwrap();
    assert_eq!(frames[0].points[0].places, vec![core::PlaceId(0)]);
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "yield");
}

#[test]
fn source_loan_survives_channel_receive() {
    let source = include_str!("../examples/lexical_channel.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let exchange = program
        .functions
        .iter()
        .find(|function| function.name == "exchange")
        .unwrap();
    assert!(
        exchange
            .blocks
            .iter()
            .any(|block| matches!(block.terminator, core::Terminator::ChannelReceive { .. }))
    );
    let frames = frame_layout::analyze(&program).unwrap();
    let exchange_frame = frames
        .iter()
        .find(|frame| program.functions[frame.function.0].name == "exchange")
        .unwrap();
    assert!(exchange_frame.place_slots.contains(&core::PlaceId(0)));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "channel");
}

#[test]
fn source_loan_survives_reply_receive_on_delivery_and_end() {
    let source = r#"
fn worker(right: ReplyTo<I64>) -> I64 {
  right.complete(1);
  0
}

fn exchange() -> Result<I64, ResourceError> {
  val (right, receiver) = reply<I64>()?;
  spawn worker(right)?;
  var total = 41;
  with borrowed = &mut total {
    val answer = receiver.receive();
    borrowed = borrowed + when (answer) {
      Reply<I64>.Value(value) -> { value },
      Reply<I64>.End -> { 0 }
    };
  };
  Result<I64, ResourceError>.Ok(total)
}

fn main() -> I64 {
  when (exchange()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    for (worker, expected, label) in [
        ("right.complete(1);", 42, "reply-delivery"),
        ("yield; right.close();", 41, "reply-end"),
    ] {
        let source = source.replace("right.complete(1);", worker);
        let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
        let frames = frame_layout::analyze(&program).unwrap();
        let exchange = frames
            .iter()
            .find(|frame| program.functions[frame.function.0].name == "exchange")
            .unwrap();
        assert!(exchange.points.iter().any(|point| {
            point.instruction.is_none() && point.places.contains(&core::PlaceId(0))
        }));
        assert_eq!(program.interpret("main", &[]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
        assert_standalone(&program, expected, label);
    }
}

#[test]
fn source_loan_survives_spawn_and_scope_join() {
    let source = r#"
fn child() -> I64 { yield; 1 }

fn start() -> Result<I64, ResourceError> {
  var x = 41;
  with borrowed = &mut x {
    scope { spawn child()?; }
    borrowed = borrowed + 1;
  };
  Result<I64, ResourceError>.Ok(x)
}

fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "scope-join");
    let limited = ScopeLimits {
        tasks: 1,
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
}

#[test]
fn lexical_loans_survive_request_send_and_receive() {
    let source = include_str!("../examples/lexical_request.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let frames = frame_layout::analyze(&program).unwrap();
    let service = frames
        .iter()
        .find(|frame| program.functions[frame.function.0].name == "service")
        .unwrap();
    assert!(service.points.iter().any(|point| {
        point.instruction.is_none()
            && matches!(
                program.functions[service.function.0].blocks[point.block.0].terminator,
                core::Terminator::RequestReceive { .. }
            )
            && point.places.contains(&core::PlaceId(0))
    }));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "request");
}

#[test]
fn lexical_loan_survives_full_request_queue() {
    let source = include_str!("../examples/request_backpressure.phoebe")
        .replace(
            "  val second = request(22, second_right);\n  when (sender.send(second)) {",
            "  var total = 41;\n  val second = request(22, second_right);\n  with borrowed = &mut total {\n    when (sender.send(second)) {",
        )
        .replace(
            "  sender.close();\n  val first_value",
            "    borrowed = borrowed + 1;\n  };\n  sender.close();\n  val first_value",
        );
    let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
    let frames = frame_layout::analyze(&program).unwrap();
    let exchange = frames
        .iter()
        .find(|frame| program.functions[frame.function.0].name == "exchange")
        .unwrap();
    assert!(exchange.points.iter().any(|point| {
        point.instruction.is_none()
            && matches!(
                program.functions[exchange.function.0].blocks[point.block.0].terminator,
                core::Terminator::RequestSend { .. }
            )
            && point.places.contains(&core::PlaceId(0))
    }));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "request-backpressure");
    let executable = native::compile_poll_jit(&program, "main").unwrap();
    let root = unsafe { NativePollTask::new(executable.entry(), executable.layout().total_words) }
        .unwrap();
    let mut scheduler = Scheduler::new(root, 2, 1, 2).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert!(scheduler.stats().parks >= 2);
    scheduler.verify().unwrap();
}

#[test]
fn lexical_alias_can_reach_suspending_helper() {
    let source = include_str!("../examples/lexical_call.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let ir = program.to_string();
    assert!(ir.contains("call add_after_wait(&mut v"));
    let frames = frame_layout::analyze(&program).unwrap();
    let exchange_frame = frames
        .iter()
        .find(|frame| program.functions[frame.function.0].name == "exchange")
        .unwrap();
    assert!(
        exchange_frame
            .points
            .iter()
            .any(|point| point.instruction.is_some() && point.places.contains(&core::PlaceId(0)))
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "call");
}

#[test]
fn incoming_exclusive_parameter_can_be_reborrowed_across_yield() {
    let source = include_str!("../examples/lexical_parameter.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert!(program.to_string().contains("borrow.reborrow &mut v0"));
    let frames = frame_layout::analyze(&program).unwrap();
    let main = frames
        .iter()
        .find(|frame| program.functions[frame.function.0].name == "main")
        .unwrap();
    assert!(main.place_slots.contains(&core::PlaceId(0)));
    let change = frames
        .iter()
        .find(|frame| program.functions[frame.function.0].name == "change")
        .unwrap();
    assert!(change.place_slots.is_empty());
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "parameter");
}

#[test]
fn incoming_shared_parameter_grants_shared_lexical_child() {
    let source = "fn read(value: &I64) -> I64 { with alias = &value { alias } } fn main() -> I64 { var x = 42; read(&x) }";
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert!(program.to_string().contains("borrow.reborrow &v0"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "shared-parameter");
}

#[test]
fn incoming_parameter_is_frozen_while_exclusive_child_is_live() {
    let source = "fn bad(value: &mut I64) -> I64 { with alias = &mut value { value = 2; }; value } fn main() -> I64 { var x = 1; bad(&mut x) }";
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(error.message.contains("frozen parent cannot be used"));

    let upgrade = "fn bad(value: &I64) -> I64 { with alias = &mut value { alias = 2; }; value } fn main() -> I64 { var x = 1; bad(&x) }";
    let error = core::lower(&syntax::parse(upgrade).unwrap()).unwrap_err();
    assert!(
        error
            .message
            .contains("exclusive reborrow requires an exclusive parent")
    );

    let forwarded = "fn read(v: &mut I64) -> I64 { v } fn bad(value: &mut I64) -> I64 { with alias = &mut value { read(&mut value) } } fn main() -> I64 { var x = 1; bad(&mut x) }";
    let error = core::lower(&syntax::parse(forwarded).unwrap()).unwrap_err();
    assert!(error.message.contains("frozen parent cannot be forwarded"));
}

#[test]
fn unrelated_direct_helper_runs_inside_lexical_region() {
    let source = "fn one() -> I64 { 1 } fn main() -> I64 { var x = 41; with r = &mut x { r = r + one(); }; x }";
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "direct-call");
}

#[test]
fn call_cannot_reborrow_owner_around_live_lexical_alias() {
    let source = "fn write(v: &mut I64) -> I64 { v = 2; v } fn main() -> I64 { var x = 1; with r = &x { val ignored = write(&mut x); }; x }";
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(
        error
            .message
            .contains("call borrow conflicts with lexical loan")
    );

    let frozen = "fn write(v: &mut I64) -> I64 { v = 2; v } fn main() -> I64 { var x = 1; with outer = &mut x { with inner = &mut outer { val ignored = write(&mut outer); }; }; x }";
    let error = core::lower(&syntax::parse(frozen).unwrap()).unwrap_err();
    assert!(error.message.contains("frozen parent cannot be forwarded"));
}

#[test]
fn shared_lexical_and_direct_call_borrows_can_coexist() {
    let source = "fn add(a: &I64, b: &I64) -> I64 { a + b } fn main() -> I64 { var x = 21; with r = &x { add(&r, &x) } }";
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "shared-call");
}

#[test]
fn deadlocked_helper_with_live_lexical_loan_unwinds_task() {
    let source = r#"
fn wait(value: &mut I64, receiver: Receiver<I64>) -> I64 {
  val item = receiver.receive();
  receiver.close();
  value
}

fn exchange() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  var x = 41;
  with r = &mut x {
    with inner = &mut r {
      val result = wait(&mut inner, receiver);
      inner = result;
    };
  };
  sender.close();
  Result<I64, ResourceError>.Ok(x)
}

fn main() -> I64 {
  when (exchange()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert!(
        program
            .interpret("main", &[])
            .unwrap_err()
            .contains("scope deadlocked")
    );
    assert!(
        native::run(&program, "main", &[])
            .unwrap_err()
            .contains("scope deadlocked")
    );
    let executable = native::compile_poll_jit(&program, "main").unwrap();
    let root = unsafe { NativePollTask::new(executable.entry(), executable.layout().total_words) }
        .unwrap();
    let mut scheduler = Scheduler::new(root, 1, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap_err(), "scope deadlocked");
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();

    let path = std::env::temp_dir().join(format!("phoebe-lexical-deadlock-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stderr, b"phoebe runtime failed: scope deadlocked\n");
}

#[test]
fn failing_child_cancels_pending_borrowed_call() {
    let source = r#"
fn grandchild() -> I64 { yield; 0 }

fn build() -> Result<I64, ResourceError> {
  spawn grandchild()?;
  Result<I64, ResourceError>.Ok(0)
}

fn child() -> I64 {
  scope { val unused = build(); }
  0
}

fn wait(value: &mut I64) -> Result<I64, ResourceError> {
  scope { spawn child()?; }
  Result<I64, ResourceError>.Ok(value)
}

fn start() -> Result<I64, ResourceError> {
  var x = 41;
  with borrowed = &mut x {
    with inner = &mut borrowed {
      val updated = wait(&mut inner)?;
      inner = updated + 1;
    };
  };
  Result<I64, ResourceError>.Ok(x)
}

fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let limits = ScopeLimits {
        tasks: 3,
        scopes: 1,
        ..ScopeLimits::default()
    };
    let interpreted = program
        .interpret_with_limits("main", &[], limits)
        .unwrap_err();
    assert!(interpreted.contains("scope"), "{interpreted}");
    let native_error = native::run_with_limits(&program, "main", &[], limits).unwrap_err();
    assert!(native_error.contains("task 1 failed"), "{native_error}");

    let executable = native::compile_poll_jit(&program, "main").unwrap();
    let root = unsafe { NativePollTask::new(executable.entry(), executable.layout().total_words) }
        .unwrap();
    let mut scheduler = Scheduler::with_limits(root, limits).unwrap();
    assert_eq!(scheduler.run().unwrap_err(), native_error);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

#[test]
fn shared_val_borrow_can_return_a_scalar_from_with_expression() {
    let source =
        "fn main() -> I64 { val ready = true; with r = &ready { if (r) { 42 } else { 0 } } }";
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert!(program.to_string().contains("borrow.begin &p0"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "shared-val");
}

#[test]
fn question_failure_ends_live_loan_before_return() {
    let source = r#"
fn fail() -> Result<I64, I64> {
  var x = 41;
  with r = &mut x {
    val unused = Result<I64, I64>.Err(7)?;
    r = unused;
  };
  Result<I64, I64>.Ok(x)
}

fn main() -> I64 {
  when (fail()) {
    Result<I64, I64>.Ok(value) -> { value },
    Result<I64, I64>.Err(error) -> { error }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let ir = program.to_string();
    assert!(ir.contains("borrow.end"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 7);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 7);
    assert_standalone(&program, 7, "question");
}

#[test]
fn invalid_source_borrows_report_the_source_span() {
    let bare = "fn main() -> I64 { var x = 1; val r = &x; 0 }";
    let error = core::lower(&syntax::parse(bare).unwrap()).unwrap_err();
    assert_eq!(&bare[error.span.start..error.span.end], "&x");
    assert!(error.message.contains("requires a with binding"));

    let immutable = "fn main() -> I64 { val x = 1; with r = &mut x { r = 2; }; 0 }";
    let error = core::lower(&syntax::parse(immutable).unwrap()).unwrap_err();
    assert_eq!(&immutable[error.span.start..error.span.end], "&mut x");
    assert!(error.message.contains("cannot grant"));

    let upgrade = "fn main() -> I64 { var x = 1; with a = &x { with b = &mut a { b = 2; }; }; x }";
    let error = core::lower(&syntax::parse(upgrade).unwrap()).unwrap_err();
    assert_eq!(&upgrade[error.span.start..error.span.end], "&mut a");
    assert!(error.message.contains("exclusive reborrow"));

    let overlap = "fn main() -> I64 { var x = 1; with r = &mut x { val other = x; }; x }";
    let error = core::lower(&syntax::parse(overlap).unwrap()).unwrap_err();
    assert!(error.message.contains("read conflicts with exclusive loan"));
}
