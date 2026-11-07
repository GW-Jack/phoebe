use std::path::Path;

use phoebe::{core, native, syntax};

#[test]
fn nested_user_variants_agree_between_interpreter_and_native() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/nested_variant.phoebe");
    let source = std::fs::read_to_string(path).unwrap();
    let ast = syntax::parse(&source).unwrap();
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
fn nested_product_field_projection_consumes_its_owner() {
    let source = "data Inner { value: I64 } data Outer { inner: Inner } fn main() -> I64 { val outer = Outer(Inner(42)); val inner = outer.inner; val other = outer.inner; other.value }";
    let ast = syntax::parse(source).unwrap();
    let error = core::lower(&ast).unwrap_err();
    assert!(
        error.message.contains("use after move"),
        "{}",
        error.message
    );
}

#[test]
fn explicit_copy_allows_two_owned_nested_projections() {
    let source = "data Inner { value: I64 } data Outer { inner: Inner } fn main() -> I64 { val outer = Outer(Inner(21)); val duplicate = copy(outer); val first = duplicate.inner; val second = outer.inner; first.value + second.value }";
    let ast = syntax::parse(source).unwrap();
    let program = core::lower(&ast).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_eq!(program.cost_events().len(), 1);
}

#[test]
fn verifier_rejects_forged_nominal_place_read() {
    let source = "data Inner { value: I64 } data Outer { inner: Inner } fn main() -> I64 { var outer = Outer(Inner(42)); val inner = outer.inner; inner.value }";
    let ast = syntax::parse(source).unwrap();
    let mut program = core::lower(&ast).unwrap();
    let operations = &mut program.functions[0].blocks[0].instructions;
    let place = operations
        .iter()
        .find_map(|instruction| match instruction.operation {
            core::Operation::Take(place) => Some(place),
            _ => None,
        })
        .unwrap();
    let field = operations
        .iter_mut()
        .find(|instruction| matches!(instruction.operation, core::Operation::Field(_, 0)))
        .unwrap();
    field.operation = core::Operation::ReadField(place, 0);
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("nominal place field requires an owning projection")
    );
}
