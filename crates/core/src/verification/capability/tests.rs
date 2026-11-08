use super::*;

#[test]
fn product_parameter_allocates_one_flow_slot_per_right() {
    let data = vec![DataDef {
        name: "Pair".into(),
        variants: vec![],
        payloads: vec![],
        fields: vec![
            ("left".into(), Type::SenderI64),
            ("right".into(), Type::ReplyToI64),
        ],
    }];
    let function = Function {
        name: "consume".into(),
        param_modes: vec![syntax::ParamMode::Own],
        return_type: Type::I64,
        value_types: vec![Type::Data(DataId(0)), Type::I64],
        place_types: vec![],
        loan_regions: vec![],
        blocks: vec![Block {
            params: vec![ValueId(0)],
            instructions: vec![Instruction {
                result: Some(ValueId(1)),
                operation: Operation::Const(0),
                span: Span { start: 0, end: 0 },
            }],
            terminator: Terminator::Return(ValueId(1)),
        }],
    };
    let (graph, owners) = capability_graph(&function, &Layout::new(&data), &[], &[]).unwrap();
    assert_eq!(graph.slots, 2);
    assert_eq!(
        graph.entry_live,
        vec![capability_flow::Slot(0), capability_flow::Slot(1)]
    );
    assert_eq!(owners, vec![ValueId(0), ValueId(0)]);
    assert!(
        capability_flow::verify(&graph)
            .unwrap_err()
            .contains("normal return leaks")
    );
}

#[test]
fn product_pack_move_and_block_argument_transfer_both_rights() {
    let data = vec![DataDef {
        name: "Pair".into(),
        variants: vec![],
        payloads: vec![],
        fields: vec![
            ("left".into(), Type::SenderI64),
            ("right".into(), Type::ReplyToI64),
        ],
    }];
    let function = Function {
        name: "pass_pair".into(),
        param_modes: vec![syntax::ParamMode::Own, syntax::ParamMode::Own],
        return_type: Type::I64,
        value_types: vec![
            Type::SenderI64,
            Type::ReplyToI64,
            Type::Data(DataId(0)),
            Type::Data(DataId(0)),
            Type::Data(DataId(0)),
            Type::I64,
        ],
        place_types: vec![],
        loan_regions: vec![],
        blocks: vec![
            Block {
                params: vec![ValueId(0), ValueId(1)],
                instructions: vec![
                    Instruction {
                        result: Some(ValueId(2)),
                        operation: Operation::Product(DataId(0), vec![ValueId(0), ValueId(1)]),
                        span: Span { start: 0, end: 0 },
                    },
                    Instruction {
                        result: Some(ValueId(3)),
                        operation: Operation::Move(ValueId(2)),
                        span: Span { start: 0, end: 0 },
                    },
                ],
                terminator: Terminator::Jump(BlockId(1), vec![ValueId(3)]),
            },
            Block {
                params: vec![ValueId(4)],
                instructions: vec![Instruction {
                    result: Some(ValueId(5)),
                    operation: Operation::Const(0),
                    span: Span { start: 0, end: 0 },
                }],
                terminator: Terminator::Return(ValueId(5)),
            },
        ],
    };
    let layout = Layout::new(&data);
    let (graph, _) = capability_graph(&function, &layout, &[BlockId(1)], &[]).unwrap();
    let facts = capability_flow::verify(&graph).unwrap();
    assert_eq!(
        facts.failures[0].live,
        vec![capability_flow::Slot(6), capability_flow::Slot(7)]
    );

    let mut duplicate = function;
    duplicate.value_types[1] = Type::SenderI64;
    duplicate.blocks[0].instructions[0].operation =
        Operation::Product(DataId(0), vec![ValueId(0), ValueId(0)]);
    let mut duplicate_data = data;
    duplicate_data[0].fields[1].1 = Type::SenderI64;
    let (graph, _) = capability_graph(
        &duplicate,
        &Layout::new(&duplicate_data),
        &[BlockId(1)],
        &[],
    )
    .unwrap();
    assert!(
        capability_flow::verify(&graph)
            .unwrap_err()
            .contains("moved twice")
    );
}

#[test]
fn whole_product_unpack_transfers_each_right_to_its_field_parameter() {
    let data = vec![DataDef {
        name: "Pair".into(),
        variants: vec![],
        payloads: vec![],
        fields: vec![
            ("sender".into(), Type::SenderI64),
            ("reply".into(), Type::ReplyToI64),
        ],
    }];
    let mut function = Function {
        name: "unpack".into(),
        param_modes: vec![syntax::ParamMode::Own],
        return_type: Type::I64,
        value_types: vec![
            Type::Data(DataId(0)),
            Type::SenderI64,
            Type::ReplyToI64,
            Type::I64,
        ],
        place_types: vec![],
        loan_regions: vec![],
        blocks: vec![
            Block {
                params: vec![ValueId(0)],
                instructions: vec![],
                terminator: Terminator::ProductUnpack {
                    product: ValueId(0),
                    next: BlockId(1),
                },
            },
            Block {
                params: vec![ValueId(1), ValueId(2)],
                instructions: vec![
                    Instruction {
                        result: None,
                        operation: Operation::CloseHandle(ValueId(1)),
                        span: Span { start: 0, end: 0 },
                    },
                    Instruction {
                        result: None,
                        operation: Operation::CloseReply(ValueId(2)),
                        span: Span { start: 0, end: 0 },
                    },
                    Instruction {
                        result: Some(ValueId(3)),
                        operation: Operation::Const(0),
                        span: Span { start: 0, end: 0 },
                    },
                ],
                terminator: Terminator::Return(ValueId(3)),
            },
        ],
    };
    let layout = Layout::new(&data);
    let (graph, _) = capability_graph(&function, &layout, &[], &[]).unwrap();
    capability_flow::verify(&graph).unwrap();

    function.blocks[1].instructions.remove(1);
    let (graph, _) = capability_graph(&function, &layout, &[], &[]).unwrap();
    assert!(
        capability_flow::verify(&graph)
            .unwrap_err()
            .contains("normal return leaks")
    );
}

#[test]
fn structural_close_disposes_product_leaves_in_reverse_field_order() {
    let data = vec![DataDef {
        name: "Pair".into(),
        variants: vec![],
        payloads: vec![],
        fields: vec![
            ("sender".into(), Type::SenderI64),
            ("reply".into(), Type::ReplyToI64),
        ],
    }];
    let function = Function {
        name: "close".into(),
        param_modes: vec![syntax::ParamMode::Own],
        return_type: Type::I64,
        value_types: vec![Type::Data(DataId(0)), Type::I64],
        place_types: vec![],
        loan_regions: vec![],
        blocks: vec![Block {
            params: vec![ValueId(0)],
            instructions: vec![
                Instruction {
                    result: None,
                    operation: Operation::CloseCaps(ValueId(0)),
                    span: Span { start: 0, end: 0 },
                },
                Instruction {
                    result: Some(ValueId(1)),
                    operation: Operation::Const(0),
                    span: Span { start: 0, end: 0 },
                },
            ],
            terminator: Terminator::Return(ValueId(1)),
        }],
    };
    let (graph, _) = capability_graph(&function, &Layout::new(&data), &[], &[]).unwrap();
    assert_eq!(
        graph.blocks[0].actions,
        vec![
            CapAction::Close(capability_flow::Slot(1)),
            CapAction::Close(capability_flow::Slot(0)),
        ]
    );
    capability_flow::verify(&graph).unwrap();
}

#[test]
fn owned_call_and_return_transfer_every_product_leaf() {
    let data = vec![DataDef {
        name: "Pair".into(),
        variants: vec![],
        payloads: vec![],
        fields: vec![
            ("sender".into(), Type::SenderI64),
            ("reply".into(), Type::ReplyToI64),
        ],
    }];
    let pair = Type::Data(DataId(0));
    let span = Span { start: 0, end: 0 };
    let callee = Function {
        name: "identity".into(),
        param_modes: vec![syntax::ParamMode::Own],
        return_type: pair,
        value_types: vec![pair],
        place_types: vec![],
        loan_regions: vec![],
        blocks: vec![Block {
            params: vec![ValueId(0)],
            instructions: vec![],
            terminator: Terminator::Return(ValueId(0)),
        }],
    };
    let layout = Layout::new(&data);
    let (graph, _) = capability_graph(&callee, &layout, &[], &[]).unwrap();
    assert_eq!(
        graph.blocks[0].exits[0].actions,
        vec![
            CapAction::Transfer(capability_flow::Slot(0)),
            CapAction::Transfer(capability_flow::Slot(1)),
        ]
    );
    capability_flow::verify(&graph).unwrap();

    let mut caller = Function {
        name: "caller".into(),
        param_modes: vec![syntax::ParamMode::Own],
        return_type: Type::I64,
        value_types: vec![pair, pair, Type::I64],
        place_types: vec![],
        loan_regions: vec![],
        blocks: vec![Block {
            params: vec![ValueId(0)],
            instructions: vec![
                Instruction {
                    result: Some(ValueId(1)),
                    operation: Operation::Call(FunctionId(0), vec![CallArg::Value(ValueId(0))]),
                    span,
                },
                Instruction {
                    result: None,
                    operation: Operation::CloseCaps(ValueId(1)),
                    span,
                },
                Instruction {
                    result: Some(ValueId(2)),
                    operation: Operation::Const(42),
                    span,
                },
            ],
            terminator: Terminator::Return(ValueId(2)),
        }],
    };
    let (graph, _) = capability_graph(&caller, &layout, &[], &[]).unwrap();
    assert_eq!(
        graph.blocks[0].actions[..4],
        [
            CapAction::Transfer(capability_flow::Slot(0)),
            CapAction::Transfer(capability_flow::Slot(1)),
            CapAction::Create(capability_flow::Slot(2)),
            CapAction::Create(capability_flow::Slot(3)),
        ]
    );
    capability_flow::verify(&graph).unwrap();

    caller.blocks[0].instructions.remove(1);
    let (graph, _) = capability_graph(&caller, &layout, &[], &[]).unwrap();
    assert!(
        capability_flow::verify(&graph)
            .unwrap_err()
            .contains("normal return leaks")
    );
}
