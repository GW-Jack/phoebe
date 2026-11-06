use phoebe::{core, native, syntax};
use std::process::Command;

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
    let then_result = program.functions[0].blocks[1].instructions[0]
        .result
        .unwrap();
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

#[test]
fn mutable_places_and_loop_backedges_agree() {
    let program = compile(
        "fn sum_to(n: I64) -> I64 { var i = 0; var total = 0; while (i < n) { total = total + i; i = i + 1; } total }",
    );
    assert_eq!(program.functions[0].place_types.len(), 2);
    for (n, expected) in [(0, 0), (1, 0), (10, 45), (-4, 0)] {
        assert_eq!(program.interpret("sum_to", &[n]).unwrap(), expected);
        assert_eq!(native::run(&program, "sum_to", &[n]).unwrap(), expected);
    }
}

#[test]
fn boolean_place_and_branch_inside_loop_agree() {
    let program = compile(
        "fn main() -> I64 { var go = true; var value = 1; while (go) { value = if (value == 1) { 8 } else { 9 }; go = false; } value }",
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 8);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 8);
}

#[test]
fn rejects_val_assignment_and_bad_while_condition() {
    let source = "fn main() -> I64 { val x = 1; x = 2; x }";
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(error.message.contains("cannot assign to val"));
    let source = "fn main() -> I64 { var x = 1; while (x) { x = 2; } x }";
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(error.message.contains("expected Bool"));
}

#[test]
fn verifier_rejects_place_use_before_initialization() {
    let mut program = compile("fn main() -> I64 { var x = 1; x }");
    program.functions[0].blocks[0].instructions.swap(1, 2);
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("place used before initialization")
    );
}

#[test]
fn branch_mutations_rejoin_through_one_place() {
    let program = compile(
        "fn choose(flag: I64) -> I64 { var x = 0; val y = if (flag < 0) { x = 5; 1 } else { x = 7; 2 }; x + y }",
    );
    for (flag, expected) in [(-1, 6), (0, 9)] {
        assert_eq!(program.interpret("choose", &[flag]).unwrap(), expected);
        assert_eq!(native::run(&program, "choose", &[flag]).unwrap(), expected);
    }
}

#[test]
fn shared_and_exclusive_borrows_cross_calls() {
    let program = compile(
        "fn read(x: &I64) -> I64 = x fn bump(x: &mut I64) -> I64 { x = x + 1; x } fn main() -> I64 { val seed = 40; var x = read(&seed); bump(&mut x); bump(&mut x); x }",
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn exclusive_borrow_can_be_forwarded_as_shared() {
    let program = compile(
        "fn read(x: &I64) -> I64 = x fn forward(x: &mut I64) -> I64 = read(&x) fn main() -> I64 { var x = 42; forward(&mut x) }",
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn overlapping_exclusive_borrows_are_rejected() {
    let source = "fn pair(a: &mut I64, b: &mut I64) -> I64 = a + b fn main() -> I64 { var x = 1; pair(&mut x, &mut x) }";
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(error.message.contains("overlapping call borrows"));
    let source = "fn pair(a: &I64, b: &mut I64) -> I64 = a + b fn main() -> I64 { var x = 1; pair(&x, &mut x) }";
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(error.message.contains("overlapping call borrows"));
}

#[test]
fn borrowed_function_cannot_be_called_as_external_entry() {
    let program = compile("fn read(x: &I64) -> I64 = x");
    assert!(
        program
            .interpret("read", &[1])
            .unwrap_err()
            .contains("Phoebe caller")
    );
    assert!(
        native::run(&program, "read", &[1])
            .unwrap_err()
            .contains("Phoebe caller")
    );
}

#[test]
fn verifier_rejects_reference_storage() {
    let mut program = compile("fn main() -> I64 { var x = 1; x }");
    program.functions[0].place_types[0] = core::Type::RefMutI64;
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("references cannot be stored")
    );
}

#[test]
fn repeated_shared_borrow_is_allowed() {
    let program = compile(
        "fn add(a: &I64, b: &I64) -> I64 = a + b fn main() -> I64 { var x = 21; add(&x, &x) }",
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn call_borrow_begins_after_value_arguments_evaluate() {
    let program = compile(
        "fn bump(x: &mut I64) -> I64 { x = x + 1; x } fn add(x: &mut I64, n: I64) -> I64 { x = x + n; x } fn main() -> I64 { var x = 1; add(&mut x, bump(&mut x)) }",
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 4);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 4);
}

#[test]
fn standalone_executable_agrees_with_interpreter_and_jit() {
    for (index, source) in [
        "fn main() -> I64 = 20 * 2 + 2",
        "fn main() -> I64 { var x = 0; while (x < 6) { x = x + 1; } if (x == 6) { 42 } else { 0 } }",
        "fn bump(x: &mut I64) -> I64 { x = x + 1; x } fn main() -> I64 { var x = 40; bump(&mut x); bump(&mut x); x }",
    ]
    .iter()
    .enumerate()
    {
        let program = compile(source);
        let path = std::env::temp_dir().join(format!("phoebe-aot-{}-{index}", std::process::id()));
        native::build_executable(&program, &path).unwrap();
        let result = Command::new(&path).output().unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(result.status.success());
        let standalone: i64 = String::from_utf8(result.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(standalone, program.interpret("main", &[]).unwrap());
        assert_eq!(standalone, native::run(&program, "main", &[]).unwrap());
    }
}

#[test]
fn standalone_requires_zero_argument_main() {
    let program = compile("fn value() -> I64 = 42");
    assert!(native::emit_object(&program).unwrap_err().contains("main"));
    let program = compile("fn main(x: I64) -> I64 = x");
    assert!(
        native::emit_object(&program)
            .unwrap_err()
            .contains("no parameters")
    );
}

#[test]
fn bool_function_boundaries_work_across_backends() {
    let program = compile(
        "fn positive(x: I64) -> Bool = 0 < x fn invert(flag: Bool) -> Bool = flag == false fn choose(flag: Bool) -> I64 = if (flag) { 42 } else { 0 } fn main() -> I64 = choose(invert(positive(-1)))",
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-bool-aot-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let result = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(result.status.success());
    assert_eq!(String::from_utf8(result.stdout).unwrap(), "42\n");
}

#[test]
fn mismatched_bool_signature_is_rejected() {
    let source =
        "fn choose(flag: Bool) -> I64 = if (flag) { 1 } else { 0 } fn main() -> I64 = choose(1)";
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(error.message.contains("expected Bool"));
    let source = "fn main() -> Bool = 42";
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(error.message.contains("expected Bool"));
}

#[test]
fn verifier_reports_missing_callee_entry() {
    let mut program = compile("fn main() -> I64 = value() fn value() -> I64 = 42");
    program.functions[1].blocks.clear();
    assert!(core::verify(&program).unwrap_err().contains("entry block"));
}

#[test]
fn nominal_variants_and_exhaustive_when_agree_across_backends() {
    let program = compile(include_str!("../examples/variant.phoebe"));
    let comparisons = program.functions[0]
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| matches!(instruction.operation, core::Operation::Eq(_, _)))
        .count();
    assert_eq!(
        comparisons, 2,
        "three variants need only two tag comparisons"
    );
    assert_eq!(
        program.functions[0].blocks.len(),
        6,
        "the final arm needs no extra jump block"
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-variant-aot-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let result = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(result.status.success());
    assert_eq!(String::from_utf8(result.stdout).unwrap(), "42\n");
}

#[test]
fn when_checks_nominal_types_and_coverage() {
    let cases = [
        (
            "data Light { Red, Green } fn main() -> I64 = when (Light.Red) { Light.Red -> { 1 } }",
            "non-exhaustive when",
        ),
        (
            "data Light { Red, Green } fn main() -> I64 = when (Light.Red) { Light.Red -> { 1 }, Light.Red -> { 2 }, Light.Green -> { 3 } }",
            "duplicate when variant",
        ),
        (
            "data Light { Red, Green } fn main() -> I64 = when (Light.Red) { Light.Blue -> { 1 }, Light.Green -> { 2 } }",
            "unknown when variant",
        ),
        (
            "data Light { Red, Green } data Shade { Red } fn main() -> I64 = when (Light.Red) { Shade.Red -> { 1 }, Light.Green -> { 2 } }",
            "wrong data type",
        ),
        (
            "data Light { Red, Green } data Shade { Red } fn use(x: Light) -> I64 = 1 fn main() -> I64 = use(Shade.Red)",
            "expected Data",
        ),
    ];
    for (source, message) in cases {
        let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
        assert!(error.message.contains(message), "{error:?}");
    }
}

#[test]
fn verifier_rejects_invalid_data_tag() {
    let mut program = compile(
        "data Light { Red, Green } fn main() -> I64 = when (Light.Red) { Light.Red -> { 1 }, Light.Green -> { 2 } }",
    );
    let instruction = &mut program.functions[0].blocks[0].instructions[0];
    instruction.operation = core::Operation::Variant(core::DataId(0), 2);
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("invalid variant tag")
    );
}

#[test]
fn one_variant_and_mutable_nominal_place_agree() {
    let program = compile(
        "data State { Ready } fn main() -> I64 { var state = State.Ready; state = State.Ready; when (state) { State.Ready -> { 42 } } }",
    );
    assert!(
        !program.functions[0]
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| matches!(instruction.operation, core::Operation::Eq(_, _)))
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}
