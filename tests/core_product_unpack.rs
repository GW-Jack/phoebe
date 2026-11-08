use phoebe::Span;
use phoebe::core::{
    self, Block, BlockId, DataDef, DataId, Function, Instruction, Operation, Program, Terminator,
    Type, ValueId,
};
use phoebe::native;
use std::process::Command;

fn instruction(result: usize, operation: Operation) -> Instruction {
    Instruction {
        result: Some(ValueId(result)),
        operation,
        span: Span { start: 0, end: 0 },
    }
}

fn block(params: &[usize], instructions: Vec<Instruction>, terminator: Terminator) -> Block {
    Block {
        params: params.iter().copied().map(ValueId).collect(),
        instructions,
        terminator,
    }
}

fn program(retained: bool) -> Program {
    let unpack = if retained { BlockId(2) } else { BlockId(1) };
    let final_block = BlockId(unpack.0 + 1);
    let mut blocks = vec![block(
        &[],
        vec![
            instruction(0, Operation::Const(40)),
            instruction(1, Operation::Const(1)),
            instruction(
                2,
                Operation::Product(DataId(0), vec![ValueId(0), ValueId(1)]),
            ),
            instruction(3, Operation::Const(1)),
            instruction(
                4,
                Operation::Product(DataId(1), vec![ValueId(2), ValueId(3)]),
            ),
        ],
        if retained {
            Terminator::Yield {
                resume: BlockId(1),
                span: Span { start: 0, end: 0 },
                automatic: false,
            }
        } else {
            Terminator::ProductUnpack {
                product: ValueId(4),
                next: unpack,
            }
        },
    )];
    if retained {
        blocks.push(block(
            &[],
            vec![],
            Terminator::ProductUnpack {
                product: ValueId(4),
                next: unpack,
            },
        ));
    }
    blocks.push(block(
        &[5, 6],
        vec![],
        Terminator::ProductUnpack {
            product: ValueId(5),
            next: final_block,
        },
    ));
    blocks.push(block(
        &[7, 8],
        vec![
            instruction(9, Operation::Add(ValueId(7), ValueId(8))),
            instruction(10, Operation::Add(ValueId(9), ValueId(6))),
        ],
        Terminator::Return(ValueId(10)),
    ));
    Program {
        data: vec![
            DataDef {
                name: "Pair".into(),
                variants: vec![],
                payloads: vec![],
                fields: vec![("a".into(), Type::I64), ("b".into(), Type::I64)],
            },
            DataDef {
                name: "Envelope".into(),
                variants: vec![],
                payloads: vec![],
                fields: vec![
                    ("pair".into(), Type::Data(DataId(0))),
                    ("c".into(), Type::I64),
                ],
            },
        ],
        functions: vec![Function {
            name: "main".into(),
            param_modes: vec![],
            return_type: Type::I64,
            value_types: vec![
                Type::I64,
                Type::I64,
                Type::Data(DataId(0)),
                Type::I64,
                Type::Data(DataId(1)),
                Type::Data(DataId(0)),
                Type::I64,
                Type::I64,
                Type::I64,
                Type::I64,
                Type::I64,
            ],
            place_types: vec![],
            loan_regions: vec![],
            blocks,
        }],
    }
}

#[test]
fn nested_product_unpack_agrees_in_direct_and_retained_execution() {
    for retained in [false, true] {
        let program = program(retained);
        core::verify(&program).unwrap();
        assert!(program.to_string().contains("product.unpack"));
        assert_eq!(program.interpret("main", &[]).unwrap(), 42);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    }
    let path = std::env::temp_dir().join(format!("phoebe-product-unpack-{}", std::process::id()));
    native::build_executable(&program(true), &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn product_unpack_requires_one_typed_parameter_per_field() {
    let mut program = program(false);
    program.functions[0].blocks[1].params.swap(0, 1);
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("unpack edge signature mismatch")
    );
}
