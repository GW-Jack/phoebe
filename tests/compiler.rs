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
fn minimum_i64_literal_and_adjacent_overflows_are_consistent() {
    let source = "fn main() -> I64 = -9223372036854775808 + 1";
    let program = compile(source);
    assert_eq!(program.interpret("main", &[]).unwrap(), i64::MIN + 1);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), i64::MIN + 1);
    let path = std::env::temp_dir().join(format!("phoebe-min-i64-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, format!("{}\n", i64::MIN + 1).as_bytes());

    for literal in ["9223372036854775808", "-9223372036854775809"] {
        let source = format!("fn main() -> I64 = {literal}");
        let error = syntax::parse(&source).unwrap_err();
        assert_eq!(&source[error.span.start..error.span.end], literal);
        assert_eq!(error.message, "integer literal exceeds I64");
        assert!(error.render(&source).starts_with("1:20:"));
    }
}

#[test]
fn comparison_family_preserves_types_and_native_results() {
    let source = r#"
data Choice { A, B }
fn direct() -> I64 = if (
  -1 < 0 && 0 > -1 && -1 <= -1 && 0 >= 0 &&
  41 != 42 && true != false && Choice.A != Choice.B &&
  42 > 42 == false && 41 >= 42 == false
) { 42 } else { 0 }
fn main() -> I64 { yield; direct() }
"#;
    let program = compile(source);
    for (name, op) in [
        ("ne", "ne v"),
        ("le", "le v"),
        ("gt", "gt v"),
        ("ge", "ge v"),
    ] {
        assert!(program.to_string().contains(op), "missing {name} in Core");
    }
    assert!(
        program.functions[0..2]
            .iter()
            .flat_map(|function| &function.blocks)
            .flat_map(|block| &block.instructions)
            .all(|instruction| !matches!(instruction.operation, core::Operation::Not(_)))
    );
    for name in ["direct", "main"] {
        assert_eq!(program.interpret(name, &[]).unwrap(), 42);
        assert_eq!(native::run(&program, name, &[]).unwrap(), 42);
    }
    let path = std::env::temp_dir().join(format!("phoebe-comparisons-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");

    for source in [
        "fn main() -> I64 = if (true < false) { 1 } else { 0 }",
        "fn main() -> I64 = if (1 != false) { 1 } else { 0 }",
    ] {
        assert!(core::lower(&syntax::parse(source).unwrap()).is_err());
    }
}

#[test]
fn ordered_comparison_evaluates_left_before_right() {
    let source = r#"
data Problem { Left, Right }
fn left() -> Result<I64, Problem> = Result<I64, Problem>.Err(Problem.Left)
fn right() -> Result<I64, Problem> = Result<I64, Problem>.Err(Problem.Right)
fn compare() -> Result<I64, Problem> = Result<I64, Problem>.Ok(
  if (left()? <= right()?) { 1 } else { 0 }
)
fn main() -> I64 = when (compare()) {
  Result<I64, Problem>.Ok(value) -> { 0 },
  Result<I64, Problem>.Err(error) -> { when (error) {
    Problem.Left -> { 42 },
    Problem.Right -> { 0 },
  } },
}
"#;
    let program = compile(source);
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
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
    assert_eq!(&source[error.span.start..error.span.end], "f(x)");
    assert_eq!(
        error.render(source),
        "1:50: recursive call cycle is not yet supported"
    );
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
fn bool_borrows_read_write_and_forward_without_copying_owner() {
    let program = compile(
        "fn read(x: &Bool) -> Bool = x \
         fn flip(x: &mut Bool) -> Bool { x = !x; x } \
         fn forward(x: &mut Bool) -> Bool = read(&x) \
         fn main() -> I64 { val initial = false; var flag = read(&initial); flip(&mut flag); \
           if (forward(&mut flag) && flag) { 42 } else { 0 } }",
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-bool-borrow-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = std::process::Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn bool_borrow_type_and_alias_conflicts_are_rejected() {
    let wrong = "fn read(x: &Bool) -> Bool = x fn main() -> I64 { var x = 1; read(&x); 0 }";
    let error = core::lower(&syntax::parse(wrong).unwrap()).unwrap_err();
    assert!(error.message.contains("expected Bool"), "{error:?}");

    let overlap = "fn pair(a: &Bool, b: &mut Bool) -> Bool = a && b \
                   fn main() -> I64 { var x = true; pair(&x, &mut x); 0 }";
    let error = core::lower(&syntax::parse(overlap).unwrap()).unwrap_err();
    assert!(
        error.message.contains("overlapping call borrows"),
        "{error:?}"
    );
}

#[test]
fn call_loan_regions_are_printed_and_verified() {
    let source = include_str!("../examples/borrow.phoebe");
    let program = compile(source);
    let (_, main) = program.function("main").unwrap();
    assert_eq!(main.loan_regions.len(), 4);
    assert_eq!(
        main.loan_regions[0].roots,
        vec![core::LoanRoot::Place(core::PlaceId(0), false)]
    );
    let (_, forward) = program.function("forward").unwrap();
    assert_eq!(
        forward.loan_regions[0].roots,
        vec![core::LoanRoot::Forwarded(core::ValueId(0), false)]
    );
    assert!(program.to_string().contains("region r0: begin block0.i2"));
    core::verify(&program).unwrap();

    let mut missing = program.clone();
    let main_id = missing.function("main").unwrap().0;
    missing.functions[main_id.0].loan_regions.pop();
    assert!(
        core::verify(&missing)
            .unwrap_err()
            .contains("loan region count mismatch")
    );

    let mut forged = program.clone();
    let main_id = forged.function("main").unwrap().0;
    forged.functions[main_id.0].loan_regions[0].roots[0] =
        core::LoanRoot::Place(core::PlaceId(1), false);
    assert!(
        core::verify(&forged)
            .unwrap_err()
            .contains("loan region mismatch")
    );
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
fn boolean_operators_short_circuit_error_effects_across_backends() {
    let program = compile(include_str!("../examples/boolean_short_circuit.phoebe"));
    let (_, skipped) = program.function("skipped_and").unwrap();
    assert!(matches!(
        skipped.blocks[0].terminator,
        core::Terminator::Branch { .. }
    ));
    assert!(
        !skipped.blocks[0]
            .instructions
            .iter()
            .any(|instruction| { matches!(instruction.operation, core::Operation::Call(_, _)) })
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-bool-short-aot-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let result = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(result.status.success());
    assert_eq!(result.stdout, b"42\n");
}

#[test]
fn boolean_operator_precedence_and_types_are_checked() {
    let program =
        compile("fn main() -> I64 = if (!false || false && false == true) { 42 } else { 0 }");
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    for source in [
        "fn main() -> I64 = if (1 && true) { 42 } else { 0 }",
        "fn main() -> I64 = if (true || 1) { 42 } else { 0 }",
        "fn main() -> I64 = if (!1) { 42 } else { 0 }",
    ] {
        let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
        assert!(error.message.contains("expected Bool"), "{error:?}");
    }
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
    assert_eq!(comparisons, 0, "tag dispatch belongs in the backend");
    assert!(matches!(
        &program.functions[0].blocks[0].terminator,
        core::Terminator::Match { arms, .. } if arms.len() == 3
    ));
    assert_eq!(
        program.functions[0].blocks.len(),
        5,
        "one dispatch, three arms, and one join"
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
    instruction.operation = core::Operation::Variant(core::DataId(0), 2, None);
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

#[test]
fn products_cross_calls_and_returns_with_checked_destinations() {
    let program = compile(include_str!("../examples/product.phoebe"));
    assert!(
        program
            .functions
            .iter()
            .flat_map(|function| &function.blocks)
            .flat_map(|block| &block.instructions)
            .any(|instruction| matches!(instruction.operation, core::Operation::DestPublish(_)))
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-product-aot-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let result = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(result.status.success());
    assert_eq!(String::from_utf8(result.stdout).unwrap(), "42\n");
}

#[test]
fn mixed_product_fields_survive_places_and_block_joins() {
    let program = compile(
        "data Item { count: I64, ready: Bool } fn choose(flag: Bool) -> Item = if (flag) { Item(41, true) } else { Item(0, false) } fn main() -> I64 { var item = choose(true); item = Item(item.count + 1, item.ready); if (item.ready) { item.count } else { 0 } }",
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn products_reject_bad_fields_and_implicit_equality() {
    let cases = [
        (
            "data Pair { x: I64, y: Bool } fn main() -> I64 { val p = Pair(1, 2); p.x }",
            "expected Bool",
        ),
        (
            "data Pair { x: I64, y: Bool } fn main() -> I64 { val p = Pair(1, true); p.z }",
            "unknown field",
        ),
        (
            "data Pair { x: I64, y: Bool } fn main() -> Bool = Pair(1, true) == Pair(1, true)",
            "product equality",
        ),
        (
            "data Pair { x: I64, x: Bool } fn main() -> I64 = 1",
            "duplicate field",
        ),
    ];
    for (source, expected) in cases {
        let error = syntax::parse(source)
            .and_then(|ast| core::lower(&ast))
            .unwrap_err();
        assert!(error.message.contains(expected), "{error:?}");
    }
}

#[test]
fn verifier_checks_product_shape_before_native_lowering() {
    let mut program = compile(
        "data Pair { left: I64, ready: Bool, spare: Unit } fn main() -> I64 { val p = Pair(42, true, Unit); p.left }",
    );
    let product = program.functions[0].blocks[0]
        .instructions
        .iter_mut()
        .find(|instruction| matches!(instruction.operation, core::Operation::Product(_, _)))
        .unwrap();
    if let core::Operation::Product(_, fields) = &mut product.operation {
        fields.pop();
    }
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("product field count mismatch")
    );

    let mut program = compile(
        "data Pair { left: I64, ready: Bool } fn main() -> I64 { val p = Pair(42, true); p.left }",
    );
    let field = program.functions[0].blocks[0]
        .instructions
        .iter_mut()
        .find(|instruction| matches!(instruction.operation, core::Operation::Field(_, _)))
        .unwrap();
    if let core::Operation::Field(_, index) = &mut field.operation {
        *index = 2;
    }
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("invalid field access")
    );
}

#[test]
fn payload_sum_uses_variant_aware_match_across_backends() {
    let program = compile(include_str!("../examples/payload.phoebe"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-payload-aot-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let result = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(result.status.success());
    assert_eq!(String::from_utf8(result.stdout).unwrap(), "42\n");
    let matcher = &program.functions[0].blocks[0].terminator;
    assert!(matches!(matcher, core::Terminator::Match { arms, .. } if arms.len() == 2));
}

#[test]
fn bool_payload_and_different_variant_payload_types_agree() {
    let program = compile(
        "data Outcome { No(error: Bool), Yes(value: I64) } fn value(outcome: Outcome) -> I64 = when (outcome) { Outcome.Yes(number) -> { number }, Outcome.No(error) -> { if (error) { -1 } else { 0 } } } fn main() -> I64 = value(Outcome.Yes(42)) + value(Outcome.No(false)) + value(Outcome.No(true)) + 1",
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn payload_forms_reject_type_and_binding_mismatches() {
    let cases = [
        (
            "data Maybe { None, Some(value: I64) } fn main() -> I64 = when (Maybe.Some(1)) { Maybe.None -> { 0 }, Maybe.Some -> { 1 } }",
            "payload binding",
        ),
        (
            "data Maybe { None, Some(value: I64) } fn main() -> I64 = when (Maybe.None) { Maybe.None(x) -> { 0 }, Maybe.Some(y) -> { y } }",
            "payload binding",
        ),
        (
            "data Maybe { None, Some(value: I64) } fn main() -> I64 = when (Maybe.Some(true)) { Maybe.None -> { 0 }, Maybe.Some(x) -> { x } }",
            "expected I64",
        ),
        (
            "data Maybe { None, Some(value: I64) } fn main() -> I64 = when (Maybe.Some(1)) { Maybe.None -> { 0 }, Maybe.Some(x) -> { false } }",
            "same type",
        ),
        (
            "data Maybe { None, Some(value: I64) } fn main() -> I64 = when (Maybe.None(1)) { Maybe.None -> { 0 }, Maybe.Some(x) -> { x } }",
            "no payload",
        ),
    ];
    for (source, expected) in cases {
        let error = syntax::parse(source)
            .and_then(|ast| core::lower(&ast))
            .unwrap_err();
        assert!(error.message.contains(expected), "{error:?}");
    }
}

#[test]
fn verifier_rejects_forged_match_payload_parameter() {
    let mut program = compile(include_str!("../examples/payload.phoebe"));
    let target =
        if let core::Terminator::Match { arms, .. } = &program.functions[0].blocks[0].terminator {
            arms[1].target
        } else {
            panic!("expected match")
        };
    let param = program.functions[0].blocks[target.0].params[0];
    program.functions[0].value_types[param.0] = core::Type::Bool;
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("match payload parameter mismatch")
    );
}

#[test]
fn verifier_rejects_match_edge_binding_mismatch() {
    let mut program = compile(include_str!("../examples/payload.phoebe"));
    if let core::Terminator::Match { arms, .. } = &mut program.functions[0].blocks[0].terminator {
        arms[1].binding = core::MatchBinding::Whole;
    } else {
        panic!("expected match");
    }
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("match payload parameter mismatch")
    );
}

#[test]
fn result_question_mark_propagates_original_error_across_backends() {
    let program = compile(include_str!("../examples/try.phoebe"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-try-aot-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let result = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(result.status.success());
    assert_eq!(String::from_utf8(result.stdout).unwrap(), "42\n");

    let (_, plus_one) = program.function("plus_one").unwrap();
    let (subject, arms) = plus_one
        .blocks
        .iter()
        .find_map(|block| {
            if let core::Terminator::Match { subject, arms } = &block.terminator {
                Some((*subject, arms))
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(arms[1].binding, core::MatchBinding::Whole);
    let failure = &plus_one.blocks[arms[1].target.0];
    assert!(
        matches!(failure.terminator, core::Terminator::Return(value) if value == failure.params[0])
    );
    assert_eq!(
        plus_one.value_types[failure.params[0].0],
        plus_one.value_types[subject.0]
    );
    assert!(failure.instructions.is_empty());
}

#[test]
fn question_mark_checks_result_shape_and_return_type() {
    let cases = [
        ("fn main() -> I64 = 1?", "? requires Result"),
        (
            "data Maybe { Ok(value: I64), Err(error: I64) } fn use(x: Maybe) -> Maybe { val y = x?; Maybe.Ok(y) } fn main() -> I64 = 1",
            "? requires Result",
        ),
        (
            "fn use(x: Result<I64,I64>) -> I64 = x? fn main() -> I64 = 1",
            "same error type",
        ),
        (
            "fn use(x: Result<Unknown,I64>) -> Result<Unknown,I64> = x fn main() -> I64 = 1",
            "unknown data type Unknown",
        ),
        (
            "data Result { Ok(value: I64), Err(error: I64) } fn main() -> I64 = 1",
            "duplicate data type",
        ),
        (
            "fn use(x: Result<I64,Bool>) -> Result<I64,I64> { val y = x?; Result<I64,I64>.Ok(y) } fn main() -> I64 = 1",
            "same error type",
        ),
    ];
    for (source, expected) in cases {
        let error = syntax::parse(source)
            .and_then(|ast| core::lower(&ast))
            .unwrap_err();
        assert!(error.message.contains(expected), "{error:?}");
    }
}

#[test]
fn question_mark_rewraps_shared_error_across_different_success_types() {
    let source = r#"
data Problem { Missing, Invalid }
fn read(x: I64) -> Result<I64, Problem> = if (x < 0) {
  Result<I64, Problem>.Err(Problem.Missing)
} else {
  Result<I64, Problem>.Ok(x)
}
fn positive(x: I64) -> Result<Bool, Problem> {
  val number = read(x)?;
  Result<Bool, Problem>.Ok(number > 0)
}
fn score(x: I64) -> I64 = when (positive(x)) {
  Result<Bool, Problem>.Ok(value) -> { if (value) { 40 } else { 0 } },
  Result<Bool, Problem>.Err(error) -> { when (error) {
    Problem.Missing -> { 2 },
    Problem.Invalid -> { 0 },
  } },
}
fn main() -> I64 { yield; score(1) + score(-1) }
"#;
    let program = compile(source);
    let (_, positive) = program.function("positive").unwrap();
    let (subject, arms) = positive
        .blocks
        .iter()
        .find_map(|block| match &block.terminator {
            core::Terminator::Match { subject, arms } => Some((*subject, arms)),
            _ => None,
        })
        .unwrap();
    assert_eq!(arms[1].binding, core::MatchBinding::Payload);
    let failure = &positive.blocks[arms[1].target.0];
    assert_eq!(failure.params.len(), 1);
    assert_ne!(positive.value_types[subject.0], positive.return_type);
    assert!(matches!(
        failure.instructions.as_slice(),
        [core::Instruction { operation: core::Operation::Variant(_, 1, Some(value)), .. }]
            if *value == failure.params[0]
    ));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-try-rewrap-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn nested_question_marks_handle_bool_errors_and_reversed_arms() {
    let program = compile(
        "fn step(x: I64) -> Result<I64,Bool> = if (x < 0) { Result<I64,Bool>.Err(true) } else { Result<I64,Bool>.Ok(x) } fn twice(x: I64) -> Result<I64,Bool> { val first = step(x)?; val second = step(first)? + 1; Result<I64,Bool>.Ok(second * 2) } fn inspect(result: Result<I64,Bool>) -> I64 = when (result) { Result<I64,Bool>.Err(error) -> { if (error) { -1 } else { 0 } }, Result<I64,Bool>.Ok(value) -> { value } } fn main() -> I64 = inspect(twice(20)) + inspect(twice(-1)) + 1",
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn concrete_option_nests_in_result_and_nominal_payloads() {
    let program = compile(include_str!("../examples/option.phoebe"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-option-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");

    for source in [
        "data Option { None, Some(value: I64) } fn main() -> I64 = 0",
        "fn main() -> I64 = Option<I64>.None?",
        "fn main() -> I64 = when (Option<I64>.Some(1)) { Option<I64>.Some(value) -> { value } }",
        "fn main() -> I64 = Option<Sender<I64>>.None",
    ] {
        assert!(
            syntax::parse(source)
                .and_then(|ast| core::lower(&ast))
                .is_err(),
            "unexpectedly accepted {source}"
        );
    }
    let missing = syntax::parse("fn main() -> Option = 0").unwrap_err();
    assert!(
        missing
            .message
            .contains("Option requires one type argument")
    );
}

#[test]
fn option_of_result_can_live_in_a_product_field() {
    let source = r#"
data Slot { item: Option<Result<I64, Bool>> }
fn main() -> I64 {
  val slot = Slot(Option<Result<I64, Bool>>.Some(Result<I64, Bool>.Ok(42)));
  when (slot.item) {
    Option<Result<I64, Bool>>.None -> { 0 },
    Option<Result<I64, Bool>>.Some(result) -> { when (result) {
      Result<I64, Bool>.Ok(value) -> { value },
      Result<I64, Bool>.Err(error) -> { 0 },
    } },
  }
}
"#;
    let program = compile(source);
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn nested_result_payloads_and_product_payloads_cross_all_backends() {
    let program = compile(include_str!("../examples/nested_result.phoebe"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path =
        std::env::temp_dir().join(format!("phoebe-nested-result-aot-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let result = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(result.status.success());
    assert_eq!(String::from_utf8(result.stdout).unwrap(), "42\n");
}

#[test]
fn wide_direct_result_uses_explicit_return_buffer() {
    let source = r#"
data Wide { a: I64, b: I64, c: I64, d: I64, e: I64, f: I64, g: I64, h: I64, i: I64, yes: Bool }

fn make() -> Wide = Wide(1, 2, 3, 4, 5, 6, 7, 8, 9, true)
fn sum(value: Wide) -> I64 =
  if (value.yes) {
    value.a + value.b + value.c + value.d + value.e + value.f + value.g + value.h + value.i
  } else { 0 }
fn main() -> I64 = sum(make())
"#;
    let program = compile(source);
    assert!(native::clif(&program).unwrap().contains("sret"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 45);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 45);
    let path = std::env::temp_dir().join(format!("phoebe-wide-aot-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let result = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(result.status.success());
    assert_eq!(String::from_utf8(result.stdout).unwrap(), "45\n");
}

#[test]
fn verifier_rejects_recursive_result_payload_shape() {
    let mut program = compile(include_str!("../examples/nested_result.phoebe"));
    let nested = program
        .data
        .iter()
        .position(|definition| definition.name == "Result<Result<Pair,Stop>,Bool>")
        .unwrap();
    program.data[nested].payloads[0] = Some(core::Type::Data(core::DataId(nested)));
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("invalid variant payload type")
    );
}

#[test]
fn closed_nominal_value_can_be_used_after_an_owning_call() {
    let source = "data Pair { left: I64, right: I64 } fn use(p: Pair) -> I64 = p.left fn main() -> I64 { val p = Pair(20, 22); val first = use(p); first + use(p) }";
    let program = compile(source);
    assert_eq!(program.interpret("main", &[]).unwrap(), 40);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 40);
}

#[test]
fn closed_nominal_reuse_follows_branch_and_loop_paths() {
    let accepted = compile(
        "data Pair { left: I64, right: I64 } fn use(p: Pair) -> I64 = p.left fn main() -> I64 { val p = Pair(20, 22); if (true) { use(p) } else { use(p) } + 22 }",
    );
    assert_eq!(accepted.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&accepted, "main", &[]).unwrap(), 42);

    let after_join = "data Pair { left: I64, right: I64 } fn use(p: Pair) -> I64 = p.left fn main() -> I64 { val p = Pair(20, 22); val n = if (true) { use(p) } else { 0 }; n + use(p) }";
    let after_join = compile(after_join);
    assert_eq!(after_join.interpret("main", &[]).unwrap(), 40);
    assert_eq!(native::run(&after_join, "main", &[]).unwrap(), 40);

    let in_loop = "data Pair { left: I64, right: I64 } fn use(p: Pair) -> I64 = p.left fn main() -> I64 { val p = Pair(20, 22); var i = 0; while (i < 2) { use(p); i = i + 1; } 42 }";
    let in_loop = compile(in_loop);
    assert_eq!(in_loop.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&in_loop, "main", &[]).unwrap(), 42);
}

#[test]
fn closed_nominal_place_remains_live_after_call() {
    let source = "data Pair { left: I64, right: I64 } fn use(p: Pair) -> I64 = p.left fn main() -> I64 { var p = Pair(20, 22); val n = use(p); p.left + n }";
    let source = compile(source);
    assert_eq!(source.interpret("main", &[]).unwrap(), 40);
    assert_eq!(native::run(&source, "main", &[]).unwrap(), 40);

    let accepted = compile(
        "data Pair { left: I64, right: I64 } fn use(p: Pair) -> I64 = p.left fn main() -> I64 { var p = Pair(20, 22); val n = use(p); p = Pair(n, 2); p.left + p.right + 20 }",
    );
    assert_eq!(accepted.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&accepted, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-move-aot-{}", std::process::id()));
    native::build_executable(&accepted, &path).unwrap();
    let result = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(result.status.success());
    assert_eq!(String::from_utf8(result.stdout).unwrap(), "42\n");
}

#[test]
fn final_closed_place_use_transfers_across_calls_branches_and_reassignment() {
    let cases = [
        (
            "data Pair { left: I64, right: I64 } fn use(p: Pair) -> I64 = p.left fn main() -> I64 { var p = Pair(20, 22); val first = use(p); first + use(p) }",
            40,
            1,
            1,
        ),
        (
            "data Pair { left: I64, right: I64 } fn use(p: Pair) -> I64 = p.left fn main() -> I64 { var p = Pair(20, 22); val first = if (true) { use(p) } else { 0 }; first + use(p) }",
            40,
            1,
            1,
        ),
        (
            "data Pair { left: I64, right: I64 } fn use(p: Pair) -> I64 = p.left fn main() -> I64 { var p = Pair(20, 22); val first = use(p); p = Pair(22, 20); first + use(p) }",
            42,
            0,
            2,
        ),
    ];
    for (source, expected, copies, takes) in cases {
        let program = compile(source);
        let (main, _) = program.function("main").unwrap();
        let operations = program.functions[main.0]
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .map(|instruction| &instruction.operation)
            .collect::<Vec<_>>();
        assert_eq!(
            operations
                .iter()
                .filter(|operation| matches!(operation, core::Operation::InferredCopyPlace(_)))
                .count(),
            copies
        );
        assert_eq!(
            operations
                .iter()
                .filter(|operation| matches!(operation, core::Operation::Take(_)))
                .count(),
            takes
        );
        assert_eq!(
            program
                .cost_events()
                .iter()
                .filter(|event| event.kind == "copy.value")
                .count(),
            copies
        );
        assert_eq!(program.interpret("main", &[]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
    }
}

#[test]
fn loop_backedge_keeps_closed_place_live() {
    let source = "data Pair { left: I64, right: I64 } fn use(p: Pair) -> I64 = p.left fn main() -> I64 { var p = Pair(20, 22); var i = 0; while (i < 2) { use(p); i = i + 1; } use(p) + 22 }";
    let program = compile(source);
    let (main, _) = program.function("main").unwrap();
    let operations = program.functions[main.0]
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .map(|instruction| &instruction.operation)
        .collect::<Vec<_>>();
    assert_eq!(
        operations
            .iter()
            .filter(|operation| matches!(operation, core::Operation::InferredCopyPlace(_)))
            .count(),
        1
    );
    assert_eq!(
        operations
            .iter()
            .filter(|operation| matches!(operation, core::Operation::Take(_)))
            .count(),
        1
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn verifier_rejects_forged_second_nominal_move() {
    let mut program = compile(
        "data Pair { left: I64, right: I64 } fn use(p: Pair) -> I64 = p.left fn main() -> I64 { val first = Pair(20, 22); val n = use(first); n + use(first) }",
    );
    let (main_id, _) = program.function("main").unwrap();
    let copy = program.functions[main_id.0].blocks[0]
        .instructions
        .iter()
        .enumerate()
        .filter_map(|(index, instruction)| {
            if let core::Operation::InferredCopy(value) = instruction.operation {
                Some((index, value))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(copy.len(), 1);
    program.functions[main_id.0].blocks[0].instructions[copy[0].0].operation =
        core::Operation::Move(copy[0].1);
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("use after move")
    );
}

#[test]
fn explicit_nominal_copies_preserve_owners_and_record_costs() {
    let program = compile(include_str!("../examples/copy.phoebe"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-copy-aot-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let result = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(result.status.success());
    assert_eq!(String::from_utf8(result.stdout).unwrap(), "42\n");

    let events = program.cost_events();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.reason == "explicit")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.reason == "inferred value use")
            .count(),
        0
    );
    assert!(
        events
            .iter()
            .all(|event| event.kind == "copy.value" && event.type_name == "Pair")
    );
    assert!(program.to_string().contains("copy.value"));
    assert!(
        compile(include_str!("../examples/product.phoebe"))
            .cost_events()
            .is_empty()
    );
}

#[test]
fn explicit_copy_remains_available_after_inferred_value_use() {
    let source = "data Pair { left: I64, right: I64 } fn use(p: Pair) -> I64 = p.left fn main() -> I64 { val p = Pair(20,22); val a = use(p); a + use(copy(p)) }";
    let program = compile(source);
    assert_eq!(program.interpret("main", &[]).unwrap(), 40);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 40);
}

#[test]
fn copying_a_scalar_has_no_cost_event() {
    let program = compile("fn main() -> I64 { val x = 21; copy(x) + x }");
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert!(program.cost_events().is_empty());
}

#[test]
fn concrete_result_fields_and_variant_payloads_cross_all_backends() {
    let program = compile(include_str!("../examples/result_in_data.phoebe"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-result-data-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n");

    let recursive = "data Loop { value: Result<Loop, Bool> } fn main() -> I64 = 0";
    let error = core::lower(&syntax::parse(recursive).unwrap()).unwrap_err();
    assert!(
        error.message.contains("unknown data type Loop"),
        "{error:?}"
    );
    assert!(error.span.start > 0);
}
