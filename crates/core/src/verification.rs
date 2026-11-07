use super::*;

fn op_type(
    program: &Program,
    function: &Function,
    operation: &Operation,
) -> Result<Option<Type>, String> {
    let ty = |id: ValueId| function.value_types[id.0];
    match operation {
        Operation::Const(_) => Ok(Some(Type::I64)),
        Operation::Bool(_) => Ok(Some(Type::Bool)),
        Operation::Move(value) => {
            if !matches!(ty(*value), Type::Data(_)) {
                return Err("move requires a nominal value".into());
            }
            Ok(Some(ty(*value)))
        }
        Operation::Copy(value) => {
            if !matches!(ty(*value), Type::Data(_)) {
                return Err("copy requires a nominal value".into());
            }
            Ok(Some(ty(*value)))
        }
        Operation::Variant(data_id, tag, payload) => {
            let definition = program
                .data
                .get(data_id.0)
                .ok_or_else(|| "invalid data ID".to_owned())?;
            if !definition.fields.is_empty() || *tag >= definition.variants.len() {
                return Err("invalid variant tag".into());
            }
            if definition.payloads[*tag] != payload.map(ty) {
                return Err("variant payload type mismatch".into());
            }
            Ok(Some(Type::Data(*data_id)))
        }
        Operation::Product(data_id, fields) => {
            let definition = program.data.get(data_id.0).ok_or("invalid data ID")?;
            if definition.fields.len() != fields.len() || fields.is_empty() {
                return Err("product field count mismatch".into());
            }
            for (value, (_, expected)) in fields.iter().zip(&definition.fields) {
                if ty(*value) != *expected {
                    return Err("product field type mismatch".into());
                }
            }
            Ok(Some(Type::Data(*data_id)))
        }
        Operation::Field(value, index) => {
            let Type::Data(id) = ty(*value) else {
                return Err("field access requires a product".into());
            };
            program
                .data
                .get(id.0)
                .and_then(|definition| definition.fields.get(*index))
                .map(|(_, ty)| Some(*ty))
                .ok_or_else(|| "invalid field access".into())
        }
        Operation::ReadField(place, index) => {
            let Type::Data(id) = *function
                .place_types
                .get(place.0)
                .ok_or("invalid place ID")?
            else {
                return Err("place field access requires a product".into());
            };
            let field_type = program.data[id.0]
                .fields
                .get(*index)
                .map(|(_, ty)| *ty)
                .ok_or_else(|| "invalid place field access".to_owned())?;
            if matches!(field_type, Type::Data(_)) {
                return Err("nominal place field requires an owning projection".into());
            }
            Ok(Some(field_type))
        }
        Operation::Neg(a) => {
            if ty(*a) != Type::I64 {
                return Err("neg expects I64".into());
            }
            Ok(Some(Type::I64))
        }
        Operation::Add(a, b) | Operation::Sub(a, b) | Operation::Mul(a, b) => {
            if ty(*a) != Type::I64 || ty(*b) != Type::I64 {
                return Err("arithmetic expects I64".into());
            }
            Ok(Some(Type::I64))
        }
        Operation::Eq(a, b) => {
            if ty(*a) != ty(*b) || !matches!(ty(*a), Type::I64 | Type::Bool | Type::Data(_)) {
                return Err("equality type mismatch".into());
            }
            if let Type::Data(id) = ty(*a)
                && (!program.data[id.0].fields.is_empty()
                    || program.data[id.0].payloads.iter().any(Option::is_some))
            {
                return Err("product equality and payload sum equality are not implemented".into());
            }
            Ok(Some(Type::Bool))
        }
        Operation::Lt(a, b) => {
            if ty(*a) != Type::I64 || ty(*b) != Type::I64 {
                return Err("less-than expects I64".into());
            }
            Ok(Some(Type::Bool))
        }
        Operation::Call(target, args) => {
            let callee = program
                .functions
                .get(target.0)
                .ok_or_else(|| "invalid call target".to_owned())?;
            let callee_params = callee
                .blocks
                .first()
                .ok_or_else(|| "callee has no entry block".to_owned())?
                .params
                .as_slice();
            if args.len() != callee.param_modes.len()
                || args.len() != callee_params.len()
                || check_call_aliases(args).is_err()
            {
                return Err("call signature mismatch".into());
            }
            for ((arg, mode), param) in args.iter().zip(&callee.param_modes).zip(callee_params) {
                let expected = *callee
                    .value_types
                    .get(param.0)
                    .ok_or_else(|| "invalid callee parameter".to_owned())?;
                let valid = match (arg, mode) {
                    (CallArg::Value(value), ParamMode::Own) => ty(*value) == expected,
                    (CallArg::Borrow(place, false), ParamMode::Shared)
                    | (CallArg::Borrow(place, true), ParamMode::Exclusive) => {
                        function.place_types.get(place.0) == Some(&Type::I64)
                    }
                    (CallArg::Ref(value, false), ParamMode::Shared) => {
                        matches!(ty(*value), Type::RefI64 | Type::RefMutI64)
                    }
                    (CallArg::Ref(value, true), ParamMode::Exclusive) => {
                        ty(*value) == Type::RefMutI64
                    }
                    _ => false,
                };
                if !valid {
                    return Err("call signature mismatch".into());
                }
            }
            Ok(Some(callee.return_type))
        }
        Operation::LoadRef(reference) => {
            if !matches!(ty(*reference), Type::RefI64 | Type::RefMutI64) {
                return Err("load requires a reference".into());
            }
            Ok(Some(Type::I64))
        }
        Operation::StoreRef(reference, value) => {
            if ty(*reference) != Type::RefMutI64 || ty(*value) != Type::I64 {
                return Err("store requires an exclusive I64 reference".into());
            }
            Ok(None)
        }
        Operation::CloseHandle(value) => {
            if !matches!(ty(*value), Type::SenderI64 | Type::ReceiverI64) {
                return Err("close.handle requires a channel endpoint".into());
            }
            Ok(None)
        }
        Operation::PlaceInit(place, value) | Operation::Store(place, value) => {
            let place_ty = function
                .place_types
                .get(place.0)
                .ok_or_else(|| "invalid place ID".to_owned())?;
            if ty(*value) != *place_ty {
                return Err("place store type mismatch".into());
            }
            Ok(None)
        }
        Operation::Load(place) => match function.place_types.get(place.0) {
            Some(Type::I64) => Ok(Some(Type::I64)),
            Some(Type::Bool) => Ok(Some(Type::Bool)),
            Some(_) => Err("nominal place requires take or field read".into()),
            None => Err("invalid place ID".into()),
        },
        Operation::Take(place) => match function.place_types.get(place.0) {
            Some(ty @ Type::Data(_)) => Ok(Some(*ty)),
            Some(_) => Err("take requires a nominal place".into()),
            None => Err("invalid place ID".into()),
        },
        Operation::CopyPlace(place) => match function.place_types.get(place.0) {
            Some(ty @ Type::Data(_)) => Ok(Some(*ty)),
            Some(_) => Err("copy requires a nominal place".into()),
            None => Err("invalid place ID".into()),
        },
    }
}

/// SSA values read by an operation, used by frame liveness and diagnostics.
pub fn operands(operation: &Operation) -> Vec<ValueId> {
    match operation {
        Operation::Const(_) | Operation::Bool(_) => vec![],
        Operation::Move(value) | Operation::Copy(value) => vec![*value],
        Operation::Variant(_, _, payload) => payload.iter().copied().collect(),
        Operation::Product(_, fields) => fields.clone(),
        Operation::Field(value, _) => vec![*value],
        Operation::CloseHandle(a)
        | Operation::Neg(a)
        | Operation::PlaceInit(_, a)
        | Operation::Store(_, a)
        | Operation::LoadRef(a) => vec![*a],
        Operation::StoreRef(a, b) => vec![*a, *b],
        Operation::Load(_)
        | Operation::Take(_)
        | Operation::CopyPlace(_)
        | Operation::ReadField(_, _) => vec![],
        Operation::Add(a, b)
        | Operation::Sub(a, b)
        | Operation::Mul(a, b)
        | Operation::Eq(a, b)
        | Operation::Lt(a, b) => vec![*a, *b],
        Operation::Call(_, args) => args
            .iter()
            .filter_map(|arg| match arg {
                CallArg::Value(value) | CallArg::Ref(value, _) => Some(*value),
                CallArg::Borrow(_, _) => None,
            })
            .collect(),
    }
}

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
            if matches!(function.value_types[param.0], Type::Data(_)) {
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
                if matches!(function.value_types[param.0], Type::Data(_)) {
                    state.values[param.0] = true;
                }
            }
            let value_use = |state: &mut OwnershipState,
                             value: ValueId,
                             consume: bool,
                             span: Span| {
                if matches!(function.value_types[value.0], Type::Data(_)) {
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
                    Operation::Variant(_, _, payload) => {
                        if let Some(value) = payload {
                            value_use(&mut state, *value, true, span)?;
                        }
                    }
                    Operation::Product(_, fields) => {
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
                    Operation::PlaceInit(place, value) | Operation::Store(place, value) => {
                        value_use(&mut state, *value, true, span)?;
                        if matches!(function.place_types[place.0], Type::Data(_)) {
                            state.places[place.0] = true;
                        }
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
                    Operation::ReadField(place, _) | Operation::CopyPlace(place) => {
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
                    Operation::Field(owner, index) => {
                        let Type::Data(id) = function.value_types[owner.0] else {
                            unreachable!("structure verifier checked field owner")
                        };
                        let field_type = program.data[id.0].fields[*index].1;
                        value_use(
                            &mut state,
                            *owner,
                            matches!(field_type, Type::Data(_)),
                            span,
                        )?;
                    }
                    other => {
                        for value in operands(other) {
                            value_use(&mut state, value, false, span)?;
                        }
                    }
                }
                if let Some(result) = instruction.result
                    && matches!(function.value_types[result.0], Type::Data(_))
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
                } => vec![success.0, failure.0],
                Terminator::ChannelSend { sent, closed, .. } => vec![sent.0, closed.0],
                Terminator::ChannelReceive { value, end, .. } => vec![value.0, end.0],
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

pub(super) fn verify_structure(program: &Program) -> Result<(), String> {
    if program.functions.is_empty() {
        return Err("empty Core IR program".into());
    }
    let mut data_names = std::collections::HashSet::new();
    for (index, definition) in program.data.iter().enumerate() {
        if !data_names.insert(&definition.name)
            || definition.variants.is_empty() == definition.fields.is_empty()
            || definition.payloads.len() != definition.variants.len()
        {
            return Err("invalid data declaration".into());
        }
        let mut variants = std::collections::HashSet::new();
        if definition
            .variants
            .iter()
            .any(|variant| !variants.insert(variant))
        {
            return Err("duplicate data variant".into());
        }
        if definition.payloads.iter().flatten().any(|ty| match ty {
            Type::I64 | Type::Bool => false,
            Type::Data(id) => id.0 >= index,
            Type::ResourceError => false,
            Type::RefI64 | Type::RefMutI64 | Type::SenderI64 | Type::ReceiverI64 => true,
        }) {
            return Err("invalid variant payload type".into());
        }
        let mut fields = std::collections::HashSet::new();
        for (name, ty) in &definition.fields {
            if !fields.insert(name)
                || match ty {
                    Type::I64 | Type::Bool => false,
                    Type::Data(id) => id.0 >= index,
                    Type::ResourceError => false,
                    Type::RefI64 | Type::RefMutI64 | Type::SenderI64 | Type::ReceiverI64 => true,
                }
            {
                return Err("invalid product field".into());
            }
        }
    }
    for function in &program.functions {
        for ty in function
            .value_types
            .iter()
            .chain(&function.place_types)
            .chain(std::iter::once(&function.return_type))
        {
            if let Type::Data(id) = ty
                && id.0 >= program.data.len()
            {
                return Err(format!("{}: invalid data type ID", function.name));
            }
        }
        if !matches!(function.return_type, Type::I64 | Type::Bool | Type::Data(_)) {
            return Err(format!("{}: invalid return type", function.name));
        }
        if function.blocks.is_empty() {
            return Err(format!("{}: no entry block", function.name));
        }
        if function.blocks[0].params.len() > 4 {
            return Err(format!(
                "{}: at most four parameters are supported",
                function.name
            ));
        }
        if function.param_modes.len() != function.blocks[0].params.len() {
            return Err(format!("{}: parameter mode count mismatch", function.name));
        }
        if function
            .place_types
            .iter()
            .any(|ty| !matches!(ty, Type::I64 | Type::Bool | Type::Data(_)))
        {
            return Err(format!(
                "{}: references cannot be stored in places",
                function.name
            ));
        }
        let count = function.value_types.len();
        let mut definitions = vec![None; count];
        let mut place_initializers = vec![None; function.place_types.len()];
        for (block_id, block) in function.blocks.iter().enumerate() {
            for (param_index, param) in block.params.iter().enumerate() {
                let slot = definitions
                    .get_mut(param.0)
                    .ok_or_else(|| format!("{}: invalid value ID", function.name))?;
                if slot.replace((block_id, None)).is_some() {
                    return Err(format!("{}: duplicate value definition", function.name));
                }
                if block_id == 0 {
                    let expected = match function.param_modes[param_index] {
                        ParamMode::Own => function.value_types[param.0],
                        ParamMode::Shared => Type::RefI64,
                        ParamMode::Exclusive => Type::RefMutI64,
                    };
                    if param_index != param.0
                        || function.value_types[param.0] != expected
                        || (function.param_modes[param_index] == ParamMode::Own
                            && !matches!(
                                expected,
                                Type::I64
                                    | Type::Bool
                                    | Type::Data(_)
                                    | Type::SenderI64
                                    | Type::ReceiverI64
                                    | Type::ResourceError
                            ))
                    {
                        return Err(format!("{}: invalid function parameter", function.name));
                    }
                } else if matches!(
                    function.value_types[param.0],
                    Type::RefI64 | Type::RefMutI64
                ) {
                    return Err(format!(
                        "{}: reference cannot be a block parameter",
                        function.name
                    ));
                }
            }
            for (index, instruction) in block.instructions.iter().enumerate() {
                if let Some(result) = instruction.result {
                    let slot = definitions
                        .get_mut(result.0)
                        .ok_or_else(|| format!("{}: invalid value ID", function.name))?;
                    if slot.replace((block_id, Some(index))).is_some() {
                        return Err(format!("{}: duplicate value definition", function.name));
                    }
                }
                if let Operation::PlaceInit(place, _) = instruction.operation {
                    let slot = place_initializers
                        .get_mut(place.0)
                        .ok_or_else(|| format!("{}: invalid place ID", function.name))?;
                    if slot.replace((block_id, index)).is_some() {
                        return Err(format!("{}: duplicate place initialization", function.name));
                    }
                }
            }
        }
        if definitions.iter().any(Option::is_none) {
            return Err(format!("{}: undefined value ID", function.name));
        }
        if place_initializers.iter().any(Option::is_none) {
            return Err(format!("{}: uninitialized place", function.name));
        }
        let mut successors = vec![Vec::new(); function.blocks.len()];
        let mut predecessors = vec![Vec::new(); function.blocks.len()];
        for (block_id, block) in function.blocks.iter().enumerate() {
            let targets = match &block.terminator {
                Terminator::Return(_) => vec![],
                Terminator::Jump(target, _) => vec![*target],
                Terminator::Branch {
                    then_block,
                    else_block,
                    ..
                } => vec![*then_block, *else_block],
                Terminator::Match { arms, .. } => arms.iter().map(|arm| arm.target).collect(),
                Terminator::ChannelNew {
                    success, failure, ..
                } => {
                    if success == failure {
                        return Err(format!(
                            "{}: creation edges must be distinct",
                            function.name
                        ));
                    }
                    vec![*success, *failure]
                }
                Terminator::ChannelSend { sent, closed, .. } => {
                    if sent == closed {
                        return Err(format!("{}: send edges must be distinct", function.name));
                    }
                    vec![*sent, *closed]
                }
                Terminator::ChannelReceive { value, end, .. } => {
                    if value == end {
                        return Err(format!("{}: receive edges must be distinct", function.name));
                    }
                    vec![*value, *end]
                }
                Terminator::Spawn {
                    success, failure, ..
                } => {
                    if success == failure {
                        return Err(format!("{}: spawn edges must be distinct", function.name));
                    }
                    vec![*success, *failure]
                }
            };
            for target in targets {
                if target.0 >= function.blocks.len() {
                    return Err(format!("{}: invalid branch target", function.name));
                }
                successors[block_id].push(target.0);
                predecessors[target.0].push(block_id);
            }
        }
        if !predecessors[0].is_empty() {
            return Err(format!("{}: entry block has a predecessor", function.name));
        }
        let mut reachable = vec![false; function.blocks.len()];
        let mut ready = VecDeque::from([0]);
        reachable[0] = true;
        while let Some(id) = ready.pop_front() {
            for target in &successors[id] {
                if !reachable[*target] {
                    reachable[*target] = true;
                    ready.push_back(*target);
                }
            }
        }
        if reachable.iter().any(|yes| !yes) {
            return Err(format!("{}: unreachable block", function.name));
        }
        let all = (0..function.blocks.len()).collect::<std::collections::HashSet<_>>();
        let mut dominators = vec![all.clone(); function.blocks.len()];
        dominators[0] = std::collections::HashSet::from([0]);
        let mut changed = true;
        while changed {
            changed = false;
            for id in 1..function.blocks.len() {
                let mut next = all.clone();
                for predecessor in &predecessors[id] {
                    next.retain(|candidate| dominators[*predecessor].contains(candidate));
                }
                next.insert(id);
                if next != dominators[id] {
                    dominators[id] = next;
                    changed = true;
                }
            }
        }
        let check_use =
            |value: ValueId, block_id: usize, instruction_index: usize| -> Result<(), String> {
                let (def_block, def_index) = definitions
                    .get(value.0)
                    .and_then(|item| *item)
                    .ok_or_else(|| format!("{}: undefined operand", function.name))?;
                if def_block == block_id {
                    if def_index.is_some_and(|index| index >= instruction_index) {
                        return Err(format!("{}: use before definition", function.name));
                    }
                } else if !dominators[block_id].contains(&def_block) {
                    return Err(format!("{}: non-dominating value use", function.name));
                }
                Ok(())
            };
        let check_place = |place: PlaceId, block_id: usize, index: usize| -> Result<(), String> {
            let (init_block, init_index) =
                place_initializers
                    .get(place.0)
                    .and_then(|item| *item)
                    .ok_or_else(|| format!("{}: invalid place ID", function.name))?;
            if init_block == block_id {
                if init_index >= index {
                    return Err(format!(
                        "{}: place used before initialization",
                        function.name
                    ));
                }
            } else if !dominators[block_id].contains(&init_block) {
                return Err(format!(
                    "{}: non-dominating place initialization",
                    function.name
                ));
            }
            Ok(())
        };
        for (block_id, block) in function.blocks.iter().enumerate() {
            for (index, instruction) in block.instructions.iter().enumerate() {
                for operand in operands(&instruction.operation) {
                    check_use(operand, block_id, index)?;
                }
                let expected = op_type(program, function, &instruction.operation)?;
                if instruction
                    .result
                    .map(|result| function.value_types[result.0])
                    != expected
                {
                    return Err(format!(
                        "{}: instruction result type mismatch",
                        function.name
                    ));
                }
                if let Operation::Load(place)
                | Operation::Take(place)
                | Operation::CopyPlace(place)
                | Operation::ReadField(place, _)
                | Operation::Store(place, _) = instruction.operation
                {
                    check_place(place, block_id, index)?;
                }
                if let Operation::Call(_, args) = &instruction.operation {
                    for arg in args {
                        if let CallArg::Borrow(place, _) = arg {
                            check_place(*place, block_id, index)?;
                        }
                    }
                }
            }
            let end = block.instructions.len();
            match &block.terminator {
                Terminator::Return(value) => {
                    check_use(*value, block_id, end)?;
                    if function.value_types[value.0] != function.return_type {
                        return Err(format!("{}: return type mismatch", function.name));
                    }
                }
                Terminator::Jump(target, args) => {
                    let params = &function.blocks[target.0].params;
                    if args.len() != params.len() {
                        return Err(format!("{}: block argument count mismatch", function.name));
                    }
                    for (arg, param) in args.iter().zip(params) {
                        check_use(*arg, block_id, end)?;
                        if function.value_types[arg.0] != function.value_types[param.0] {
                            return Err(format!("{}: block argument type mismatch", function.name));
                        }
                    }
                }
                Terminator::Branch {
                    condition,
                    then_block,
                    else_block,
                } => {
                    check_use(*condition, block_id, end)?;
                    if function.value_types[condition.0] != Type::Bool {
                        return Err(format!("{}: branch requires Bool", function.name));
                    }
                    if !function.blocks[then_block.0].params.is_empty()
                        || !function.blocks[else_block.0].params.is_empty()
                    {
                        return Err(format!(
                            "{}: conditional target cannot have parameters",
                            function.name
                        ));
                    }
                }
                Terminator::Match { subject, arms } => {
                    check_use(*subject, block_id, end)?;
                    let Type::Data(id) = function.value_types[subject.0] else {
                        return Err(format!("{}: match requires a sum value", function.name));
                    };
                    let data = &program.data[id.0];
                    if data.variants.is_empty() || arms.len() != data.variants.len() {
                        return Err(format!("{}: match arm count mismatch", function.name));
                    }
                    let mut distinct = std::collections::HashSet::new();
                    for (tag, arm) in arms.iter().enumerate() {
                        if !distinct.insert(arm.target.0) {
                            return Err(format!("{}: duplicate match target", function.name));
                        }
                        let params = &function.blocks[arm.target.0].params;
                        let valid = match arm.binding {
                            MatchBinding::Discard => params.is_empty(),
                            MatchBinding::Payload => data.payloads[tag].is_some_and(|ty| {
                                params.len() == 1 && function.value_types[params[0].0] == ty
                            }),
                            MatchBinding::Whole => {
                                params.len() == 1
                                    && function.value_types[params[0].0] == Type::Data(id)
                            }
                        };
                        if !valid {
                            return Err(format!(
                                "{}: match payload parameter mismatch",
                                function.name
                            ));
                        }
                    }
                }
                Terminator::ChannelNew {
                    capacity,
                    success,
                    failure,
                } => {
                    check_use(*capacity, block_id, end)?;
                    if function.value_types[capacity.0] != Type::I64 {
                        return Err(format!("{}: channel capacity must be I64", function.name));
                    }
                    let success_params = &function.blocks[success.0].params;
                    let failure_params = &function.blocks[failure.0].params;
                    if success_params.len() != 2
                        || function.value_types[success_params[0].0] != Type::SenderI64
                        || function.value_types[success_params[1].0] != Type::ReceiverI64
                        || failure_params.len() != 1
                        || function.value_types[failure_params[0].0] != Type::ResourceError
                    {
                        return Err(format!(
                            "{}: channel creation edge signature mismatch",
                            function.name
                        ));
                    }
                }
                Terminator::ChannelSend {
                    sender,
                    message,
                    sent,
                    closed,
                } => {
                    check_use(*sender, block_id, end)?;
                    check_use(*message, block_id, end)?;
                    if function.value_types[sender.0] != Type::SenderI64
                        || function.value_types[message.0] != Type::I64
                        || !function.blocks[sent.0].params.is_empty()
                        || !function.blocks[closed.0].params.is_empty()
                    {
                        return Err(format!(
                            "{}: channel send edge signature mismatch",
                            function.name
                        ));
                    }
                }
                Terminator::ChannelReceive {
                    receiver,
                    value,
                    end: end_block,
                } => {
                    check_use(*receiver, block_id, end)?;
                    let params = &function.blocks[value.0].params;
                    if function.value_types[receiver.0] != Type::ReceiverI64
                        || params.len() != 1
                        || function.value_types[params[0].0] != Type::I64
                        || !function.blocks[end_block.0].params.is_empty()
                    {
                        return Err(format!(
                            "{}: channel receive edge signature mismatch",
                            function.name
                        ));
                    }
                }
                Terminator::Spawn {
                    child,
                    args,
                    success,
                    failure,
                } => {
                    let callee = program
                        .functions
                        .get(child.0)
                        .ok_or_else(|| format!("{}: invalid spawn target", function.name))?;
                    let params = callee
                        .blocks
                        .first()
                        .ok_or_else(|| format!("{}: spawn target has no entry", function.name))?
                        .params
                        .as_slice();
                    if callee.return_type != Type::I64
                        || callee
                            .param_modes
                            .iter()
                            .any(|mode| *mode != ParamMode::Own)
                        || args.len() != params.len()
                        || !function.blocks[success.0].params.is_empty()
                        || function.blocks[failure.0].params.len() != 1
                        || function.value_types[function.blocks[failure.0].params[0].0]
                            != Type::ResourceError
                    {
                        return Err(format!("{}: spawn edge signature mismatch", function.name));
                    }
                    for (arg, param) in args.iter().zip(params) {
                        check_use(*arg, block_id, end)?;
                        if function.value_types[arg.0] != callee.value_types[param.0] {
                            return Err(format!("{}: spawn argument type mismatch", function.name));
                        }
                    }
                }
            }
        }
    }
    // Guarded task stacks are not implemented. Reject recursive calls for now.
    let mut incoming = vec![0usize; program.functions.len()];
    let mut edges = vec![Vec::new(); program.functions.len()];
    for (caller, function) in program.functions.iter().enumerate() {
        for block in &function.blocks {
            for instruction in &block.instructions {
                if let Operation::Call(target, _) = &instruction.operation {
                    edges[caller].push(target.0);
                    incoming[target.0] += 1;
                }
            }
        }
    }
    let mut ready = incoming
        .iter()
        .enumerate()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| id)
        .collect::<VecDeque<_>>();
    let mut visited = 0;
    while let Some(id) = ready.pop_front() {
        visited += 1;
        for target in &edges[id] {
            incoming[*target] -= 1;
            if incoming[*target] == 0 {
                ready.push_back(*target);
            }
        }
    }
    if visited != program.functions.len() {
        return Err("recursive call cycle is not yet supported".into());
    }
    Ok(())
}

pub fn verify(program: &Program) -> Result<(), String> {
    verify_structure(program)?;
    verify_ownership(program).map_err(|error| error.message)?;
    verify_capability_edges(program)
}

fn is_channel_handle(ty: Type) -> bool {
    matches!(ty, Type::SenderI64 | Type::ReceiverI64)
}

pub(super) fn verify_capability_edges(program: &Program) -> Result<(), String> {
    for function in &program.functions {
        let Some((graph, _)) = capability_graph(function, &[], &[]) else {
            continue;
        };
        capability_flow::verify(&graph).map_err(|error| format!("{}: {error}", function.name))?;
    }
    Ok(())
}

pub(super) fn capability_graph(
    function: &Function,
    implicit_cleanup_returns: &[BlockId],
    scoped_cleanup_exits: &[(BlockId, ValueId, Span)],
) -> Option<(capability_flow::Graph, Vec<ValueId>)> {
    if !function.value_types.iter().copied().any(is_channel_handle) {
        return None;
    }
    // Capability flow only needs ownership state for rights, not for every
    // scalar SSA value in the function. Keep this numbering separate from
    // ValueId so adding ordinary computations does not enlarge each state.
    let mut slots = vec![None; function.value_types.len()];
    let mut values = Vec::new();
    let mut slot_count = 0;
    for (value, ty) in function.value_types.iter().enumerate() {
        if is_channel_handle(*ty) {
            slots[value] = Some(capability_flow::Slot(slot_count));
            values.push(ValueId(value));
            slot_count += 1;
        }
    }
    let slot = |value: ValueId| slots[value.0].expect("channel handle has a capability slot");
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
        .filter(|value| is_channel_handle(function.value_types[value.0]))
        .map(|value| slot(*value))
        .collect();
    let blocks = function
        .blocks
        .iter()
        .enumerate()
        .map(|(block_id, block)| {
            let mut actions = Vec::new();
            for instruction in &block.instructions {
                match &instruction.operation {
                    Operation::CloseHandle(value) => actions.push(CapAction::Close(slot(*value))),
                    Operation::Call(_, args) => {
                        for arg in args {
                            if let CallArg::Value(value) = arg
                                && is_channel_handle(function.value_types[value.0])
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
                        .filter(|(arg, _)| is_channel_handle(function.value_types[arg.0]))
                        .map(|(arg, param)| CapAction::Move {
                            from: slot(*arg),
                            to: slot(*param),
                        })
                        .collect();
                    vec![edge(CapTarget::Block(target.0), moves)]
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
                Terminator::Spawn {
                    args,
                    success,
                    failure,
                    ..
                } => {
                    let transfers = args
                        .iter()
                        .filter(|value| is_channel_handle(function.value_types[value.0]))
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
