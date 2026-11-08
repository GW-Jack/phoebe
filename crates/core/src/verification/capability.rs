//! Exact capability-flow graph derived from checked Core.

use super::capability_layout::Layout;
use super::*;

pub(crate) fn verify_capability_edges(program: &Program) -> Result<(), String> {
    let layout = Layout::new(&program.data);
    for function in &program.functions {
        let Some((graph, _)) = capability_graph(function, &layout, &[], &[]) else {
            continue;
        };
        capability_flow::verify(&graph).map_err(|error| format!("{}: {error}", function.name))?;
    }
    Ok(())
}

pub(crate) fn capability_graph(
    function: &Function,
    layout: &Layout,
    implicit_cleanup_returns: &[BlockId],
    scoped_cleanup_exits: &[(BlockId, ValueId, Span)],
) -> Option<(capability_flow::Graph, Vec<ValueId>)> {
    // Capability flow only needs ownership state for rights, not for every
    // scalar SSA value in the function. Keep this numbering separate from
    // ValueId so adding ordinary computations does not enlarge each state.
    let mut slots = Vec::with_capacity(function.value_types.len());
    let mut values = Vec::new();
    let mut slot_count = 0;
    for (value, ty) in function.value_types.iter().enumerate() {
        let end = slot_count + layout.leaves(*ty);
        slots.push(slot_count..end);
        values.extend(std::iter::repeat_n(ValueId(value), end - slot_count));
        slot_count = end;
    }
    if slot_count == 0 {
        return None;
    }
    let slot = |value: ValueId| {
        let range = &slots[value.0];
        assert!(
            range.end - range.start == 1 && Layout::bare_right(function.value_types[value.0]),
            "operation requires a bare communication right"
        );
        capability_flow::Slot(range.start)
    };
    let mut lexical = vec![None; function.blocks.len()];
    for (block, value, _) in scoped_cleanup_exits {
        lexical[block.0] = Some(slot(*value));
    }
    let mut cleanup_return = vec![false; function.blocks.len()];
    for block in implicit_cleanup_returns {
        cleanup_return[block.0] = true;
    }
    let entry_live = function.blocks[0]
        .params
        .iter()
        .flat_map(|value| slots[value.0].clone().map(capability_flow::Slot))
        .collect();
    let blocks = function
        .blocks
        .iter()
        .enumerate()
        .map(|(block_id, block)| {
            let mut actions = Vec::new();
            for instruction in &block.instructions {
                match &instruction.operation {
                    Operation::CloseHandle(value)
                    | Operation::CloseReply(value)
                    | Operation::CloseRequest(value) => {
                        actions.push(CapAction::Close(slot(*value)))
                    }
                    Operation::PackRequest { right, .. } => {
                        let packed = instruction
                            .result
                            .expect("verified request pack has result");
                        actions.push(CapAction::Move {
                            from: slot(*right),
                            to: slot(packed),
                        });
                    }
                    Operation::Call(_, args) => {
                        for arg in args {
                            if let CallArg::Value(value) = arg
                                && Layout::bare_right(function.value_types[value.0])
                            {
                                actions.push(CapAction::Transfer(slot(*value)));
                            }
                        }
                    }
                    _ => {}
                }
            }
            if let Some(slot) = lexical[block_id] {
                actions.push(CapAction::CloseIfLive(slot));
            }
            let edge = |target: CapTarget, actions| capability_flow::Edge { target, actions };
            let exits = match &block.terminator {
                Terminator::Yield { resume, .. } | Terminator::ScopeJoin { resume, .. } => {
                    vec![edge(CapTarget::Suspend(resume.0), vec![])]
                }
                Terminator::Return(_) => vec![edge(
                    if cleanup_return[block_id] {
                        CapTarget::Failure
                    } else {
                        CapTarget::Return
                    },
                    vec![],
                )],
                Terminator::Jump(target, args) => {
                    let moves = args
                        .iter()
                        .zip(&function.blocks[target.0].params)
                        .filter(|(arg, _)| Layout::bare_right(function.value_types[arg.0]))
                        .map(|(arg, param)| (slot(*arg), slot(*param)))
                        .collect::<Vec<_>>();
                    vec![edge(
                        CapTarget::Block(target.0),
                        vec![CapAction::MoveGroup(moves)],
                    )]
                }
                Terminator::Branch {
                    then_block,
                    else_block,
                    ..
                } => vec![
                    edge(CapTarget::Block(then_block.0), vec![]),
                    edge(CapTarget::Block(else_block.0), vec![]),
                ],
                Terminator::Match { arms, .. } => arms
                    .iter()
                    .map(|arm| edge(CapTarget::Block(arm.target.0), vec![]))
                    .collect(),
                Terminator::ChannelNew {
                    success, failure, ..
                } => {
                    let params = &function.blocks[success.0].params;
                    vec![
                        edge(
                            CapTarget::Block(success.0),
                            vec![CapAction::CreatePair {
                                first: slot(params[0]),
                                second: slot(params[1]),
                            }],
                        ),
                        edge(CapTarget::Block(failure.0), vec![]),
                    ]
                }
                Terminator::ReplyNew { success, failure } => {
                    let params = &function.blocks[success.0].params;
                    vec![
                        edge(
                            CapTarget::Block(success.0),
                            vec![CapAction::CreatePair {
                                first: slot(params[0]),
                                second: slot(params[1]),
                            }],
                        ),
                        edge(CapTarget::Block(failure.0), vec![]),
                    ]
                }
                Terminator::RequestChannelNew {
                    success, failure, ..
                } => {
                    let params = &function.blocks[success.0].params;
                    vec![
                        edge(
                            CapTarget::Block(success.0),
                            vec![CapAction::CreatePair {
                                first: slot(params[0]),
                                second: slot(params[1]),
                            }],
                        ),
                        edge(CapTarget::Block(failure.0), vec![]),
                    ]
                }
                Terminator::RequestUnpack { request, next } => {
                    let params = &function.blocks[next.0].params;
                    vec![edge(
                        CapTarget::Block(next.0),
                        vec![CapAction::Move {
                            from: slot(*request),
                            to: slot(params[1]),
                        }],
                    )]
                }
                Terminator::RequestSend {
                    sender,
                    request,
                    sent,
                    closed,
                } => {
                    let returned = function.blocks[closed.0].params[0];
                    vec![
                        edge(
                            CapTarget::Block(sent.0),
                            vec![
                                CapAction::Require(slot(*sender)),
                                CapAction::Transfer(slot(*request)),
                            ],
                        ),
                        edge(
                            CapTarget::Block(closed.0),
                            vec![
                                CapAction::Require(slot(*sender)),
                                CapAction::Move {
                                    from: slot(*request),
                                    to: slot(returned),
                                },
                            ],
                        ),
                        edge(
                            CapTarget::Retry,
                            vec![
                                CapAction::Require(slot(*sender)),
                                CapAction::Require(slot(*request)),
                            ],
                        ),
                    ]
                }
                Terminator::RequestReceive {
                    receiver,
                    value,
                    end,
                } => {
                    let received = function.blocks[value.0].params[0];
                    vec![
                        edge(
                            CapTarget::Block(value.0),
                            vec![
                                CapAction::Require(slot(*receiver)),
                                CapAction::Create(slot(received)),
                            ],
                        ),
                        edge(
                            CapTarget::Block(end.0),
                            vec![CapAction::Require(slot(*receiver))],
                        ),
                        edge(CapTarget::Retry, vec![CapAction::Require(slot(*receiver))]),
                    ]
                }
                Terminator::Fork {
                    source,
                    success,
                    failure,
                } => {
                    let fresh = function.blocks[success.0].params[0];
                    vec![
                        edge(
                            CapTarget::Block(success.0),
                            vec![
                                CapAction::Require(slot(*source)),
                                CapAction::Create(slot(fresh)),
                            ],
                        ),
                        edge(
                            CapTarget::Block(failure.0),
                            vec![CapAction::Require(slot(*source))],
                        ),
                    ]
                }
                Terminator::ChannelSend {
                    sender,
                    sent,
                    closed,
                    ..
                } => {
                    let require = vec![CapAction::Require(slot(*sender))];
                    vec![
                        edge(CapTarget::Block(sent.0), require.clone()),
                        edge(CapTarget::Block(closed.0), require.clone()),
                        edge(CapTarget::Retry, require),
                    ]
                }
                Terminator::ChannelReceive {
                    receiver,
                    value,
                    end,
                } => {
                    let require = vec![CapAction::Require(slot(*receiver))];
                    vec![
                        edge(CapTarget::Block(value.0), require.clone()),
                        edge(CapTarget::Block(end.0), require.clone()),
                        edge(CapTarget::Retry, require),
                    ]
                }
                Terminator::ReplyComplete {
                    right,
                    delivered,
                    closed,
                    ..
                } => {
                    let close = vec![CapAction::Close(slot(*right))];
                    vec![
                        edge(CapTarget::Block(delivered.0), close.clone()),
                        edge(CapTarget::Block(closed.0), close),
                    ]
                }
                Terminator::ReplyReceive {
                    receiver,
                    value,
                    end,
                } => {
                    let close = vec![CapAction::Close(slot(*receiver))];
                    vec![
                        edge(CapTarget::Block(value.0), close.clone()),
                        edge(CapTarget::Block(end.0), close),
                        edge(CapTarget::Retry, vec![CapAction::Require(slot(*receiver))]),
                    ]
                }
                Terminator::Spawn {
                    args,
                    success,
                    failure,
                    ..
                } => {
                    let transfers = args
                        .iter()
                        .filter(|value| Layout::bare_right(function.value_types[value.0]))
                        .map(|value| CapAction::Transfer(slot(*value)))
                        .collect();
                    vec![
                        edge(CapTarget::Block(success.0), transfers),
                        edge(CapTarget::Block(failure.0), vec![]),
                    ]
                }
            };
            capability_flow::Block { actions, exits }
        })
        .collect();
    Some((
        capability_flow::Graph {
            slots: slot_count,
            entry_live,
            blocks,
        },
        values,
    ))
}

#[cfg(test)]
mod tests {
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
}
