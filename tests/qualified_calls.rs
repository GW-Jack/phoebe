use phoebe::{core, syntax};

#[test]
fn dotted_call_accepts_multiple_arguments_for_future_modules() {
    let ast = syntax::parse("fn main() -> I64 = math.add(20, 22)").unwrap();
    let syntax::ExprKind::QualifiedCall(module, function, args) =
        &ast.functions[0].body.tail.as_ref().unwrap().kind
    else {
        panic!("expected a qualified call");
    };
    assert_eq!(module, "math");
    assert_eq!(function, "add");
    assert_eq!(args.len(), 2);
    assert!(
        core::lower(&ast)
            .unwrap_err()
            .message
            .contains("unknown function math.add")
    );
}

#[test]
fn dotted_call_accepts_zero_arguments_for_future_modules() {
    let ast = syntax::parse("fn main() -> I64 = clock.now()").unwrap();
    let syntax::ExprKind::QualifiedCall(_, _, args) =
        &ast.functions[0].body.tail.as_ref().unwrap().kind
    else {
        panic!("expected a qualified call");
    };
    assert!(args.is_empty());
}

#[test]
fn dotted_variant_constructor_still_type_checks() {
    let source = "data Answer { Ok(value: I64), Err(value: I64) } fn main() -> I64 = when (Answer.Ok(42)) { Answer.Ok(value) -> { value }, Answer.Err(value) -> { value } }";
    let ast = syntax::parse(source).unwrap();
    let program = core::lower(&ast).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
}
