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
