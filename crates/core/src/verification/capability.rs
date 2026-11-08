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
    let moves_between = |from: ValueId, to: ValueId| {
        let source = slots[from.0].clone();
        let destination = slots[to.0].clone();
        assert_eq!(
            source.len(),
            destination.len(),
            "typed move has equal leaf layouts"
        );
        source
            .zip(destination)
            .map(|(from, to)| (capability_flow::Slot(from), capability_flow::Slot(to)))
            .collect::<Vec<_>>()
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
    let mut blocks = Vec::with_capacity(function.blocks.len());
    for (block_id, block) in function.blocks.iter().enumerate() {
        let mut actions = Vec::new();
        for instruction in &block.instructions {
            match &instruction.operation {
                Operation::Move(value) => {
                    let moved = instruction.result.expect("verified move has result");
                    let moves = moves_between(*value, moved);
                    if !moves.is_empty() {
                        actions.push(CapAction::MoveGroup(moves));
                    }
                }
                Operation::Product(_, fields) => {
                    let product = instruction.result.expect("verified product has result");
                    let mut destination = slots[product.0].start;
                    let mut moves = Vec::new();
                    for field in fields {
                        for from in slots[field.0].clone() {
                            moves.push((
                                capability_flow::Slot(from),
                                capability_flow::Slot(destination),
                            ));
                            destination += 1;
                        }
                    }
                    assert_eq!(
                        destination, slots[product.0].end,
                        "product leaf layout matches fields"
                    );
                    if !moves.is_empty() {
                        actions.push(CapAction::MoveGroup(moves));
                    }
                }
                Operation::CloseHandle(value)
                | Operation::CloseReply(value)
                | Operation::CloseRequest(value) => actions.push(CapAction::Close(slot(*value))),
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
                        if let CallArg::Value(value) = arg {
                            actions.extend(
                                slots[value.0]
                                    .clone()
                                    .map(|slot| CapAction::Transfer(capability_flow::Slot(slot))),
                            );
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
                    .flat_map(|(arg, param)| moves_between(*arg, *param))
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
            Terminator::ProductUnpack { product, next } => {
                let destinations = function.blocks[next.0]
                    .params
                    .iter()
                    .flat_map(|param| slots[param.0].clone())
                    .collect::<Vec<_>>();
                let source = slots[product.0].clone();
                assert_eq!(
                    source.len(),
                    destinations.len(),
                    "unpack leaf layout matches fields"
                );
                let moves = source
                    .zip(destinations)
                    .map(|(from, to)| (capability_flow::Slot(from), capability_flow::Slot(to)))
                    .collect();
                vec![edge(
                    CapTarget::Block(next.0),
                    vec![CapAction::MoveGroup(moves)],
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
                    .flat_map(|value| {
                        slots[value.0]
                            .clone()
                            .map(|slot| CapAction::Transfer(capability_flow::Slot(slot)))
                    })
                    .collect();
                vec![
                    edge(CapTarget::Block(success.0), transfers),
                    edge(CapTarget::Block(failure.0), vec![]),
                ]
            }
        };
        blocks.push(capability_flow::Block { actions, exits });
    }
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
mod tests;
