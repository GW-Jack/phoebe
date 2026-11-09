//! Core structural, dominance, call, and loan-region checks.

use super::dominators::Dominators;
use super::*;

/// Recover a source call site from a cycle after structural verification has
/// reported recursion. This runs only on the diagnostic path.
pub(crate) fn recursive_call_span(program: &Program) -> Option<Span> {
    let edges = program
        .functions
        .iter()
        .map(|function| {
            function
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .filter_map(|instruction| match instruction.operation {
                    Operation::Call(target, _) => Some((target.0, instruction.span)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut state = vec![0u8; edges.len()];
    for start in 0..edges.len() {
        if state[start] != 0 {
            continue;
        }
        state[start] = 1;
        let mut stack = vec![(start, 0usize)];
        while let Some((node, next)) = stack.last_mut() {
            if *next == edges[*node].len() {
                state[*node] = 2;
                stack.pop();
                continue;
            }
            let (target, span) = edges[*node][*next];
            *next += 1;
            match state[target] {
                1 => return Some(span),
                0 => {
                    state[target] = 1;
                    stack.push((target, 0));
                }
                _ => {}
            }
        }
    }
    None
}

fn verify_loan_regions(function: &Function) -> Result<(), String> {
    let mut expected = Vec::new();
    for (block_id, block) in function.blocks.iter().enumerate() {
        for (instruction_id, instruction) in block.instructions.iter().enumerate() {
            let Operation::Call(_, args) = &instruction.operation else {
                continue;
            };
            let roots = args
                .iter()
                .filter_map(|arg| match arg {
                    CallArg::Borrow(place, exclusive) => Some(LoanRoot::Place(*place, *exclusive)),
                    CallArg::BorrowPath(place, path, exclusive) => {
                        Some(LoanRoot::Path(*place, path.clone(), *exclusive))
                    }
                    CallArg::Ref(value, exclusive) => Some(LoanRoot::Forwarded(*value, *exclusive)),
                    CallArg::Value(_) => None,
                })
                .collect::<Vec<_>>();
            if !roots.is_empty() {
                expected.push((BlockId(block_id), instruction_id, roots));
            }
        }
    }
    if function.loan_regions.len() != expected.len() {
        return Err(format!(
            "{}: call loan region count mismatch",
            function.name
        ));
    }
    for (index, (region, (block, instruction, roots))) in
        function.loan_regions.iter().zip(expected).enumerate()
    {
        if region.id != RegionId(index)
            || region.call_block != block
            || region.call_instruction != instruction
            || region.roots != roots
        {
            return Err(format!("{}: call loan region mismatch", function.name));
        }
    }
    Ok(())
}

pub(crate) fn verify_structure(
    program: &Program,
    generics: Option<&CheckedModule>,
) -> Result<(), String> {
    if program.functions.is_empty() {
        return Err("empty Core IR program".into());
    }
    let capability_layout = capability_layout::Layout::new(&program.data);
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
            Type::I64 | Type::Bool | Type::Unit => false,
            Type::ArrayI64(len) => *len > 32,
            Type::Data(id) => id.0 >= index,
            Type::ResourceError => false,
            Type::RefI64 | Type::RefMutI64 | Type::RefBool | Type::RefMutBool => true,
            Type::SenderI64 | Type::ReceiverI64 | Type::ReplyToI64 | Type::ReplyReceiverI64 => {
                false
            }
            Type::RequestI64 | Type::RequestSenderI64 | Type::RequestReceiverI64 => false,
            Type::TypedSender(message) | Type::TypedReceiver(message) => {
                matches!(message, MessageType::Data(id) if id.0 >= index)
            }
        }) {
            return Err("invalid variant payload type".into());
        }
        let mut fields = std::collections::HashSet::new();
        for (name, ty) in &definition.fields {
            if !fields.insert(name)
                || match ty {
                    Type::I64 | Type::Bool | Type::Unit => false,
                    Type::ArrayI64(len) => *len > 32,
                    Type::Data(id) => id.0 >= index,
                    Type::ResourceError => false,
                    Type::RefI64 | Type::RefMutI64 | Type::RefBool | Type::RefMutBool => true,
                    Type::SenderI64
                    | Type::ReceiverI64
                    | Type::ReplyToI64
                    | Type::ReplyReceiverI64
                    | Type::RequestI64
                    | Type::RequestSenderI64
                    | Type::RequestReceiverI64 => false,
                    Type::TypedSender(message) | Type::TypedReceiver(message) => {
                        matches!(message, MessageType::Data(id) if id.0 >= index)
                    }
                }
            {
                return Err("invalid product field".into());
            }
        }
    }
    let mut checked_messages = std::collections::HashSet::new();
    let mut message_catalog = None;
    for function in &program.functions {
        for ty in function
            .value_types
            .iter()
            .chain(&function.place_types)
            .chain(std::iter::once(&function.return_type))
        {
            if matches!(ty, Type::ArrayI64(len) if *len > 32) {
                return Err(format!(
                    "{}: inline I64 array exceeds 32 elements",
                    function.name
                ));
            }
            if let Type::Data(id) = ty
                && id.0 >= program.data.len()
            {
                return Err(format!("{}: invalid data type ID", function.name));
            }
            if let Type::TypedSender(message) | Type::TypedReceiver(message) = ty {
                if matches!(message, MessageType::Data(id) if id.0 >= program.data.len()) {
                    return Err(format!("{}: invalid typed message ID", function.name));
                }
                if checked_messages.insert(*message) {
                    if message_catalog.is_none() {
                        message_catalog = Some(phoebe_message::MessageCatalog::new(program)?);
                    }
                    message_catalog
                        .as_ref()
                        .expect("initialized message catalog")
                        .descriptor(message.value_type())
                        .map_err(|error| format!("{}: {error}", function.name))?;
                }
            }
        }
        if !matches!(
            function.return_type,
            Type::I64 | Type::Bool | Type::Unit | Type::ArrayI64(_) | Type::Data(_)
        ) {
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
        verify_loan_regions(function)?;
        if function.place_types.iter().any(|ty| {
            !matches!(
                ty,
                Type::I64 | Type::Bool | Type::Unit | Type::ArrayI64(_) | Type::Data(_)
            ) && !capability_layout::Layout::bare_right(*ty)
        }) {
            return Err(format!(
                "{}: references cannot be stored in places",
                function.name
            ));
        }
        let count = function.value_types.len();
        let mut definitions = vec![None; count];
        let mut place_initializers = vec![None; function.place_types.len()];
        let mut destinations = vec![false; function.place_types.len()];
        for (block_id, block) in function.blocks.iter().enumerate() {
            for (param_index, param) in block.params.iter().enumerate() {
                let slot = definitions
                    .get_mut(param.0)
                    .ok_or_else(|| format!("{}: invalid value ID", function.name))?;
                if slot.replace((block_id, None)).is_some() {
                    return Err(format!("{}: duplicate value definition", function.name));
                }
                if block_id == 0 {
                    let actual = function.value_types[param.0];
                    let valid = match function.param_modes[param_index] {
                        ParamMode::Own => matches!(
                            actual,
                            Type::I64
                                | Type::Bool
                                | Type::Unit
                                | Type::ArrayI64(_)
                                | Type::Data(_)
                                | Type::SenderI64
                                | Type::ReceiverI64
                                | Type::ReplyToI64
                                | Type::ReplyReceiverI64
                                | Type::RequestI64
                                | Type::RequestSenderI64
                                | Type::RequestReceiverI64
                                | Type::TypedSender(_)
                                | Type::TypedReceiver(_)
                                | Type::ResourceError
                        ),
                        ParamMode::Shared => matches!(actual, Type::RefI64 | Type::RefBool),
                        ParamMode::Exclusive => {
                            matches!(actual, Type::RefMutI64 | Type::RefMutBool)
                        }
                    };
                    if param_index != param.0 || !valid {
                        return Err(format!("{}: invalid function parameter", function.name));
                    }
                } else if matches!(
                    function.value_types[param.0],
                    Type::RefI64 | Type::RefMutI64 | Type::RefBool | Type::RefMutBool
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
                if let Operation::PlaceInit(place, _) | Operation::DestBegin(place) =
                    instruction.operation
                {
                    let slot = place_initializers
                        .get_mut(place.0)
                        .ok_or_else(|| format!("{}: invalid place ID", function.name))?;
                    if slot.replace((block_id, index)).is_some() {
                        return Err(format!("{}: duplicate place initialization", function.name));
                    }
                    destinations[place.0] =
                        matches!(instruction.operation, Operation::DestBegin(_));
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
                Terminator::Yield { resume, .. } | Terminator::ScopeJoin { resume, .. } => {
                    vec![*resume]
                }
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
                Terminator::TypedChannelNew {
                    success, failure, ..
                } => {
                    if success == failure {
                        return Err(format!(
                            "{}: typed creation edges must be distinct",
                            function.name
                        ));
                    }
                    vec![*success, *failure]
                }
                Terminator::ReplyNew { success, failure } => {
                    if success == failure {
                        return Err(format!(
                            "{}: reply creation edges must be distinct",
                            function.name
                        ));
                    }
                    vec![*success, *failure]
                }
                Terminator::RequestChannelNew {
                    success, failure, ..
                } => {
                    if success == failure {
                        return Err(format!(
                            "{}: request creation edges must be distinct",
                            function.name
                        ));
                    }
                    vec![*success, *failure]
                }
                Terminator::RequestUnpack { next, .. } | Terminator::ProductUnpack { next, .. } => {
                    vec![*next]
                }
                Terminator::RequestSend { sent, closed, .. } => {
                    if sent == closed {
                        return Err(format!(
                            "{}: request send edges must be distinct",
                            function.name
                        ));
                    }
                    vec![*sent, *closed]
                }
                Terminator::TypedSend { sent, closed, .. } => {
                    if sent == closed {
                        return Err(format!(
                            "{}: typed send edges must be distinct",
                            function.name
                        ));
                    }
                    vec![*sent, *closed]
                }
                Terminator::RequestReceive { value, end, .. } => {
                    if value == end {
                        return Err(format!(
                            "{}: request receive edges must be distinct",
                            function.name
                        ));
                    }
                    vec![*value, *end]
                }
                Terminator::TypedReceive { value, end, .. } => {
                    if value == end {
                        return Err(format!(
                            "{}: typed receive edges must be distinct",
                            function.name
                        ));
                    }
                    vec![*value, *end]
                }
                Terminator::Fork {
                    success, failure, ..
                } => {
                    if success == failure {
                        return Err(format!("{}: fork edges must be distinct", function.name));
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
                Terminator::ReplyComplete {
                    delivered, closed, ..
                } => {
                    if delivered == closed {
                        return Err(format!(
                            "{}: reply completion edges must be distinct",
                            function.name
                        ));
                    }
                    vec![*delivered, *closed]
                }
                Terminator::ReplyReceive { value, end, .. } => {
                    if value == end {
                        return Err(format!(
                            "{}: reply receive edges must be distinct",
                            function.name
                        ));
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
        let dominators = Dominators::new(&successors, &predecessors);
        let mut destination_fields = vec![[None; 2]; function.place_types.len()];
        let mut destination_publishes = vec![None; function.place_types.len()];
        for (block_id, block) in function.blocks.iter().enumerate() {
            for (index, instruction) in block.instructions.iter().enumerate() {
                match instruction.operation {
                    Operation::DestField(place, field, _) => {
                        if !destinations.get(place.0).copied().unwrap_or(false) {
                            return Err(format!(
                                "{}: field write needs a destination",
                                function.name
                            ));
                        }
                        let slot = destination_fields[place.0].get_mut(field).ok_or_else(|| {
                            format!("{}: invalid destination field", function.name)
                        })?;
                        if slot.replace((block_id, index)).is_some() {
                            return Err(format!("{}: duplicate destination field", function.name));
                        }
                    }
                    Operation::DestPublish(place) => {
                        if !destinations.get(place.0).copied().unwrap_or(false) {
                            return Err(format!(
                                "{}: publication needs a destination",
                                function.name
                            ));
                        }
                        if destination_publishes[place.0]
                            .replace((block_id, index))
                            .is_some()
                        {
                            return Err(format!(
                                "{}: duplicate destination publication",
                                function.name
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        for (place, is_destination) in destinations.iter().copied().enumerate() {
            if !is_destination {
                continue;
            }
            let (publish_block, publish_index) = destination_publishes[place]
                .ok_or_else(|| format!("{}: destination was not published", function.name))?;
            for field in destination_fields[place] {
                let (write_block, write_index) = field.ok_or_else(|| {
                    format!("{}: destination field was not initialized", function.name)
                })?;
                if (write_block == publish_block && write_index >= publish_index)
                    || (write_block != publish_block
                        && !dominators.dominates(write_block, publish_block))
                {
                    return Err(format!(
                        "{}: destination published before field initialization",
                        function.name
                    ));
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
                } else if !dominators.dominates(def_block, block_id) {
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
            } else if !dominators.dominates(init_block, block_id) {
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
                let expected = op_type(
                    program,
                    function,
                    &instruction.operation,
                    &capability_layout,
                    generics,
                )?;
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
                | Operation::InferredCopyPlace(place)
                | Operation::ReadField(place, _)
                | Operation::TakeField(place, _)
                | Operation::ArrayGetPlace(place, _)
                | Operation::ArraySetPlace(place, _, _)
                | Operation::DestField(place, _, _)
                | Operation::DestPublish(place)
                | Operation::DestTakeField(place, _)
                | Operation::BorrowBegin(place, _)
                | Operation::BorrowBeginPath(place, _, _)
                | Operation::Store(place, _) = instruction.operation
                {
                    check_place(place, block_id, index)?;
                    if destinations[place.0]
                        && !matches!(
                            instruction.operation,
                            Operation::DestField(_, _, _)
                                | Operation::DestPublish(_)
                                | Operation::DestTakeField(_, _)
                        )
                    {
                        return Err(format!(
                            "{}: unpublished destination cannot be read or replaced",
                            function.name
                        ));
                    }
                }
                if let Operation::DestTakeField(place, field) = instruction.operation {
                    let (write_block, write_index) = destination_fields[place.0][field]
                        .ok_or_else(|| {
                            format!("{}: taking an unwritten destination field", function.name)
                        })?;
                    if (write_block == block_id && write_index >= index)
                        || (write_block != block_id && !dominators.dominates(write_block, block_id))
                    {
                        return Err(format!(
                            "{}: taking an unwritten destination field",
                            function.name
                        ));
                    }
                }
                if let Operation::Call(_, args) = &instruction.operation {
                    for arg in args {
                        match arg {
                            CallArg::Borrow(place, _) | CallArg::BorrowPath(place, _, _) => {
                                check_place(*place, block_id, index)?;
                                if destinations[place.0] {
                                    return Err(format!(
                                        "{}: unpublished destination cannot be borrowed",
                                        function.name
                                    ));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            let end = block.instructions.len();
            match &block.terminator {
                Terminator::Yield { resume, .. } | Terminator::ScopeJoin { resume, .. } => {
                    if !function.blocks[resume.0].params.is_empty() {
                        return Err(format!(
                            "{}: yield resume cannot have parameters",
                            function.name
                        ));
                    }
                }
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
                            MatchBinding::Discard => {
                                params.is_empty()
                                    && data.payloads[tag]
                                        .is_none_or(|ty| capability_layout.leaves(ty) == 0)
                            }
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
                    ..
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
                Terminator::TypedChannelNew {
                    message,
                    capacity,
                    success,
                    failure,
                    ..
                } => {
                    check_use(*capacity, block_id, end)?;
                    let outputs = &function.blocks[success.0].params;
                    let errors = &function.blocks[failure.0].params;
                    if function.value_types[capacity.0] != Type::I64
                        || outputs.len() != 2
                        || function.value_types[outputs[0].0] != Type::TypedSender(*message)
                        || function.value_types[outputs[1].0] != Type::TypedReceiver(*message)
                        || errors.len() != 1
                        || function.value_types[errors[0].0] != Type::ResourceError
                    {
                        return Err(format!(
                            "{}: typed channel creation edge signature mismatch",
                            function.name
                        ));
                    }
                }
                Terminator::ReplyNew { success, failure } => {
                    let success_params = &function.blocks[success.0].params;
                    let failure_params = &function.blocks[failure.0].params;
                    if success_params.len() != 2
                        || function.value_types[success_params[0].0] != Type::ReplyToI64
                        || function.value_types[success_params[1].0] != Type::ReplyReceiverI64
                        || failure_params.len() != 1
                        || function.value_types[failure_params[0].0] != Type::ResourceError
                    {
                        return Err(format!(
                            "{}: reply creation edge signature mismatch",
                            function.name
                        ));
                    }
                }
                Terminator::RequestChannelNew {
                    capacity,
                    success,
                    failure,
                } => {
                    check_use(*capacity, block_id, end)?;
                    let params = &function.blocks[success.0].params;
                    let errors = &function.blocks[failure.0].params;
                    if function.value_types[capacity.0] != Type::I64
                        || params.len() != 2
                        || function.value_types[params[0].0] != Type::RequestSenderI64
                        || function.value_types[params[1].0] != Type::RequestReceiverI64
                        || errors.len() != 1
                        || function.value_types[errors[0].0] != Type::ResourceError
                    {
                        return Err(format!(
                            "{}: request channel creation edge signature mismatch",
                            function.name
                        ));
                    }
                }
                Terminator::RequestUnpack { request, next } => {
                    check_use(*request, block_id, end)?;
                    let params = &function.blocks[next.0].params;
                    if function.value_types[request.0] != Type::RequestI64
                        || params.len() != 2
                        || function.value_types[params[0].0] != Type::I64
                        || function.value_types[params[1].0] != Type::ReplyToI64
                    {
                        return Err(format!(
                            "{}: request unpack edge signature mismatch",
                            function.name
                        ));
                    }
                }
                Terminator::ProductUnpack { product, next } => {
                    check_use(*product, block_id, end)?;
                    let Type::Data(id) = function.value_types[product.0] else {
                        return Err(format!(
                            "{}: product unpack requires a product",
                            function.name
                        ));
                    };
                    let fields = &program.data[id.0].fields;
                    let params = &function.blocks[next.0].params;
                    if fields.is_empty()
                        || params.len() != fields.len()
                        || params
                            .iter()
                            .zip(fields)
                            .any(|(param, (_, ty))| function.value_types[param.0] != *ty)
                    {
                        return Err(format!(
                            "{}: product unpack edge signature mismatch",
                            function.name
                        ));
                    }
                }
                Terminator::RequestSend {
                    sender,
                    request,
                    sent,
                    closed,
                } => {
                    check_use(*sender, block_id, end)?;
                    check_use(*request, block_id, end)?;
                    let returned = &function.blocks[closed.0].params;
                    if function.value_types[sender.0] != Type::RequestSenderI64
                        || function.value_types[request.0] != Type::RequestI64
                        || !function.blocks[sent.0].params.is_empty()
                        || returned.len() != 1
                        || function.value_types[returned[0].0] != Type::RequestI64
                    {
                        return Err(format!(
                            "{}: request send edge signature mismatch",
                            function.name
                        ));
                    }
                }
                Terminator::TypedSend {
                    sender,
                    message,
                    sent,
                    closed,
                } => {
                    check_use(*sender, block_id, end)?;
                    check_use(*message, block_id, end)?;
                    let Type::TypedSender(message_type) = function.value_types[sender.0] else {
                        return Err(format!(
                            "{}: typed send requires a typed sender",
                            function.name
                        ));
                    };
                    let returned = &function.blocks[closed.0].params;
                    if function.value_types[message.0] != message_type.value_type()
                        || !function.blocks[sent.0].params.is_empty()
                        || returned.len() != 1
                        || function.value_types[returned[0].0] != message_type.value_type()
                    {
                        return Err(format!(
                            "{}: typed send edge signature mismatch",
                            function.name
                        ));
                    }
                }
                Terminator::RequestReceive {
                    receiver,
                    value,
                    end: end_block,
                } => {
                    check_use(*receiver, block_id, end)?;
                    let params = &function.blocks[value.0].params;
                    if function.value_types[receiver.0] != Type::RequestReceiverI64
                        || params.len() != 1
                        || function.value_types[params[0].0] != Type::RequestI64
                        || !function.blocks[end_block.0].params.is_empty()
                    {
                        return Err(format!(
                            "{}: request receive edge signature mismatch",
                            function.name
                        ));
                    }
                }
                Terminator::TypedReceive {
                    receiver,
                    value,
                    end: end_block,
                } => {
                    check_use(*receiver, block_id, end)?;
                    let Type::TypedReceiver(message_type) = function.value_types[receiver.0] else {
                        return Err(format!(
                            "{}: typed receive requires a typed receiver",
                            function.name
                        ));
                    };
                    let params = &function.blocks[value.0].params;
                    if params.len() != 1
                        || function.value_types[params[0].0] != message_type.value_type()
                        || !function.blocks[end_block.0].params.is_empty()
                    {
                        return Err(format!(
                            "{}: typed receive edge signature mismatch",
                            function.name
                        ));
                    }
                }
                Terminator::Fork {
                    source,
                    success,
                    failure,
                } => {
                    check_use(*source, block_id, end)?;
                    let ty = function.value_types[source.0];
                    if !matches!(
                        ty,
                        Type::SenderI64
                            | Type::ReceiverI64
                            | Type::RequestSenderI64
                            | Type::RequestReceiverI64
                            | Type::TypedSender(_)
                            | Type::TypedReceiver(_)
                    ) || function.blocks[success.0].params.len() != 1
                        || function.value_types[function.blocks[success.0].params[0].0] != ty
                        || function.blocks[failure.0].params.len() != 1
                        || function.value_types[function.blocks[failure.0].params[0].0]
                            != Type::ResourceError
                    {
                        return Err(format!("{}: fork edge signature mismatch", function.name));
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
                Terminator::ReplyComplete {
                    right,
                    value,
                    delivered,
                    closed,
                } => {
                    check_use(*right, block_id, end)?;
                    check_use(*value, block_id, end)?;
                    if function.value_types[right.0] != Type::ReplyToI64
                        || function.value_types[value.0] != Type::I64
                        || !function.blocks[delivered.0].params.is_empty()
                        || !function.blocks[closed.0].params.is_empty()
                    {
                        return Err(format!(
                            "{}: reply completion edge signature mismatch",
                            function.name
                        ));
                    }
                }
                Terminator::ReplyReceive {
                    receiver,
                    value,
                    end: end_block,
                } => {
                    check_use(*receiver, block_id, end)?;
                    let params = &function.blocks[value.0].params;
                    if function.value_types[receiver.0] != Type::ReplyReceiverI64
                        || params.len() != 1
                        || function.value_types[params[0].0] != Type::I64
                        || !function.blocks[end_block.0].params.is_empty()
                    {
                        return Err(format!(
                            "{}: reply receive edge signature mismatch",
                            function.name
                        ));
                    }
                }
                Terminator::Spawn {
                    child,
                    args,
                    success,
                    failure,
                    ..
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
                    if !matches!(callee.return_type, Type::I64 | Type::Unit)
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
