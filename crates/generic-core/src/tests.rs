use crate::{Block, BlockId, Function, Instruction, Operation, Terminator, Type, ValueId, verify};

fn identity() -> Function {
    Function {
        name: "identity".into(),
        type_params: 1,
        params: vec![Type::Param(0)],
        result: Type::Param(0),
        value_types: vec![Type::Param(0)],
        blocks: vec![Block {
            params: vec![ValueId(0)],
            instructions: Vec::new(),
            terminator: Terminator::Return(ValueId(0)),
        }],
    }
}

#[test]
fn opaque_identity_is_checked_without_a_concrete_abi_shape() {
    verify(&identity()).unwrap();
    let mut moved = identity();
    moved.value_types.push(Type::Param(0));
    moved.blocks[0].instructions.push(Instruction {
        result: ValueId(1),
        operation: Operation::Move(ValueId(0)),
    });
    moved.blocks[0].terminator = Terminator::Return(ValueId(1));
    verify(&moved).unwrap();
    moved.blocks[0].terminator = Terminator::Return(ValueId(0));
    assert!(verify(&moved).unwrap_err().contains("not live"));
}

#[test]
fn branch_join_forwards_one_opaque_owner_on_each_path() {
    let function = Function {
        name: "choose".into(),
        type_params: 1,
        params: vec![Type::Param(0), Type::Bool],
        result: Type::Param(0),
        value_types: vec![
            Type::Param(0),
            Type::Bool,
            Type::Param(0),
            Type::Param(0),
            Type::Param(0),
        ],
        blocks: vec![
            Block {
                params: vec![ValueId(0), ValueId(1)],
                instructions: Vec::new(),
                terminator: Terminator::Branch {
                    condition: ValueId(1),
                    when_true: BlockId(1),
                    true_args: vec![ValueId(0)],
                    when_false: BlockId(2),
                    false_args: vec![ValueId(0)],
                },
            },
            Block {
                params: vec![ValueId(2)],
                instructions: Vec::new(),
                terminator: Terminator::Jump {
                    target: BlockId(3),
                    args: vec![ValueId(2)],
                },
            },
            Block {
                params: vec![ValueId(3)],
                instructions: Vec::new(),
                terminator: Terminator::Jump {
                    target: BlockId(3),
                    args: vec![ValueId(3)],
                },
            },
            Block {
                params: vec![ValueId(4)],
                instructions: Vec::new(),
                terminator: Terminator::Return(ValueId(4)),
            },
        ],
    };
    verify(&function).unwrap();
    let mut invalid = function;
    invalid.blocks[2].terminator = Terminator::Jump {
        target: BlockId(3),
        args: vec![ValueId(2)],
    };
    assert!(
        verify(&invalid)
            .unwrap_err()
            .contains("not defined in this block")
    );
}

#[test]
fn opaque_owner_cannot_be_copied_lost_or_sent_twice() {
    let mut copied = identity();
    copied.value_types.push(Type::Param(0));
    copied.blocks[0].instructions.push(Instruction {
        result: ValueId(1),
        operation: Operation::CopyScalar(ValueId(0)),
    });
    copied.blocks[0].terminator = Terminator::Return(ValueId(1));
    assert!(verify(&copied).unwrap_err().contains("cannot be copied"));

    let leaked = Function {
        name: "leaked".into(),
        type_params: 1,
        params: vec![Type::Param(0), Type::I64],
        result: Type::I64,
        value_types: vec![Type::Param(0), Type::I64],
        blocks: vec![Block {
            params: vec![ValueId(0), ValueId(1)],
            instructions: Vec::new(),
            terminator: Terminator::Return(ValueId(1)),
        }],
    };
    assert!(verify(&leaked).unwrap_err().contains("normal return leaks"));

    let duplicate = Function {
        name: "duplicate".into(),
        type_params: 1,
        params: vec![Type::Param(0)],
        result: Type::Param(0),
        value_types: vec![Type::Param(0); 3],
        blocks: vec![
            Block {
                params: vec![ValueId(0)],
                instructions: Vec::new(),
                terminator: Terminator::Jump {
                    target: BlockId(1),
                    args: vec![ValueId(0), ValueId(0)],
                },
            },
            Block {
                params: vec![ValueId(1), ValueId(2)],
                instructions: Vec::new(),
                terminator: Terminator::Return(ValueId(1)),
            },
        ],
    };
    assert!(verify(&duplicate).unwrap_err().contains("moved twice"));
}

#[test]
fn malformed_type_binder_and_unreachable_block_are_rejected() {
    let mut unbound = identity();
    unbound.result = Type::Param(1);
    assert!(
        verify(&unbound)
            .unwrap_err()
            .contains("unbound type parameter")
    );

    let mut unreachable = identity();
    unreachable.value_types.push(Type::Param(0));
    unreachable.blocks.push(Block {
        params: vec![ValueId(1)],
        instructions: Vec::new(),
        terminator: Terminator::Return(ValueId(1)),
    });
    assert!(
        verify(&unreachable)
            .unwrap_err()
            .contains("unreachable block")
    );
}
