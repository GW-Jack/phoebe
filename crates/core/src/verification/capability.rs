//! Exact capability-flow graph derived from checked Core.

use super::capability_layout::Layout;
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SlotOwner {
    Value(ValueId),
    Place(PlaceId),
}

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
) -> Option<(capability_flow::Graph, Vec<SlotOwner>)> {
    // Capability flow only needs ownership state for rights, not for every
    // scalar SSA value in the function. Keep this numbering separate from
    // ValueId so adding ordinary computations does not enlarge each state.
    let mut slots = Vec::with_capacity(function.value_types.len());
    let mut owners = Vec::new();
    let mut slot_count = 0;
    for (value, ty) in function.value_types.iter().enumerate() {
        let end = slot_count + layout.leaves(*ty);
        slots.push(slot_count..end);
        owners.extend(std::iter::repeat_n(
            SlotOwner::Value(ValueId(value)),
            end - slot_count,
        ));
        slot_count = end;
    }
    let mut places = Vec::with_capacity(function.place_types.len());
    for (place, ty) in function.place_types.iter().enumerate() {
        let end = slot_count + layout.leaves(*ty);
        places.push(slot_count..end);
        owners.extend(std::iter::repeat_n(
            SlotOwner::Place(PlaceId(place)),
            end - slot_count,
        ));
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
    let move_ranges = |source: std::ops::Range<usize>, destination: std::ops::Range<usize>| {
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
    let moves_between =
        |from: ValueId, to: ValueId| move_ranges(slots[from.0].clone(), slots[to.0].clone());
    let mut lexical = vec![None; function.blocks.len()];
    for (block, value, _) in scoped_cleanup_exits {
        lexical[block.0] = Some(slots[value.0].clone());
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
                Operation::PlaceInit(place, value) | Operation::Store(place, value) => {
                    let moves = move_ranges(slots[value.0].clone(), places[place.0].clone());
                    if !moves.is_empty() {
                        actions.push(CapAction::MoveGroup(moves));
                    }
                }
                Operation::DestField(place, field, value) => {
                    let Type::Data(id) = function.place_types[place.0] else {
                        unreachable!("verified destination")
                    };
                    let leaves = layout.field_leaves(id, *field);
                    let destination = (places[place.0].start + leaves.start)
                        ..(places[place.0].start + leaves.end);
                    let moves = move_ranges(slots[value.0].clone(), destination);
                    if !moves.is_empty() {
                        actions.push(CapAction::MoveGroup(moves));
                    }
                }
                Operation::DestPublish(place) => {
                    let published = instruction.result.expect("verified destination result");
                    let moves = move_ranges(places[place.0].clone(), slots[published.0].clone());
                    if !moves.is_empty() {
                        actions.push(CapAction::MoveGroup(moves));
                    }
                }
                Operation::DestTakeField(place, field) => {
                    let Type::Data(id) = function.place_types[place.0] else {
                        unreachable!("verified destination")
                    };
                    let leaves = layout.field_leaves(id, *field);
                    let source = (places[place.0].start + leaves.start)
                        ..(places[place.0].start + leaves.end);
                    let taken = instruction
                        .result
                        .expect("verified destination field result");
                    let moves = move_ranges(source, slots[taken.0].clone());
                    if !moves.is_empty() {
                        actions.push(CapAction::MoveGroup(moves));
                    }
                }
                Operation::TakeField(place, field) => {
                    let Type::Data(id) = function.place_types[place.0] else {
                        unreachable!("verified product place")
                    };
                    let leaves = layout.field_leaves(id, *field);
                    let source = (places[place.0].start + leaves.start)
                        ..(places[place.0].start + leaves.end);
                    let taken = instruction.result.expect("verified field take result");
                    let moves = move_ranges(source, slots[taken.0].clone());
                    actions.push(CapAction::MoveGroup(moves));
                }
                Operation::StoreField(place, field, value) => {
                    let Type::Data(id) = function.place_types[place.0] else {
                        unreachable!("verified product place")
                    };
                    let leaves = layout.field_leaves(id, *field);
                    let destination = (places[place.0].start + leaves.start)
                        ..(places[place.0].start + leaves.end);
                    let moves = move_ranges(slots[value.0].clone(), destination);
                    if !moves.is_empty() {
                        actions.push(CapAction::MoveGroup(moves));
                    }
                }
                Operation::Take(place) => {
                    let taken = instruction.result.expect("verified take has result");
                    let moves = move_ranges(places[place.0].clone(), slots[taken.0].clone());
                    if !moves.is_empty() {
                        actions.push(CapAction::MoveGroup(moves));
                    }
                }
                Operation::ReadField(_, _) => {}
                Operation::Variant(_, _, payload) => {
                    let sum = instruction.result.expect("verified variant has result");
                    if let Some(envelope) = slots[sum.0].clone().next() {
                        actions.push(CapAction::PackSum {
                            sources: payload
                                .iter()
                                .flat_map(|value| slots[value.0].clone().map(capability_flow::Slot))
                                .collect(),
                            envelope: capability_flow::Slot(envelope),
                        });
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
                Operation::CloseCaps(value) => {
                    actions.extend(
                        slots[value.0]
                            .clone()
                            .rev()
                            .map(|slot| CapAction::Close(capability_flow::Slot(slot))),
                    );
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
                        if let CallArg::Value(value) = arg {
                            actions.extend(
                                slots[value.0]
                                    .clone()
                                    .map(|slot| CapAction::Transfer(capability_flow::Slot(slot))),
                            );
                        }
                    }
                    if let Some(result) = instruction.result {
                        actions.extend(
                            slots[result.0]
                                .clone()
                                .map(|slot| CapAction::Create(capability_flow::Slot(slot))),
                        );
                    }
                }
                Operation::GenericCall { args, .. } => {
                    for value in args {
                        actions.extend(
                            slots[value.0]
                                .clone()
                                .map(|slot| CapAction::Transfer(capability_flow::Slot(slot))),
                        );
                    }
                    if let Some(result) = instruction.result {
                        actions.extend(
                            slots[result.0]
                                .clone()
                                .map(|slot| CapAction::Create(capability_flow::Slot(slot))),
                        );
                    }
                }
                _ => {}
            }
        }
        if let Some(range) = &lexical[block_id] {
            let leaves = range.clone().map(capability_flow::Slot).collect::<Vec<_>>();
            if leaves.len() == 1 {
                actions.push(CapAction::CloseIfLive(leaves[0]));
            } else {
                actions.push(CapAction::CloseGroupIfLive(leaves));
            }
        }
        let edge = |target: CapTarget, actions| capability_flow::Edge { target, actions };
        let exits = match &block.terminator {
            Terminator::Yield { resume, .. } | Terminator::ScopeJoin { resume, .. } => {
                vec![edge(CapTarget::Suspend(resume.0), vec![])]
            }
            Terminator::Return(value) => vec![edge(
                if cleanup_return[block_id] {
                    CapTarget::Failure
                } else {
                    CapTarget::Return
                },
                slots[value.0]
                    .clone()
                    .map(|slot| CapAction::Transfer(capability_flow::Slot(slot)))
                    .collect(),
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
            Terminator::Match { subject, arms } => arms
                .iter()
                .map(|arm| {
                    let actions = if let Some(envelope) = slots[subject.0].clone().next() {
                        let target = &function.blocks[arm.target.0];
                        match arm.binding {
                            MatchBinding::Whole => {
                                vec![CapAction::Move {
                                    from: capability_flow::Slot(envelope),
                                    to: capability_flow::Slot(slots[target.params[0].0].start),
                                }]
                            }
                            MatchBinding::Payload | MatchBinding::Discard => {
                                let destinations = if arm.binding == MatchBinding::Payload {
                                    slots[target.params[0].0]
                                        .clone()
                                        .map(capability_flow::Slot)
                                        .collect()
                                } else {
                                    vec![]
                                };
                                vec![CapAction::UnpackSum {
                                    envelope: capability_flow::Slot(envelope),
                                    destinations,
                                }]
                            }
                        }
                    } else {
                        vec![]
                    };
                    edge(CapTarget::Block(arm.target.0), actions)
                })
                .collect(),
            Terminator::ChannelNew {
                success, failure, ..
            }
            | Terminator::TypedChannelNew {
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
            Terminator::TypedSend {
                sender,
                message,
                sent,
                closed,
            } => {
                let returned = function.blocks[closed.0].params[0];
                let message_rights = slots[message.0]
                    .clone()
                    .map(capability_flow::Slot)
                    .collect::<Vec<_>>();
                let mut sent_actions = vec![CapAction::Require(slot(*sender))];
                sent_actions.extend(message_rights.iter().copied().map(CapAction::Transfer));
                let mut retry_actions = vec![CapAction::Require(slot(*sender))];
                retry_actions.extend(message_rights.iter().copied().map(CapAction::Require));
                let mut closed_actions = vec![CapAction::Require(slot(*sender))];
                let moves = moves_between(*message, returned);
                if !moves.is_empty() {
                    closed_actions.push(CapAction::MoveGroup(moves));
                }
                vec![
                    edge(CapTarget::Block(sent.0), sent_actions),
                    edge(CapTarget::Block(closed.0), closed_actions),
                    edge(CapTarget::Retry, retry_actions),
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
            Terminator::TypedReceive {
                receiver,
                value,
                end,
            } => {
                let received = function.blocks[value.0].params[0];
                let mut value_actions = vec![CapAction::Require(slot(*receiver))];
                value_actions.extend(
                    slots[received.0]
                        .clone()
                        .map(|slot| CapAction::Create(capability_flow::Slot(slot))),
                );
                let require = vec![CapAction::Require(slot(*receiver))];
                vec![
                    edge(CapTarget::Block(value.0), value_actions),
                    edge(CapTarget::Block(end.0), require.clone()),
                    edge(CapTarget::Retry, require),
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
        owners,
    ))
}

#[cfg(test)]
mod tests;
