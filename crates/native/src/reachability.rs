//! Function graph selection for native entry points.

use phoebe_core::{FunctionId, Operation, Program, Terminator};
use phoebe_frame as frame_layout;

pub(super) fn spawned_poll_children(program: &Program) -> Result<Vec<FunctionId>, String> {
    let all = vec![true; program.functions.len()];
    spawned_poll_children_in(program, &all)
}

pub(super) fn spawned_poll_children_in(
    program: &Program,
    parents: &[bool],
) -> Result<Vec<FunctionId>, String> {
    let suspendable = frame_layout::suspendable_functions(program)?;
    Ok(spawned_poll_children_with_suspendable(
        program,
        parents,
        &suspendable,
    ))
}

pub(super) fn spawned_poll_children_with_suspendable(
    program: &Program,
    parents: &[bool],
    suspendable: &[bool],
) -> Vec<FunctionId> {
    let mut children = vec![false; program.functions.len()];
    for (index, function) in program.functions.iter().enumerate() {
        if !parents[index] {
            continue;
        }
        for block in &function.blocks {
            if let Terminator::Spawn { child, .. } = block.terminator
                && suspendable[child.0]
            {
                children[child.0] = true;
            }
        }
    }
    children
        .iter()
        .enumerate()
        .filter_map(|(index, found)| found.then_some(FunctionId(index)))
        .collect()
}

pub(super) fn standalone_direct_functions(
    program: &Program,
    root: FunctionId,
    suspendable: &[bool],
) -> Vec<bool> {
    let mut selected = vec![false; program.functions.len()];
    let mut ready = vec![root];
    while let Some(id) = ready.pop() {
        if suspendable[id.0] || selected[id.0] {
            continue;
        }
        selected[id.0] = true;
        for block in &program.functions[id.0].blocks {
            for instruction in &block.instructions {
                if let Operation::Call(target, _) = instruction.operation {
                    ready.push(target);
                }
            }
            if let Terminator::Spawn { child, .. } = block.terminator {
                ready.push(child);
            }
        }
    }
    selected
}

/// Closure of one retained entry across retained calls/spawns and direct
/// helpers. A direct helper may spawn a retained child, which can in turn
/// reach more direct helpers.
pub(super) struct PollGraph {
    pub retained: Vec<FunctionId>,
    pub direct: Vec<bool>,
    pub spawned_from_direct: Vec<bool>,
}

pub(super) fn poll_graph(
    program: &Program,
    root: FunctionId,
    suspendable: &[bool],
) -> Result<PollGraph, String> {
    let mut retained = Vec::new();
    let mut seen = vec![false; program.functions.len()];
    let mut direct = vec![false; program.functions.len()];
    let mut spawned_from_direct = vec![false; program.functions.len()];
    let mut pending_poll = vec![root];
    let mut pending_direct = Vec::new();
    while !pending_poll.is_empty() || !pending_direct.is_empty() {
        if let Some(id) = pending_poll.pop() {
            if seen[id.0] {
                continue;
            }
            seen[id.0] = true;
            retained.push(id);
            for block in &program.functions[id.0].blocks {
                for instruction in &block.instructions {
                    if let Operation::Call(child, _) = instruction.operation {
                        if suspendable[child.0] {
                            pending_poll.push(child);
                        } else {
                            pending_direct.push(child);
                        }
                    }
                }
                if let Terminator::Spawn { child, .. } = block.terminator {
                    if suspendable[child.0] {
                        pending_poll.push(child);
                    } else {
                        pending_direct.push(child);
                    }
                }
            }
        } else if let Some(id) = pending_direct.pop() {
            if direct[id.0] {
                continue;
            }
            if suspendable[id.0] {
                return Err("direct call graph contains a suspendable function".into());
            }
            direct[id.0] = true;
            for block in &program.functions[id.0].blocks {
                for instruction in &block.instructions {
                    if let Operation::Call(child, _) = instruction.operation {
                        pending_direct.push(child);
                    }
                }
                if let Terminator::Spawn { child, .. } = block.terminator {
                    if suspendable[child.0] {
                        spawned_from_direct[child.0] = true;
                        pending_poll.push(child);
                    } else {
                        pending_direct.push(child);
                    }
                }
            }
        }
    }
    retained.sort_by_key(|id| id.0);
    Ok(PollGraph {
        retained,
        direct,
        spawned_from_direct,
    })
}
