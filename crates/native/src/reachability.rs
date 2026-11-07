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
    Ok(children
        .iter()
        .enumerate()
        .filter_map(|(index, found)| found.then_some(FunctionId(index)))
        .collect())
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
