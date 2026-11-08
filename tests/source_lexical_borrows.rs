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
    val result = wait(&mut r, receiver);
    r = result;
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
