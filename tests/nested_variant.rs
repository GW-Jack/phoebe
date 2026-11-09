use phoebe::{core, native, syntax};

#[test]
fn nested_user_variants_agree_between_interpreter_and_native() {
    let source = include_str!("../examples/nested_variant.phoebe");
    let ast = syntax::parse(source).unwrap();
    let program = core::lower(&ast).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn recursive_nominal_payload_is_rejected_with_a_span() {
    let source = "data Loop { End, Next(value: Loop) } fn main() -> I64 = 0";
    let ast = syntax::parse(source).unwrap();
    let error = core::lower(&ast).unwrap_err();
    assert!(error.message.contains("declared earlier"));
    assert!(error.span.start > 0);
}

#[test]
fn nested_closed_field_projection_preserves_its_owner() {
    let source = "data Inner { value: I64 } data Outer { inner: Inner } fn main() -> I64 { val outer = Outer(Inner(42)); val inner = outer.inner; val other = outer.inner; other.value }";
    let ast = syntax::parse(source).unwrap();
    let program = core::lower(&ast).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn explicit_copy_allows_two_owned_nested_projections() {
    let source = "data Inner { value: I64 } data Outer { inner: Inner } fn main() -> I64 { val outer = Outer(Inner(21)); val duplicate = copy(outer); val first = duplicate.inner; val second = outer.inner; first.value + second.value }";
    let ast = syntax::parse(source).unwrap();
    let program = core::lower(&ast).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_eq!(
        program
            .cost_events()
            .iter()
            .filter(|event| event.reason == "explicit")
            .count(),
        1
    );
    assert_eq!(
        program
            .cost_events()
            .iter()
            .filter(|event| event.reason == "inferred field value")
            .count(),
        2
    );
}

#[test]
fn verifier_accepts_closed_nominal_place_field_read() {
    let source = "data Inner { value: I64 } data Outer { inner: Inner } fn main() -> I64 { var outer = Outer(Inner(42)); val inner = outer.inner; inner.value }";
    let ast = syntax::parse(source).unwrap();
    let program = core::lower(&ast).unwrap();
    assert!(
        program.functions[0].blocks[0]
            .instructions
            .iter()
            .any(|instruction| matches!(instruction.operation, core::Operation::ReadField(_, 0)))
    );
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}
