use phoebe::{core, syntax};

fn check_source(source: &str) {
    let result = std::panic::catch_unwind(|| match syntax::parse(source) {
        Ok(ast) => {
            let lowered = if ast
                .functions
                .iter()
                .any(|function| !function.type_params.is_empty())
            {
                core::lower_linked_checked(&ast).map(|_| ())
            } else {
                core::lower(&ast).map(|_| ())
            };
            if let Err(error) = lowered {
                assert!(error.span.start <= error.span.end);
                assert!(error.span.end <= source.len(), "{error:?}");
                let _ = error.render(source);
            }
        }
        Err(error) => {
            assert!(error.span.start <= error.span.end);
            assert!(error.span.end <= source.len(), "{error:?}");
            let _ = error.render(source);
        }
    });
    assert!(result.is_ok(), "compiler panicked on source:\n{source}");
}

#[test]
fn nearby_malformed_programs_return_diagnostics() {
    let seeds = [
        include_str!("../examples/spawn_pair.phoebe"),
        include_str!("../examples/nested_variant.phoebe"),
        include_str!("../examples/try.phoebe"),
        include_str!("../examples/boolean_short_circuit.phoebe"),
        include_str!("../examples/generic_call.phoebe"),
        include_str!("../examples/channel_exchange.phoebe"),
        include_str!("../examples/request_backpressure.phoebe"),
        include_str!("../examples/capability_nested_sum.phoebe"),
    ];
    let insertions: [&[u8]; 15] = [
        b"(", b")", b"{", b"}", b"?", b".", b"\n", b"fn ", b"val ", b"0", b"&mut ", b"send", b"&&",
        b"||", b"!",
    ];
    let replacements = b" (){}?.,:;=+*-&|!<>0az\n";
    let mut state = 0x1c5e_8532_72db_9f31_u64;
    for seed in seeds {
        check_source(seed);
        for _ in 0..1_000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let index = (state as usize) % seed.len();
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let variant = (state >> 32) as usize;
            let mut bytes = seed.as_bytes().to_vec();
            match variant % 3 {
                0 => {
                    bytes.remove(index);
                }
                1 => {
                    bytes.splice(
                        index..index,
                        insertions[variant % insertions.len()].iter().copied(),
                    );
                }
                _ => {
                    bytes[index] = replacements[variant % replacements.len()];
                }
            }
            check_source(std::str::from_utf8(&bytes).unwrap());
        }
    }
}

#[test]
fn unsupported_unicode_has_a_renderable_diagnostic() {
    check_source("fn main() -> I64 = 🦆");
    check_source("fn main() -> I64 = 7\n🦆");
}

#[test]
fn deep_source_hits_a_location_bounded_limit() {
    let ordinary = format!("fn main() -> I64 = {}7{}", "(".repeat(20), ")".repeat(20));
    let program = core::lower(&syntax::parse(&ordinary).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 7);
    let nested = format!("fn main() -> I64 = {}1{}", "(".repeat(256), ")".repeat(256));
    let chain = format!("fn main() -> I64 = 1{}", " + 1".repeat(256));
    let mut ty = "I64".to_owned();
    for _ in 0..256 {
        ty = format!("Result<{ty}, I64>");
    }
    let nested_type = format!("fn main(x: {ty}) -> I64 = 0");
    let mut combined = "1".to_owned();
    for _ in 0..40 {
        combined = format!("({combined}){}", " + 1".repeat(40));
    }
    let combined = format!("fn main() -> I64 = {combined}");
    let mut branching = "0".to_owned();
    for _ in 0..256 {
        branching = format!("if (true) {{ {branching} }} else {{ 0 }}");
    }
    let branching = format!("fn main() -> I64 = {branching}");
    for source in [&nested, &chain, &nested_type, &combined, &branching] {
        let error = syntax::parse(source).unwrap_err();
        assert!(error.message.contains("compiler limit"), "{error:?}");
        assert!(error.span.end <= source.len());
        let rendered = error.render(source);
        assert!(rendered.contains("compiler limit"));
    }
}
