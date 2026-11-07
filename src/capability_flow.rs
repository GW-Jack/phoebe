//! Exact ownership flow for capability slots.
//! Core IR derives these graphs for channel creation, close, and block edges.

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Slot(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Require(Slot),
    Create(Slot),
    CreatePair { first: Slot, second: Slot },
    Move { from: Slot, to: Slot },
    Close(Slot),
    Transfer(Slot),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Block(usize),
    Suspend(usize),
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
}

fn live_slots(state: &[bool]) -> Vec<Slot> {
    state
        .iter()
        .enumerate()
        .filter_map(|(index, live)| live.then_some(Slot(index)))
        .collect()
}

fn apply(actions: &[Action], state: &mut [bool], block: usize) -> Result<(), String> {
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
        match *action {
            Action::Require(slot) => require(slot)?,
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
                require(from)?;
                if from == to || state.get(to.0) != Some(&false) {
                    return Err(format!(
                        "block {block}: move destination {} is not empty",
                        to.0
                    ));
                }
                state[from.0] = false;
                state[to.0] = true;
            }
            Action::Close(slot) | Action::Transfer(slot) => {
                require(slot)?;
                state[slot.0] = false;
            }
        }
    }
    Ok(())
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
        apply(&block.actions, &mut state, block_id)?;
        for (edge_id, edge) in block.exits.iter().enumerate() {
            let mut outgoing = state.clone();
            apply(&edge.actions, &mut outgoing, block_id)?;
            match edge.target {
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
