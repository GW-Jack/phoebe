use phoebe::{core, native, syntax};
use std::process::Command;

fn lower(source: &str) -> core::Program {
    core::lower(&syntax::parse(source).unwrap()).unwrap()
}

#[test]
fn fixed_array_source_agrees_across_execution_paths() {
    for (source, expected) in [
        ("fn main() -> I64 { val a = [40, 2]; a[0] + a.len }", 42),
        (
            "fn main() -> I64 { val a = [40, 2]; yield; a[0] + a.len }",
            42,
        ),
        ("fn main() -> I64 { var a = [40, 2]; a[0] + a.len }", 42),
        (
            "fn main() -> I64 { var a = [40, 2]; yield; a[0] + a.len }",
            42,
        ),
        ("fn main() -> I64 = [40, 2][1] + 40", 42),
        ("fn main() -> I64 { val a = []; a.len }", 0),
    ] {
        let program = lower(source);
        assert_eq!(program.interpret("main", &[]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
        let path = std::env::temp_dir().join(format!(
            "phoebe-source-array-{}-{expected}",
            std::process::id()
        ));
        native::build_executable(&program, &path).unwrap();
        let output = Command::new(&path).output().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
    }
}

#[test]
fn fixed_array_reads_need_no_copy_or_bounds_site() {
    let program = lower("fn main() -> I64 { val a = [40, 2]; a[0] + a.len }");
    assert!(program.cost_events().is_empty());
    assert!(!program.to_string().contains("copy.value"));
    let clif = native::clif(&program).unwrap();
    assert!(!clif.contains("call "), "{clif}");
    assert!(!clif.contains("stack_slot"), "{clif}");

    let mutable = lower("fn main() -> I64 { var a = [40, 2]; a[0] + a.len }");
    assert!(mutable.cost_events().is_empty());
    assert!(mutable.to_string().contains("place.array.get"));
}

#[test]
fn fixed_array_source_rejects_wrong_element_and_index() {
    let wrong_element = "fn main() -> I64 = [40, false][0]";
    assert!(
        core::lower(&syntax::parse(wrong_element).unwrap())
            .unwrap_err()
            .message
            .contains("expected I64")
    );
    let bad_index = "fn main() -> I64 = [40, 2][2]";
    assert!(
        core::lower(&syntax::parse(bad_index).unwrap())
            .unwrap_err()
            .message
            .contains("constant array index is out of bounds")
    );
    let dynamic_index = "fn main() -> I64 { val a = [40, 2]; val i = 1; a[i] }";
    assert!(
        syntax::parse(dynamic_index)
            .unwrap_err()
            .message
            .contains("fixed array index")
    );
}

#[test]
fn indexed_array_update_preserves_prior_values_across_backends() {
    for source in [
        "fn main() -> I64 { var a = [40, 0]; val prior = a; a[0] = 2; prior[0] + a[0] }",
        "fn main() -> I64 { var a = [40, 0]; val prior = a; yield; a[0] = 2; prior[0] + a[0] }",
        "fn main() -> I64 { var a = [40, 0]; var i = 0; while (i < 2) { a[1] = a[1] + 1; i = i + 1; } a[0] + a[1] }",
    ] {
        let program = lower(source);
        assert_eq!(program.interpret("main", &[]).unwrap(), 42);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
        let path = std::env::temp_dir().join(format!(
            "phoebe-array-update-{}-{}",
            std::process::id(),
            source.len()
        ));
        native::build_executable(&program, &path).unwrap();
        let output = Command::new(&path).output().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"42\n");
    }
    let copied =
        lower("fn main() -> I64 { var a = [40, 0]; val prior = a; a[0] = 2; prior[0] + a[0] }");
    assert!(copied.to_string().contains("place.array.set"));
    assert_eq!(
        copied
            .cost_events()
            .iter()
            .filter(|event| event.kind == "copy.value")
            .count(),
        1
    );
}

#[test]
fn indexed_array_update_checks_mutability_bounds_and_element_type() {
    for (source, message) in [
        (
            "fn main() -> I64 { val a = [40, 2]; a[0] = 3; a[0] }",
            "requires a var",
        ),
        (
            "fn main() -> I64 { var a = [40, 2]; a[2] = 3; a[0] }",
            "out of bounds",
        ),
        (
            "fn main() -> I64 { var a = [40, 2]; a[0] = true; a[0] }",
            "expected I64",
        ),
        (
            "fn main() -> I64 { var x = 40; x[0] = 2; x }",
            "requires a fixed I64 array",
        ),
    ] {
        let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
        assert!(error.message.contains(message), "{error:?}");
    }

    let source = "fn main() -> I64 { var a = [40, 0]; val prior = a; a[0] = 2; prior[0] + a[0] }";
    let mut invalid_index = lower(source);
    for instruction in invalid_index.functions[0]
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
    {
        if let core::Operation::ArraySetPlace(_, index, _) = &mut instruction.operation {
            *index = 2;
        }
    }
    assert!(
        core::verify(&invalid_index)
            .unwrap_err()
            .contains("out of bounds")
    );

    let mut moved_before_write = lower(source);
    for instruction in moved_before_write.functions[0]
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
    {
        if let core::Operation::InferredCopyPlace(place) = instruction.operation {
            instruction.operation = core::Operation::Take(place);
        }
    }
    assert!(
        core::verify(&moved_before_write)
            .unwrap_err()
            .contains("use after move")
    );
}
