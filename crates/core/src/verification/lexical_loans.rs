//! Exact active-loan flow for explicit Core borrow.begin / borrow.end pairs.
//! Call-scoped loans remain atomic Call operations and need no active state.

use super::*;

fn successors(terminator: &Terminator) -> Vec<BlockId> {
    match terminator {
        Terminator::Return(_) => vec![],
        Terminator::Yield { resume, .. } | Terminator::ScopeJoin { resume, .. } => vec![*resume],
        Terminator::Jump(target, _)
        | Terminator::RequestUnpack { next: target, .. }
        | Terminator::ProductUnpack { next: target, .. } => vec![*target],
        Terminator::Branch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Match { arms, .. } => arms.iter().map(|arm| arm.target).collect(),
        Terminator::ChannelNew {
            success, failure, ..
        }
        | Terminator::ReplyNew { success, failure }
        | Terminator::RequestChannelNew {
            success, failure, ..
        }
        | Terminator::Fork {
            success, failure, ..
        }
        | Terminator::Spawn {
            success, failure, ..
        } => vec![*success, *failure],
        Terminator::RequestSend { sent, closed, .. }
        | Terminator::ChannelSend { sent, closed, .. } => vec![*sent, *closed],
        Terminator::RequestReceive { value, end, .. }
        | Terminator::ChannelReceive { value, end, .. }
        | Terminator::ReplyReceive { value, end, .. } => vec![*value, *end],
        Terminator::ReplyComplete {
            delivered, closed, ..
        } => vec![*delivered, *closed],
    }
}

#[derive(Clone, Copy)]
struct Loan {
    origin: LoanOrigin,
    exclusive: bool,
}

#[derive(Clone, Copy)]
enum LoanOrigin {
    Root(PlaceId),
    Parent(ValueId),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StorageRoot {
    Place(PlaceId),
    External(ValueId),
}

#[derive(Clone, PartialEq, Eq)]
struct LoanState {
    active: Vec<bool>,
    frozen: Vec<bool>,
}

fn external_root(function: &Function, value: ValueId) -> Option<StorageRoot> {
    (function.blocks.first()?.params.contains(&value)
        && function.value_types.get(value.0)?.referent().is_some())
    .then_some(StorageRoot::External(value))
}

fn roots(function: &Function, loans: &[Option<Loan>]) -> Option<Vec<Option<StorageRoot>>> {
    let mut roots = vec![None; loans.len()];
    for start in 0..loans.len() {
        if loans[start].is_none() || roots[start].is_some() {
            continue;
        }
        let mut path = Vec::new();
        let mut current = start;
        let root = loop {
            if let Some(root) = roots.get(current).copied().flatten() {
                break root;
            }
            let Some(loan) = loans.get(current)?.as_ref() else {
                break external_root(function, ValueId(current))?;
            };
            path.push(current);
            if path.len() > loans.len() {
                return None;
            }
            match loan.origin {
                LoanOrigin::Root(place) => break StorageRoot::Place(place),
                LoanOrigin::Parent(parent) => current = parent.0,
            }
        };
        for id in path {
            roots[id] = Some(root);
        }
    }
    Some(roots)
}

fn loan_definitions(function: &Function) -> Result<Vec<Option<Loan>>, String> {
    let mut loans = vec![None; function.value_types.len()];
    for block in &function.blocks {
        for instruction in &block.instructions {
            let loan = match instruction.operation {
                Operation::BorrowBegin(place, exclusive) => Some(Loan {
                    origin: LoanOrigin::Root(place),
                    exclusive,
                }),
                Operation::BorrowReborrow(parent, exclusive) => Some(Loan {
                    origin: LoanOrigin::Parent(parent),
                    exclusive,
                }),
                _ => None,
            };
            if let Some(loan) = loan {
                let result = instruction
                    .result
                    .ok_or("borrow operation lacks a result")?;
                let slot = loans.get_mut(result.0).ok_or("invalid lexical result")?;
                *slot = Some(loan);
            }
        }
    }
    Ok(loans)
}

/// Resolve lexical references rooted in task-local places. Reborrows of
/// incoming reference parameters return `None`: their caller owns the root.
pub fn lexical_roots(function: &Function) -> Result<Vec<Option<PlaceId>>, String> {
    let loans = loan_definitions(function)?;
    roots(function, &loans)
        .ok_or_else(|| format!("{}: reborrow has no lexical root", function.name))
        .map(|roots| {
            roots
                .into_iter()
                .map(|root| match root {
                    Some(StorageRoot::Place(place)) => Some(place),
                    _ => None,
                })
                .collect()
        })
}

fn conflict(
    active: &[bool],
    loans: &[Option<Loan>],
    roots: &[Option<StorageRoot>],
    root_place: PlaceId,
    exclusive: bool,
) -> bool {
    active.iter().enumerate().any(|(id, live)| {
        *live
            && loans[id].is_some_and(|loan| {
                roots[id] == Some(StorageRoot::Place(root_place)) && (exclusive || loan.exclusive)
            })
    })
}

pub(super) fn verify_lexical_loans(program: &Program) -> Result<(), String> {
    for function in &program.functions {
        if !function.blocks.iter().any(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(
                    instruction.operation,
                    Operation::BorrowBegin(_, _)
                        | Operation::BorrowReborrow(_, _)
                        | Operation::BorrowEnd(_)
                )
            })
        }) {
            continue;
        }
        let loans = loan_definitions(function)?;
        let roots = roots(function, &loans)
            .ok_or_else(|| format!("{}: reborrow has no lexical root", function.name))?;
        let mut incoming = vec![None; function.blocks.len()];
        incoming[0] = Some(LoanState {
            active: vec![false; loans.len()],
            frozen: vec![false; loans.len()],
        });
        let mut ready = VecDeque::from([0usize]);
        while let Some(block_id) = ready.pop_front() {
            let block = &function.blocks[block_id];
            let mut state = incoming[block_id]
                .clone()
                .expect("queued block has loan state");
            for instruction in &block.instructions {
                for operand in operands(&instruction.operation) {
                    if loans[operand.0].is_some() && !state.active[operand.0] {
                        return Err(format!("{}: use after borrow end", function.name));
                    }
                }
                match &instruction.operation {
                    Operation::BorrowBegin(place, exclusive) => {
                        if conflict(&state.active, &loans, &roots, *place, *exclusive) {
                            return Err(format!("{}: overlapping lexical borrows", function.name));
                        }
                        state.active[instruction.result.expect("typed borrow begin").0] = true;
                    }
                    Operation::BorrowReborrow(parent, _) => {
                        if loans[parent.0].is_none() && external_root(function, *parent).is_none() {
                            return Err(format!(
                                "{}: reborrow requires a lexical or incoming borrowed parent",
                                function.name
                            ));
                        }
                        if state.frozen[parent.0] {
                            return Err(format!(
                                "{}: frozen parent cannot be reborrowed",
                                function.name
                            ));
                        }
                        if matches!(
                            function.value_types[parent.0],
                            Type::RefMutI64 | Type::RefMutBool
                        ) {
                            state.frozen[parent.0] = true;
                        }
                        state.active[instruction.result.expect("typed reborrow").0] = true;
                    }
                    Operation::BorrowEnd(reference) => {
                        if loans[reference.0].is_none() {
                            return Err(format!(
                                "{}: borrow end requires a lexical loan",
                                function.name
                            ));
                        }
                        if loans.iter().enumerate().any(|(id, loan)| {
                            state.active[id]
                                && loan.is_some_and(|loan| {
                                    matches!(loan.origin, LoanOrigin::Parent(parent) if parent == *reference)
                                })
                        }) {
                            return Err(format!("{}: parent loan ends before child", function.name));
                        }
                        state.active[reference.0] = false;
                        if let LoanOrigin::Parent(parent) =
                            loans[reference.0].expect("checked loan").origin
                        {
                            state.frozen[parent.0] = false;
                        }
                    }
                    Operation::LoadRef(reference) | Operation::StoreRef(reference, _)
                        if state.frozen[reference.0] =>
                    {
                        return Err(format!("{}: frozen parent cannot be used", function.name));
                    }
                    Operation::Load(place)
                    | Operation::ReadField(place, _)
                    | Operation::CopyPlace(place)
                        if conflict(&state.active, &loans, &roots, *place, false) =>
                    {
                        return Err(format!(
                            "{}: read conflicts with exclusive loan",
                            function.name
                        ));
                    }
                    Operation::PlaceInit(place, _)
                    | Operation::Store(place, _)
                    | Operation::Take(place)
                        if conflict(&state.active, &loans, &roots, *place, true) =>
                    {
                        return Err(format!(
                            "{}: mutation conflicts with lexical loan",
                            function.name
                        ));
                    }
                    Operation::Call(_, args) => {
                        let mut call_roots = Vec::new();
                        for arg in args {
                            let root = match arg {
                                CallArg::Borrow(place, exclusive) => {
                                    if conflict(&state.active, &loans, &roots, *place, *exclusive) {
                                        return Err(format!(
                                            "{}: call borrow conflicts with lexical loan",
                                            function.name
                                        ));
                                    }
                                    Some((StorageRoot::Place(*place), *exclusive))
                                }
                                CallArg::Ref(reference, exclusive) => {
                                    if state.frozen[reference.0] {
                                        return Err(format!(
                                            "{}: frozen parent cannot be forwarded to a call",
                                            function.name
                                        ));
                                    }
                                    roots[reference.0]
                                        .or_else(|| external_root(function, *reference))
                                        .map(|root| (root, *exclusive))
                                }
                                _ => None,
                            };
                            if let Some((place, exclusive)) = root {
                                if call_roots.iter().any(|(other, other_exclusive)| {
                                    *other == place && (exclusive || *other_exclusive)
                                }) {
                                    return Err(format!(
                                        "{}: call arguments overlap an exclusive lexical root",
                                        function.name
                                    ));
                                }
                                call_roots.push((place, exclusive));
                            }
                        }
                    }
                    Operation::GenericCall { .. } if state.active.iter().any(|live| *live) => {
                        return Err(format!(
                            "{}: generic call inside lexical loan is not yet supported",
                            function.name
                        ));
                    }
                    _ => {}
                }
            }
            if state.active.iter().any(|live| *live)
                && matches!(
                    block.terminator,
                    Terminator::Return(_)
                        | Terminator::ScopeJoin { .. }
                        | Terminator::RequestSend { .. }
                        | Terminator::RequestReceive { .. }
                        | Terminator::ReplyReceive { .. }
                        | Terminator::Spawn { .. }
                )
            {
                return Err(format!(
                    "{}: lexical loan crosses exit or suspension",
                    function.name
                ));
            }
            for target in successors(&block.terminator) {
                match &incoming[target.0] {
                    None => {
                        incoming[target.0] = Some(state.clone());
                        ready.push_back(target.0);
                    }
                    Some(existing) if existing != &state => {
                        return Err(format!(
                            "{}: lexical loan states differ at a CFG join",
                            function.name
                        ));
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
