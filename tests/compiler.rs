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
    assert!(program.functions[0].block.instructions.is_empty());
    assert_eq!(program.functions[0].block.return_value.0, 0);
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
    program.functions[0].block.instructions[0].operation = core::Operation::Neg(core::ValueId(2));
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
