use phoebe::Span;
use phoebe::core::{
    self, Block, BlockId, Function, Instruction, Operation, Program, Terminator, Type, ValueId,
};
use phoebe::native;

fn span() -> Span {
    Span { start: 0, end: 0 }
}

fn instruction(result: Option<usize>, operation: Operation) -> Instruction {
    Instruction {
        result: result.map(ValueId),
        operation,
        span: span(),
    }
}

fn channel_program() -> Program {
    Program {
        data: vec![],
        functions: vec![Function {
            name: "main".into(),
            param_modes: vec![],
            return_type: Type::I64,
            value_types: vec![
                Type::I64,
                Type::SenderI64,
                Type::ReceiverI64,
                Type::ResourceError,
                Type::I64,
                Type::I64,
            ],
            place_types: vec![],
            blocks: vec![
                Block {
                    params: vec![],
                    instructions: vec![instruction(Some(0), Operation::Const(1))],
                    terminator: Terminator::ChannelNew {
                        capacity: ValueId(0),
                        success: BlockId(1),
                        failure: BlockId(2),
                    },
                },
                Block {
                    params: vec![ValueId(1), ValueId(2)],
                    instructions: vec![
                        instruction(None, Operation::CloseHandle(ValueId(1))),
                        instruction(None, Operation::CloseHandle(ValueId(2))),
                        instruction(Some(4), Operation::Const(7)),
                    ],
                    terminator: Terminator::Return(ValueId(4)),
                },
                Block {
                    params: vec![ValueId(3)],
                    instructions: vec![instruction(Some(5), Operation::Const(0))],
                    terminator: Terminator::Return(ValueId(5)),
                },
            ],
        }],
    }
}

#[test]
fn checked_channel_creation_has_two_success_rights_and_a_typed_failure() {
    let program = channel_program();
    core::verify(&program).unwrap();
    let printed = program.to_string();
    assert!(printed.contains("channel.new.i64 v0 success block1 failure block2"));
    assert!(printed.contains("close.handle v1"));
    assert!(
        program
            .interpret("main", &[])
            .unwrap_err()
            .contains("runtime bridge")
    );
    assert!(
        native::compile_jit(&program, "main")
            .err()
            .unwrap()
            .contains("runtime ABI")
    );
}

#[test]
fn channel_creation_rejects_a_forgotten_right() {
    let mut program = channel_program();
    program.functions[0].blocks[1].instructions.remove(1);
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("normal return leaks")
    );

    let mut program = channel_program();
    program.functions[0].blocks[1]
        .instructions
        .insert(1, instruction(None, Operation::CloseHandle(ValueId(1))));
    assert!(core::verify(&program).unwrap_err().contains("is not live"));
}

#[test]
fn channel_rights_move_through_typed_block_arguments() {
    let mut program = channel_program();
    let function = &mut program.functions[0];
    function.blocks[1].instructions.drain(0..2);
    function.blocks[1].terminator = Terminator::Jump(BlockId(3), vec![ValueId(1), ValueId(2)]);
    function
        .value_types
        .extend([Type::SenderI64, Type::ReceiverI64, Type::I64]);
    function.blocks.push(Block {
        params: vec![ValueId(6), ValueId(7)],
        instructions: vec![
            instruction(None, Operation::CloseHandle(ValueId(6))),
            instruction(None, Operation::CloseHandle(ValueId(7))),
            instruction(Some(8), Operation::Const(9)),
        ],
        terminator: Terminator::Return(ValueId(8)),
    });
    core::verify(&program).unwrap();
    program.functions[0].blocks[3].instructions.remove(1);
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("normal return leaks")
    );
}

#[test]
fn channel_creation_checks_edge_signatures_and_distinctness() {
    let mut program = channel_program();
    program.functions[0].value_types[1] = Type::ReceiverI64;
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("edge signature mismatch")
    );

    let mut program = channel_program();
    if let Terminator::ChannelNew { failure, .. } = &mut program.functions[0].blocks[0].terminator {
        *failure = BlockId(1);
    }
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("creation edges must be distinct")
    );

    let mut program = channel_program();
    program.functions[0].value_types[0] = Type::Bool;
    program.functions[0].blocks[0].instructions[0].operation = Operation::Bool(true);
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("capacity must be I64")
    );
}
