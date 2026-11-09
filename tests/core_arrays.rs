use phoebe::Span;
use phoebe::core::{
    self, Block, BlockId, Function, Instruction, Operation, Program, Terminator, Type, ValueId,
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

fn program(retained: bool) -> Program {
    let mut blocks = vec![Block {
        params: vec![],
        instructions: vec![
            instruction(0, Operation::Const(40)),
            instruction(1, Operation::Const(2)),
            instruction(2, Operation::ArrayI64(vec![ValueId(0), ValueId(1)])),
        ],
        terminator: if retained {
            Terminator::Yield {
                resume: BlockId(1),
                span: Span { start: 0, end: 0 },
                automatic: false,
            }
        } else {
            Terminator::Jump(BlockId(1), vec![])
        },
    }];
    blocks.push(Block {
        params: vec![],
        instructions: vec![
            instruction(3, Operation::ArrayGet(ValueId(2), 0)),
            instruction(4, Operation::ArrayLen(ValueId(2))),
            instruction(5, Operation::Add(ValueId(3), ValueId(4))),
        ],
        terminator: Terminator::Return(ValueId(5)),
    });
    Program {
        data: vec![],
        functions: vec![Function {
            name: "main".into(),
            param_modes: vec![],
            return_type: Type::I64,
            value_types: vec![
                Type::I64,
                Type::I64,
                Type::ArrayI64(2),
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
fn fixed_array_agrees_in_direct_and_retained_execution() {
    for retained in [false, true] {
        let program = program(retained);
        core::verify(&program).unwrap();
        assert_eq!(program.interpret("main", &[]).unwrap(), 42);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
        let path = std::env::temp_dir().join(format!(
            "phoebe-core-array-{}-{retained}",
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

#[test]
fn fixed_array_checks_shape_and_constant_index_before_execution() {
    let mut bad_index = program(false);
    bad_index.functions[0].blocks[1].instructions[0].operation = Operation::ArrayGet(ValueId(2), 2);
    assert!(
        core::verify(&bad_index)
            .unwrap_err()
            .contains("constant array index")
    );

    let mut oversized = program(false);
    oversized.functions[0].value_types[2] = Type::ArrayI64(257);
    assert!(
        core::verify(&oversized)
            .unwrap_err()
            .contains("exceeds 256")
    );
}

#[test]
fn direct_fixed_array_has_no_storage_or_dynamic_bounds_work() {
    let program = program(false);
    assert!(program.cost_events().is_empty());
    let clif = native::clif(&program).unwrap();
    assert!(!clif.contains("call "), "{clif}");
    assert!(!clif.contains("stack_slot"), "{clif}");
}

#[test]
fn explicit_array_copy_is_visible_in_cost_ledger() {
    let mut program = program(false);
    program.functions[0].value_types.push(Type::ArrayI64(2));
    program.functions[0].blocks[0]
        .instructions
        .push(instruction(6, Operation::Copy(ValueId(2))));
    core::verify(&program).unwrap();
    let events = program.cost_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "copy.value");
    assert_eq!(events[0].type_name, "[I64; 2]");
}

#[test]
fn empty_fixed_array_is_zero_width_and_has_length_zero() {
    let mut program = program(false);
    program.functions[0].value_types.truncate(4);
    program.functions[0].value_types[2] = Type::ArrayI64(0);
    program.functions[0].blocks[0].instructions[2].operation = Operation::ArrayI64(vec![]);
    program.functions[0].blocks[1].instructions =
        vec![instruction(3, Operation::ArrayLen(ValueId(2)))];
    program.functions[0].blocks[1].terminator = Terminator::Return(ValueId(3));
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 0);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 0);
}
