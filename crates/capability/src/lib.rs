//! Exact ownership flow for capability slots.
//! Core IR derives these graphs for channel creation, close, send, receive,
//! and block edges. `Retry` resumes a terminator with its retained frame.

use std::collections::{HashSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Slot(pub usize);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Require(Slot),
    Create(Slot),
    CreatePair {
        first: Slot,
        second: Slot,
    },
    Move {
        from: Slot,
        to: Slot,
    },
    /// Simultaneous SSA/block-argument assignment. A live destination may
    /// also be a source in this group, allowing permutations on backedges.
    MoveGroup(Vec<(Slot, Slot)>),
    /// Replace the selected variant payload's live rights with one affine
    /// sum envelope. An empty source still creates an envelope for a variant
    /// with no rights. The typed Core producer must bind this to the tag.
    PackSum {
        sources: Vec<Slot>,
        envelope: Slot,
    },
    /// Consume a sum envelope and expose the active variant's rights. The
    /// typed Core producer must select destinations from that variant alone.
    UnpackSum {
        envelope: Slot,
        destinations: Vec<Slot>,
    },
    Close(Slot),
    /// Provisional lexical cleanup: close only if this scope still owns it.
    CloseIfLive(Slot),
    /// A product owner has one conditional close for all of its leaves.
    /// Partial ownership is invalid; the first slot identifies the owner
    /// when lowering this provisional action to Core.
    CloseGroupIfLive(Vec<Slot>),
    Transfer(Slot),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Block(usize),
    Suspend(usize),
    Retry,
    Return,
    Failure,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub target: Target,
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub actions: Vec<Action>,
    pub exits: Vec<Edge>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Graph {
    pub slots: usize,
    pub entry_live: Vec<Slot>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureCleanup {
    pub block: usize,
    pub edge: usize,
    pub live: Vec<Slot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suspension {
    pub block: usize,
    pub edge: usize,
    pub resume: usize,
    pub live: Vec<Slot>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlowFacts {
    pub failures: Vec<FailureCleanup>,
    pub suspensions: Vec<Suspension>,
    pub lexical_closes: Vec<(usize, Slot)>,
}

fn live_slots(state: &[bool]) -> Vec<Slot> {
    state
        .iter()
        .enumerate()
        .filter_map(|(index, live)| live.then_some(Slot(index)))
        .collect()
}

fn apply(actions: &[Action], state: &mut [bool], block: usize) -> Result<Vec<Slot>, String> {
    let mut lexical_closes = Vec::new();
    for action in actions {
        let require = |slot: Slot| -> Result<(), String> {
            match state.get(slot.0) {
                Some(true) => Ok(()),
                Some(false) => Err(format!(
                    "block {block}: capability slot {} is not live",
                    slot.0
                )),
                None => Err(format!("block {block}: invalid capability slot {}", slot.0)),
            }
        };
        match action {
            Action::Require(slot) => require(*slot)?,
            Action::Create(slot) => match state.get_mut(slot.0) {
                Some(false) => state[slot.0] = true,
                Some(true) => {
                    return Err(format!(
                        "block {block}: capability slot {} is already live",
                        slot.0
                    ));
                }
                None => return Err(format!("block {block}: invalid capability slot {}", slot.0)),
            },
            Action::CreatePair { first, second } => {
                if first == second {
                    return Err(format!("block {block}: paired creation aliases a slot"));
                }
                for slot in [first, second] {
                    match state.get(slot.0) {
                        Some(false) => {}
                        Some(true) => {
                            return Err(format!(
                                "block {block}: capability slot {} is already live",
                                slot.0
                            ));
                        }
                        None => {
                            return Err(format!(
                                "block {block}: invalid capability slot {}",
                                slot.0
                            ));
                        }
                    }
                }
                state[first.0] = true;
                state[second.0] = true;
            }
            Action::Move { from, to } => {
                require(*from)?;
                if from == to || state.get(to.0) != Some(&false) {
                    return Err(format!(
                        "block {block}: move destination {} is not empty",
                        to.0
                    ));
                }
                state[from.0] = false;
                state[to.0] = true;
            }
            Action::MoveGroup(moves) => {
                for (index, &(from, to)) in moves.iter().enumerate() {
                    require(from)?;
                    if moves[..index].iter().any(|(prior, _)| *prior == from) {
                        return Err(format!(
                            "block {block}: capability slot {} is moved twice",
                            from.0
                        ));
                    }
                    if moves[..index].iter().any(|(_, prior)| *prior == to) {
                        return Err(format!(
                            "block {block}: capability slot {} receives two rights",
                            to.0
                        ));
                    }
                }
                for &(_, to) in moves {
                    if state.get(to.0) != Some(&false) && !moves.iter().any(|(from, _)| *from == to)
                    {
                        return Err(format!(
                            "block {block}: move destination {} is not empty",
                            to.0
                        ));
                    }
                }
                for &(from, _) in moves {
                    state[from.0] = false;
                }
                for &(_, to) in moves {
                    state[to.0] = true;
                }
            }
            Action::PackSum { sources, envelope } => {
                if state.get(envelope.0) != Some(&false) {
                    return Err(format!(
                        "block {block}: sum envelope {} is not empty",
                        envelope.0
                    ));
                }
                let mut seen = HashSet::with_capacity(sources.len());
                for source in sources {
                    require(*source)?;
                    if !seen.insert(*source) {
                        return Err(format!(
                            "block {block}: sum payload slot {} is packed twice",
                            source.0
                        ));
                    }
                }
                for source in sources {
                    state[source.0] = false;
                }
                state[envelope.0] = true;
            }
            Action::UnpackSum {
                envelope,
                destinations,
            } => {
                require(*envelope)?;
                let mut seen = HashSet::with_capacity(destinations.len());
                for destination in destinations {
                    if state.get(destination.0) != Some(&false) {
                        return Err(format!(
                            "block {block}: sum payload destination {} is not empty",
                            destination.0
                        ));
                    }
                    if !seen.insert(*destination) {
                        return Err(format!(
                            "block {block}: sum payload destination {} is repeated",
                            destination.0
                        ));
                    }
                }
                state[envelope.0] = false;
                for destination in destinations {
                    state[destination.0] = true;
                }
            }
            Action::Close(slot) | Action::Transfer(slot) => {
                require(*slot)?;
                state[slot.0] = false;
            }
            Action::CloseIfLive(slot) => match state.get_mut(slot.0) {
                Some(live) if *live => {
                    *live = false;
                    lexical_closes.push(*slot);
                }
                Some(_) => {}
                None => return Err(format!("block {block}: invalid capability slot {}", slot.0)),
            },
            Action::CloseGroupIfLive(slots) => {
                let Some(first) = slots.first() else {
                    return Err(format!("block {block}: empty lexical capability group"));
                };
                let mut live = 0;
                let mut seen = HashSet::with_capacity(slots.len());
                for slot in slots {
                    if !seen.insert(*slot) {
                        return Err(format!(
                            "block {block}: duplicate lexical capability slot {}",
                            slot.0
                        ));
                    }
                    match state.get(slot.0) {
                        Some(true) => live += 1,
                        Some(false) => {}
                        None => {
                            return Err(format!(
                                "block {block}: invalid capability slot {}",
                                slot.0
                            ));
                        }
                    }
                }
                if live != 0 && live != slots.len() {
                    return Err(format!(
                        "block {block}: partial lexical capability ownership"
                    ));
                }
                if live == slots.len() {
                    for slot in slots {
                        state[slot.0] = false;
                    }
                    lexical_closes.push(*first);
                }
            }
        }
    }
    Ok(lexical_closes)
}

pub fn verify(graph: &Graph) -> Result<FlowFacts, String> {
    if graph.blocks.is_empty() {
        return Err("capability graph has no entry block".into());
    }
    let mut entry = vec![false; graph.slots];
    for slot in &graph.entry_live {
        match entry.get_mut(slot.0) {
            Some(false) => entry[slot.0] = true,
            Some(true) => return Err(format!("duplicate entry capability slot {}", slot.0)),
            None => return Err(format!("invalid entry capability slot {}", slot.0)),
        }
    }
    let mut incoming = vec![None; graph.blocks.len()];
    incoming[0] = Some(entry);
    let mut ready = VecDeque::from([0]);
    let mut facts = FlowFacts::default();
    while let Some(block_id) = ready.pop_front() {
        let block = &graph.blocks[block_id];
        if block.exits.is_empty() {
            return Err(format!("block {block_id}: no exit"));
        }
        let mut state = incoming[block_id]
            .clone()
            .expect("reachable block has state");
        facts.lexical_closes.extend(
            apply(&block.actions, &mut state, block_id)?
                .into_iter()
                .map(|slot| (block_id, slot)),
        );
        for (edge_id, edge) in block.exits.iter().enumerate() {
            let mut outgoing = state.clone();
            if !apply(&edge.actions, &mut outgoing, block_id)?.is_empty() {
                return Err(format!(
                    "block {block_id}: lexical close must be a block action"
                ));
            }
            match edge.target {
                Target::Retry => {
                    if outgoing != state {
                        return Err(format!(
                            "block {block_id}: retry edge changes retained capability ownership"
                        ));
                    }
                    facts.suspensions.push(Suspension {
                        block: block_id,
                        edge: edge_id,
                        resume: block_id,
                        live: live_slots(&outgoing),
                    });
                }
                Target::Block(target) | Target::Suspend(target) => {
                    if matches!(edge.target, Target::Suspend(_)) {
                        facts.suspensions.push(Suspension {
                            block: block_id,
                            edge: edge_id,
                            resume: target,
                            live: live_slots(&outgoing),
                        });
                    }
                    let incoming_target = incoming
                        .get_mut(target)
                        .ok_or_else(|| format!("block {block_id}: invalid target {target}"))?;
                    match incoming_target {
                        None => {
                            *incoming_target = Some(outgoing);
                            ready.push_back(target);
                        }
                        Some(prior) if *prior != outgoing => {
                            return Err(format!(
                                "block {target}: capability ownership differs across incoming edges"
                            ));
                        }
                        Some(_) => {}
                    }
                }
                Target::Return if outgoing.iter().any(|live| *live) => {
                    return Err(format!(
                        "block {block_id}: normal return leaks capabilities"
                    ));
                }
                Target::Return => {}
                Target::Failure => facts.failures.push(FailureCleanup {
                    block: block_id,
                    edge: edge_id,
                    live: live_slots(&outgoing),
                }),
            }
        }
    }
    if incoming.iter().any(Option::is_none) {
        return Err("unreachable capability block".into());
    }
    Ok(facts)
}

#[cfg(test)]
mod lexical_group_tests {
    use super::*;

    #[test]
    fn lexical_product_closes_all_or_none() {
        let slots = vec![Slot(0), Slot(1)];
        let mut graph = Graph {
            slots: 2,
            entry_live: slots.clone(),
            blocks: vec![Block {
                actions: vec![
                    Action::CloseGroupIfLive(slots.clone()),
                    Action::CloseGroupIfLive(slots.clone()),
                ],
                exits: vec![Edge {
                    target: Target::Return,
                    actions: vec![],
                }],
            }],
        };
        assert_eq!(verify(&graph).unwrap().lexical_closes, vec![(0, Slot(0))]);

        graph.blocks[0].actions.insert(0, Action::Close(Slot(0)));
        assert!(
            verify(&graph)
                .unwrap_err()
                .contains("partial lexical capability ownership")
        );
    }
}

#[cfg(test)]
mod sum_envelope_tests {
    use super::*;

    fn edge(target: Target, actions: Vec<Action>) -> Edge {
        Edge { target, actions }
    }

    fn optional_right_graph() -> Graph {
        Graph {
            slots: 3,
            entry_live: vec![],
            blocks: vec![
                Block {
                    actions: vec![],
                    exits: vec![
                        edge(Target::Block(1), vec![]),
                        edge(Target::Block(2), vec![]),
                    ],
                },
                Block {
                    actions: vec![
                        Action::Create(Slot(0)),
                        Action::PackSum {
                            sources: vec![Slot(0)],
                            envelope: Slot(1),
                        },
                    ],
                    exits: vec![edge(Target::Block(3), vec![])],
                },
                Block {
                    actions: vec![Action::PackSum {
                        sources: vec![],
                        envelope: Slot(1),
                    }],
                    exits: vec![edge(Target::Block(3), vec![])],
                },
                Block {
                    actions: vec![],
                    exits: vec![
                        edge(
                            Target::Block(4),
                            vec![Action::UnpackSum {
                                envelope: Slot(1),
                                destinations: vec![Slot(2)],
                            }],
                        ),
                        edge(
                            Target::Block(5),
                            vec![Action::UnpackSum {
                                envelope: Slot(1),
                                destinations: vec![],
                            }],
                        ),
                        edge(Target::Failure, vec![]),
                    ],
                },
                Block {
                    actions: vec![Action::Close(Slot(2))],
                    exits: vec![edge(Target::Return, vec![])],
                },
                Block {
                    actions: vec![],
                    exits: vec![edge(Target::Return, vec![])],
                },
            ],
        }
    }

    #[test]
    fn optional_right_has_one_exact_envelope_at_join() {
        let graph = optional_right_graph();
        let facts = verify(&graph).unwrap();
        assert_eq!(facts.failures.len(), 1);
        assert_eq!(facts.failures[0].live, vec![Slot(1)]);

        let mut leaked = graph;
        leaked.blocks[4].actions.clear();
        assert!(verify(&leaked).unwrap_err().contains("normal return leaks"));
    }

    #[test]
    fn sum_pack_and_unpack_reject_aliases() {
        let mut graph = optional_right_graph();
        graph.blocks[1].actions[1] = Action::PackSum {
            sources: vec![Slot(0), Slot(0)],
            envelope: Slot(1),
        };
        assert!(verify(&graph).unwrap_err().contains("packed twice"));

        let mut graph = optional_right_graph();
        graph.blocks[3].exits[0].actions[0] = Action::UnpackSum {
            envelope: Slot(1),
            destinations: vec![Slot(2), Slot(2)],
        };
        assert!(verify(&graph).unwrap_err().contains("repeated"));
    }
}
