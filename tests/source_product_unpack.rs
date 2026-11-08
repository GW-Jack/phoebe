use phoebe::{core, native, syntax};
use std::process::Command;

#[test]
fn source_nested_product_unpack_agrees_across_execution_paths() {
    let source = include_str!("../examples/product_unpack.phoebe");
    for retained in [false, true] {
        let source = if retained {
            source.replace("  val (pair, bonus)", "  yield;\n  val (pair, bonus)")
        } else {
            source.to_owned()
        };
        let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
        assert_eq!(program.to_string().matches("product.unpack").count(), 2);
        assert_eq!(program.interpret("main", &[]).unwrap(), 42);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
        if retained {
            let path = std::env::temp_dir().join(format!(
                "phoebe-source-product-unpack-{}",
                std::process::id()
            ));
            native::build_executable(&program, &path).unwrap();
            let output = Command::new(&path).output().unwrap();
            std::fs::remove_file(path).unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(output.stdout, b"42\n");
        }
    }
}

#[test]
fn unpack_rejects_wrong_field_count_and_duplicate_names() {
    let source = include_str!("../examples/product_unpack.phoebe");
    let wrong_count = source.replace("(pair, bonus)", "(pair)");
    let error = core::lower(&syntax::parse(&wrong_count).unwrap()).unwrap_err();
    assert!(error.message.contains("unpack requires 2 binding name"));

    let duplicate = source.replace("(pair, bonus)", "(pair, pair)");
    let error = core::lower(&syntax::parse(&duplicate).unwrap()).unwrap_err();
    assert!(error.message.contains("duplicate local pair"));
}

#[test]
fn unpack_binding_names_participate_in_incremental_ast_identity() {
    let source = include_str!("../examples/product_unpack.phoebe");
    let previous = syntax::parse(source).unwrap();
    let shifted = syntax::parse(&format!("\n{source}")).unwrap();
    assert!(syntax::same_function_with_shift(
        &previous.functions[0],
        &shifted.functions[0],
        1,
    ));

    let edited = syntax::parse(&source.replace("(pair, bonus)", "(pair, extra)")).unwrap();
    assert!(!syntax::same_function_with_shift(
        &previous.functions[0],
        &edited.functions[0],
        0,
    ));
}

#[test]
fn structural_close_consumes_a_closed_product_without_runtime_context() {
    for retained in [false, true] {
        let source = format!(
            "data Pair {{ left: I64, right: I64 }}\nfn main() -> I64 {{\n  val pair = Pair(19, 23);\n  {}pair.close_caps();\n  42\n}}",
            if retained { "yield;\n  " } else { "" }
        );
        let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
        assert!(program.to_string().contains("close.caps"));
        assert_eq!(program.runtime_context_functions(), vec![retained]);
        if !retained {
            assert!(!native::clif(&program).unwrap().contains("call"));
        }
        assert_eq!(program.interpret("main", &[]).unwrap(), 42);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    }

    let reused = "data Pair { left: I64, right: I64 }\nfn main() -> I64 { val pair = Pair(1, 2); pair.close_caps(); pair.left }";
    let error = core::lower(&syntax::parse(reused).unwrap()).unwrap_err();
    assert!(error.message.contains("use after move"));
}
