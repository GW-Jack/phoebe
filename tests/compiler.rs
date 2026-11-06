use phoebe::{core, native, syntax};

fn compile(source: &str) -> core::Program {
    core::lower(&syntax::parse(source).unwrap()).unwrap()
}

#[test]
fn interpreter_and_native_agree_on_calls_and_precedence() {
    let program = compile(
        "\
        fn twice(x: I64) -> I64 = x * 2
        fn shifted(x: I64) -> I64 { val y = twice(x + 3); y - 4 }
        fn main() -> I64 = shifted(20)
    ",
    );
    for (name, args, expected) in [
        ("main", vec![], 42),
        ("twice", vec![11], 22),
        ("shifted", vec![0], 2),
    ] {
        assert_eq!(program.interpret(name, &args).unwrap(), expected);
        assert_eq!(native::run(&program, name, &args).unwrap(), expected);
    }
}

#[test]
fn arithmetic_wraps_identically() {
    let program = compile("fn main() -> I64 = 9223372036854775807 + 1");
    assert_eq!(program.interpret("main", &[]).unwrap(), i64::MIN);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), i64::MIN);
}

#[test]
fn identity_has_no_core_instructions_or_copy() {
    let program = compile("fn identity(x: I64) -> I64 = x");
    assert!(program.functions[0].blocks[0].instructions.is_empty());
    assert!(matches!(
        program.functions[0].blocks[0].terminator,
        core::Terminator::Return(core::ValueId(0))
    ));
    assert_eq!(native::run(&program, "identity", &[73]).unwrap(), 73);
}

#[test]
fn rejects_undefined_names_and_bad_calls() {
    let source = "fn main() -> I64 = missing";
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(error.message.contains("unknown local"));
    let source = "fn f(x: I64) -> I64 = x fn main() -> I64 = f()";
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(error.message.contains("expects 1 arguments"));
}

#[test]
fn verifier_rejects_use_before_definition() {
    let mut program = compile("fn main() -> I64 = 1 + 2");
    program.functions[0].blocks[0].instructions[0].operation =
        core::Operation::Neg(core::ValueId(2));
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("use before definition")
    );
}

#[test]
fn recursion_is_rejected_until_task_stacks_are_guarded() {
    let source = "fn f(x: I64) -> I64 = g(x) fn g(x: I64) -> I64 = f(x)";
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(error.message.contains("recursive call cycle"));
}

#[test]
fn branches_and_block_parameters_agree_across_backends() {
    let program = compile(
        "fn abs(x: I64) -> I64 = if (x < 0) { -x } else { x } fn main() -> I64 = abs(-41) + 1",
    );
    assert!(program.functions[0].blocks.len() >= 4);
    for (name, args, expected) in [
        ("abs", vec![-41], 41),
        ("abs", vec![7], 7),
        ("main", vec![], 42),
    ] {
        assert_eq!(program.interpret(name, &args).unwrap(), expected);
        assert_eq!(native::run(&program, name, &args).unwrap(), expected);
    }
}

#[test]
fn nested_branches_and_bool_equality_agree() {
    let program = compile(
        "fn classify(x: I64) -> I64 = if (x < 0) { -1 } else { if (x == 0) { 0 } else { 1 } }",
    );
    for (arg, expected) in [(-3, -1), (0, 0), (9, 1)] {
        assert_eq!(program.interpret("classify", &[arg]).unwrap(), expected);
        assert_eq!(native::run(&program, "classify", &[arg]).unwrap(), expected);
    }
}

#[test]
fn rejects_non_boolean_conditions_and_mismatched_branches() {
    let source = "fn main() -> I64 = if (1) { 2 } else { 3 }";
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(error.message.contains("expected Bool"));
    let source = "fn main() -> I64 = if (true) { 2 } else { false }";
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(error.message.contains("branches must have the same type"));
}

#[test]
fn verifier_rejects_non_dominating_branch_value() {
    let mut program = compile("fn main() -> I64 = if (true) { 1 } else { 2 }");
    let then_result = program.functions[0].blocks[1].instructions[0].result;
    if let core::Terminator::Jump(_, args) = &mut program.functions[0].blocks[2].terminator {
        args[0] = then_result;
    } else {
        panic!("expected else jump");
    }
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("non-dominating")
    );
}

#[test]
fn boolean_join_value_can_control_another_branch() {
    let program = compile(
        "fn main(x: I64) -> I64 = if (if (x < 0) { true } else { false }) { 1 } else { 2 }",
    );
    for (arg, expected) in [(-4, 1), (4, 2)] {
        assert_eq!(program.interpret("main", &[arg]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[arg]).unwrap(), expected);
    }
}
