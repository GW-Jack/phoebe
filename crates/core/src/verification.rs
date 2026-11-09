use super::*;

mod capability;
pub(crate) mod capability_layout;
mod dominators;
mod lexical_loans;
mod operations;
mod structure;

pub(crate) use capability::{SlotOwner, capability_graph, verify_capability_edges};
pub use lexical_loans::lexical_roots;
use operations::op_type;
pub use operations::operands;
pub(crate) use structure::{recursive_call_span, verify_structure};

#[derive(Clone, PartialEq, Eq)]
struct OwnershipState {
    values: Vec<bool>,
    places: Vec<bool>,
}

impl OwnershipState {
    fn intersect(&mut self, other: &Self) -> bool {
        let mut changed = false;
        for (value, incoming) in self.values.iter_mut().zip(&other.values) {
            let next = *value && *incoming;
            changed |= next != *value;
            *value = next;
        }
        for (place, incoming) in self.places.iter_mut().zip(&other.places) {
            let next = *place && *incoming;
            changed |= next != *place;
            *place = next;
        }
        changed
    }
}

pub(super) struct OwnershipError {
    pub(super) span: Span,
    pub(super) message: String,
}

pub(super) fn verify_ownership(program: &Program) -> Result<(), OwnershipError> {
    for function in &program.functions {
        let mut entry = OwnershipState {
            values: vec![false; function.value_types.len()],
            places: vec![false; function.place_types.len()],
        };
        for param in &function.blocks[0].params {
            if matches!(
                function.value_types[param.0],
                Type::Data(_) | Type::ArrayI64(_)
            ) {
                entry.values[param.0] = true;
            }
        }
        let mut incoming = vec![None; function.blocks.len()];
        incoming[0] = Some(entry);
        let mut ready = VecDeque::from([0usize]);
        while let Some(block_id) = ready.pop_front() {
            let block = &function.blocks[block_id];
            let mut state = incoming[block_id].clone().expect("queued block has state");
            for param in &block.params {
                if matches!(
                    function.value_types[param.0],
                    Type::Data(_) | Type::ArrayI64(_)
                ) {
                    state.values[param.0] = true;
                }
            }
            let value_use = |state: &mut OwnershipState,
                             value: ValueId,
                             consume: bool,
                             span: Span| {
                if matches!(
                    function.value_types[value.0],
                    Type::Data(_) | Type::ArrayI64(_)
                ) {
                    if !state.values[value.0] {
                        return Err(OwnershipError {
                            span,
                            message: format!("{}: use after move of v{}", function.name, value.0),
                        });
                    }
                    if consume {
                        state.values[value.0] = false;
                    }
                }
                Ok(())
            };
            for instruction in &block.instructions {
                let span = instruction.span;
                match &instruction.operation {
                    Operation::Move(value) => value_use(&mut state, *value, true, span)?,
                    Operation::CloseCaps(value) => value_use(&mut state, *value, true, span)?,
                    Operation::Variant(_, _, payload) => {
                        if let Some(value) = payload {
                            value_use(&mut state, *value, true, span)?;
                        }
                    }
                    Operation::Product(_, fields) | Operation::ArrayI64(fields) => {
                        for field in fields {
                            value_use(&mut state, *field, true, span)?;
                        }
                    }
                    Operation::Call(_, args) => {
                        for arg in args {
                            if let CallArg::Value(value) = arg {
                                value_use(&mut state, *value, true, span)?;
                            }
                        }
                    }
                    Operation::GenericCall { args, .. } => {
                        for value in args {
                            value_use(&mut state, *value, true, span)?;
                        }
                    }
                    Operation::PlaceInit(place, value) | Operation::Store(place, value) => {
                        value_use(&mut state, *value, true, span)?;
                        if matches!(
                            function.place_types[place.0],
                            Type::Data(_) | Type::ArrayI64(_)
                        ) || capability_layout::Layout::bare_right(function.place_types[place.0])
                        {
                            state.places[place.0] = true;
                        }
                    }
                    Operation::DestBegin(place) => {
                        state.places[place.0] = true;
                    }
                    Operation::DestField(place, _, value) => {
                        if !state.places[place.0] {
                            return Err(OwnershipError {
                                span,
                                message: format!(
                                    "{}: destination p{} is not live",
                                    function.name, place.0
                                ),
                            });
                        }
                        value_use(&mut state, *value, true, span)?;
                    }
                    Operation::DestPublish(place) => {
                        if !state.places[place.0] {
                            return Err(OwnershipError {
                                span,
                                message: format!(
                                    "{}: destination p{} is not live",
                                    function.name, place.0
                                ),
                            });
                        }
                        state.places[place.0] = false;
                    }
                    Operation::Take(place) => {
                        if !state.places[place.0] {
                            return Err(OwnershipError {
                                span,
                                message: format!(
                                    "{}: use after move of p{}",
                                    function.name, place.0
                                ),
                            });
                        }
                        state.places[place.0] = false;
                    }
                    Operation::ReadField(place, _)
                    | Operation::ArrayGetPlace(place, _)
                    | Operation::ArraySetPlace(place, _, _)
                    | Operation::CopyPlace(place)
                    | Operation::InferredCopyPlace(place) => {
                        if !state.places[place.0] {
                            return Err(OwnershipError {
                                span,
                                message: format!(
                                    "{}: use after move of p{}",
                                    function.name, place.0
                                ),
                            });
                        }
                    }
                    Operation::Field(owner, _) => value_use(&mut state, *owner, false, span)?,
                    other => {
                        for value in operands(other) {
                            value_use(&mut state, value, false, span)?;
                        }
                    }
                }
                if let Some(result) = instruction.result
                    && matches!(
                        function.value_types[result.0],
                        Type::Data(_) | Type::ArrayI64(_)
                    )
                {
                    state.values[result.0] = true;
                }
            }
            let span = block
                .instructions
                .last()
                .map(|item| item.span)
                .unwrap_or(Span { start: 0, end: 0 });
            let mut spawn_success = None;
            let successors = match &block.terminator {
                Terminator::Yield { resume, .. } | Terminator::ScopeJoin { resume, .. } => {
                    vec![resume.0]
                }
                Terminator::Return(value) => {
                    value_use(&mut state, *value, true, span)?;
                    vec![]
                }
                Terminator::Jump(target, args) => {
                    for value in args {
                        value_use(&mut state, *value, true, span)?;
                    }
                    vec![target.0]
                }
                Terminator::Branch {
                    then_block,
                    else_block,
                    ..
                } => {
                    vec![then_block.0, else_block.0]
                }
                Terminator::Match { subject, arms } => {
                    value_use(&mut state, *subject, true, span)?;
                    arms.iter().map(|arm| arm.target.0).collect()
                }
                Terminator::ChannelNew {
                    success, failure, ..
                }
                | Terminator::TypedChannelNew {
                    success, failure, ..
                } => vec![success.0, failure.0],
                Terminator::ReplyNew { success, failure } => vec![success.0, failure.0],
                Terminator::RequestChannelNew {
                    success, failure, ..
                } => vec![success.0, failure.0],
                Terminator::RequestUnpack { request, next } => {
                    value_use(&mut state, *request, true, span)?;
                    vec![next.0]
                }
                Terminator::ProductUnpack { product, next } => {
                    value_use(&mut state, *product, true, span)?;
                    vec![next.0]
                }
                Terminator::RequestSend { sent, closed, .. } => vec![sent.0, closed.0],
                Terminator::TypedSend {
                    message,
                    sent,
                    closed,
                    ..
                } => {
                    value_use(&mut state, *message, true, span)?;
                    vec![sent.0, closed.0]
                }
                Terminator::RequestReceive { value, end, .. }
                | Terminator::TypedReceive { value, end, .. } => vec![value.0, end.0],
                Terminator::Fork {
                    success, failure, ..
                } => vec![success.0, failure.0],
                Terminator::ChannelSend { sent, closed, .. } => vec![sent.0, closed.0],
                Terminator::ChannelReceive { value, end, .. } => vec![value.0, end.0],
                Terminator::ReplyComplete {
                    right,
                    delivered,
                    closed,
                    ..
                } => {
                    value_use(&mut state, *right, true, span)?;
                    vec![delivered.0, closed.0]
                }
                Terminator::ReplyReceive {
                    receiver,
                    value,
                    end,
                } => {
                    value_use(&mut state, *receiver, true, span)?;
                    vec![value.0, end.0]
                }
                Terminator::Spawn {
                    args,
                    success,
                    failure,
                    ..
                } => {
                    let mut moved = state.clone();
                    for value in args {
                        value_use(&mut moved, *value, true, span)?;
                    }
                    spawn_success = Some(moved);
                    vec![success.0, failure.0]
                }
            };
            for (edge, target) in successors.into_iter().enumerate() {
                let outgoing = if edge == 0 {
                    spawn_success.as_ref().unwrap_or(&state)
                } else {
                    &state
                };
                match &mut incoming[target] {
                    None => {
                        incoming[target] = Some(outgoing.clone());
                        ready.push_back(target);
                    }
                    Some(prior) => {
                        if prior.intersect(outgoing) {
                            ready.push_back(target);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn verify(program: &Program) -> Result<(), String> {
    verify_structure(program, None)?;
    lexical_loans::verify_lexical_loans(program)?;
    super::scope_verification::verify(program)?;
    verify_ownership(program).map_err(|error| error.message)?;
    verify_capability_edges(program)
}

pub fn verify_linked(program: &Program, generics: &CheckedModule) -> Result<(), String> {
    verify_structure(program, Some(generics))?;
    lexical_loans::verify_lexical_loans(program)?;
    super::scope_verification::verify(program)?;
    verify_ownership(program).map_err(|error| error.message)?;
    verify_capability_edges(program)
}
