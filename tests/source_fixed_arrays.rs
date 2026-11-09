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
