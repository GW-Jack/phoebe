use super::*;

pub(super) struct AbiShapes {
    pub(super) data: Vec<Vec<cranelift_codegen::ir::Type>>,
    pointer_type: cranelift_codegen::ir::Type,
}

impl AbiShapes {
    pub(super) fn new(program: &Program, pointer_type: cranelift_codegen::ir::Type) -> Self {
        let mut shapes = Self {
            data: Vec::with_capacity(program.data.len()),
            pointer_type,
        };
        for definition in &program.data {
            let shape = if !definition.fields.is_empty() {
                definition
                    .fields
                    .iter()
                    .flat_map(|(_, ty)| shapes.types(*ty))
                    .collect()
            } else {
                let words = definition
                    .payloads
                    .iter()
                    .flatten()
                    .map(|ty| shapes.types(*ty).len())
                    .max()
                    .unwrap_or(0);
                vec![types::I64; 1 + words]
            };
            shapes.data.push(shape);
        }
        shapes
    }

    pub(super) fn types(&self, ty: Type) -> Vec<cranelift_codegen::ir::Type> {
        match ty {
            Type::I64 => vec![types::I64],
            Type::Bool => vec![types::I8],
            Type::Data(id) => self.data[id.0].clone(),
            Type::RefI64 | Type::RefMutI64 => vec![self.pointer_type],
            Type::SenderI64 | Type::ReceiverI64 | Type::ResourceError => vec![types::I64],
        }
    }
}

/// Physical word ranges for the retained functions, derived from the same
/// flattened shapes used by direct Cranelift calls and block arguments.
pub fn frame_word_layouts(program: &Program) -> Result<Vec<FrameWords>, String> {
    let frames = frame_layout::analyze(program)?;
    let shapes = AbiShapes::new(program, types::I64);
    frames
        .iter()
        .map(|frame| frame_layout::word_layout(program, frame, |ty| shapes.types(ty).len()))
        .collect()
}

#[derive(Clone)]
struct SpawnEntry {
    id: FuncId,
    layout: FrameWords,
    needs_wrapper: bool,
}

pub(super) fn compile_functions<M: Module>(
    module: &mut M,
    program: &Program,
    context_functions: &[bool],
    export_main: bool,
    mut clif: Option<&mut String>,
    // A poll module passes the direct-call closure; ordinary direct builds
    // define every non-suspendable function.
    selected: Option<&[bool]>,
) -> Result<Vec<(FuncId, Signature)>, String> {
    let uses_runtime = context_functions.iter().any(|needed| *needed);
    let suspendable = frame_layout::suspendable_functions(program)?;
    let mut poll_layouts = vec![None; program.functions.len()];
    if program.functions.iter().any(|function| {
        function.blocks.iter().any(|block| {
            matches!(block.terminator, Terminator::Spawn { child, .. } if suspendable[child.0])
        })
    }) {
        for layout in frame_word_layouts(program)? {
            let index = layout.function.0;
            poll_layouts[index] = Some(layout);
        }
    }
    let pointer_type = module.target_config().pointer_type();
    let shapes = AbiShapes::new(program, pointer_type);
    let runtime_imports = if uses_runtime {
        let mut create_signature = module.make_signature();
        create_signature.params.extend([
            AbiParam::new(pointer_type),
            AbiParam::new(types::I64),
            AbiParam::new(pointer_type),
            AbiParam::new(pointer_type),
        ]);
        create_signature.returns.push(AbiParam::new(types::I64));
        let create = module
            .declare_function(
                "phoebe_rt_v1_channel_new_i64",
                Linkage::Import,
                &create_signature,
            )
            .map_err(|error| error.to_string())?;
        let mut close_signature = module.make_signature();
        close_signature.params.extend([
            AbiParam::new(pointer_type),
            AbiParam::new(types::I64),
            AbiParam::new(types::I64),
        ]);
        close_signature.returns.push(AbiParam::new(types::I64));
        let close = module
            .declare_function(
                "phoebe_rt_v1_close_handle_i64",
                Linkage::Import,
                &close_signature,
            )
            .map_err(|error| error.to_string())?;
        let mut spawn_signature = module.make_signature();
        spawn_signature.params.extend([
            AbiParam::new(pointer_type),
            AbiParam::new(pointer_type),
            AbiParam::new(types::I64),
            AbiParam::new(pointer_type),
            AbiParam::new(types::I64),
            AbiParam::new(pointer_type),
            AbiParam::new(types::I64),
        ]);
        spawn_signature.returns.push(AbiParam::new(types::I64));
        let spawn = module
            .declare_function("phoebe_rt_v1_spawn_poll", Linkage::Import, &spawn_signature)
            .map_err(|error| error.to_string())?;
        Some((create, close, spawn))
    } else {
        None
    };
    let mut ids = Vec::with_capacity(program.functions.len());
    for (index, function) in program.functions.iter().enumerate() {
        let mut signature = module.make_signature();
        if context_functions[index] {
            signature.params.push(AbiParam::new(pointer_type));
        }
        for (index, mode) in function.param_modes.iter().enumerate() {
            let components = if *mode == ParamMode::Own {
                shapes.types(function.value_types[function.blocks[0].params[index].0])
            } else {
                vec![pointer_type]
            };
            signature
                .params
                .extend(components.into_iter().map(AbiParam::new));
        }
        signature.returns.extend(
            shapes
                .types(function.return_type)
                .into_iter()
                .map(AbiParam::new),
        );
        let symbol = if function.name == "main" {
            "phoebe_main".to_owned()
        } else {
            format!("phoebe_fn_{index}")
        };
        let linkage = if suspendable[index] || selected.is_some_and(|set| !set[index]) {
            Linkage::Import
        } else if export_main && function.name == "main" {
            Linkage::Export
        } else {
            Linkage::Local
        };
        let id = module
            .declare_function(&symbol, linkage, &signature)
            .map_err(|e| e.to_string())?;
        ids.push((id, signature));
    }
    let mut spawn_wrappers: Vec<Option<SpawnEntry>> = vec![None; program.functions.len()];
    for (index, function) in program.functions.iter().enumerate() {
        // Selected poll parents are emitted elsewhere, but still need a
        // wrapper when they spawn a direct child compiled by this pass.
        if (suspendable[index] && selected.is_none()) || selected.is_some_and(|set| !set[index]) {
            continue;
        }
        for block in &function.blocks {
            if let Terminator::Spawn { child, .. } = block.terminator
                && spawn_wrappers[child.0].is_none()
            {
                if suspendable[child.0] {
                    if suspendable[index] {
                        continue;
                    }
                    let mut signature = module.make_signature();
                    signature
                        .params
                        .extend([AbiParam::new(pointer_type), AbiParam::new(pointer_type)]);
                    signature.returns.push(AbiParam::new(types::I64));
                    let id = module
                        .declare_function(
                            &format!("phoebe_spawn_poll_{}", child.0),
                            Linkage::Import,
                            &signature,
                        )
                        .map_err(|error| error.to_string())?;
                    let layout = poll_layouts[child.0]
                        .as_ref()
                        .ok_or("suspendable spawn child has no frame")?
                        .clone();
                    spawn_wrappers[child.0] = Some(SpawnEntry {
                        id,
                        layout,
                        needs_wrapper: false,
                    });
                    continue;
                }
                let empty_frame = FunctionFrame {
                    function: child,
                    points: Vec::new(),
                    value_slots: Vec::new(),
                    place_slots: Vec::new(),
                };
                let layout =
                    frame_layout::word_layout(program, &empty_frame, |ty| shapes.types(ty).len())?;
                let mut signature = module.make_signature();
                signature
                    .params
                    .extend([AbiParam::new(pointer_type), AbiParam::new(pointer_type)]);
                signature.returns.push(AbiParam::new(types::I64));
                let id = module
                    .declare_function(
                        &format!("phoebe_spawn_poll_{}", child.0),
                        Linkage::Local,
                        &signature,
                    )
                    .map_err(|error| error.to_string())?;
                spawn_wrappers[child.0] = Some(SpawnEntry {
                    id,
                    layout,
                    needs_wrapper: true,
                });
            }
        }
    }
    let mut ctx = module.make_context();
    let mut builder_context = FunctionBuilderContext::new();
    for (index, function) in program.functions.iter().enumerate() {
        if suspendable[index] || selected.is_some_and(|set| !set[index]) {
            continue;
        }
        let (id, signature) = &ids[index];
        ctx.func.signature = signature.clone();
        ctx.func.name = UserFuncName::user(0, id.as_u32());
        {
            let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_context);
            let blocks = function
                .blocks
                .iter()
                .map(|_| builder.create_block())
                .collect::<Vec<_>>();
            let places = function
                .place_types
                .iter()
                .map(|ty| {
                    shapes
                        .types(*ty)
                        .into_iter()
                        .map(|component| {
                            let (size, align_shift) = if component == types::I8 {
                                (1, 0)
                            } else {
                                (8, 3)
                            };
                            builder.create_sized_stack_slot(StackSlotData::new(
                                StackSlotKind::ExplicitSlot,
                                size,
                                align_shift,
                            ))
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            builder.append_block_params_for_function_params(blocks[0]);
            for (block_id, block) in function.blocks.iter().enumerate().skip(1) {
                for param in &block.params {
                    for ty in shapes.types(function.value_types[param.0]) {
                        builder.append_block_param(blocks[block_id], ty);
                    }
                }
            }
            let mut values: Vec<Option<Vec<ClValue>>> = vec![None; function.value_types.len()];
            let runtime_context =
                context_functions[index].then(|| builder.block_params(blocks[0])[0]);
            for (block_id, block) in function.blocks.iter().enumerate() {
                let mut offset = usize::from(context_functions[index] && block_id == 0);
                let raw = builder.block_params(blocks[block_id]);
                for param in &block.params {
                    let count = shapes.types(function.value_types[param.0]).len();
                    values[param.0] = Some(raw[offset..offset + count].to_vec());
                    offset += count;
                }
            }
            let mut order = Vec::with_capacity(function.blocks.len());
            let mut seen = vec![false; function.blocks.len()];
            let mut ready = VecDeque::from([0usize]);
            seen[0] = true;
            while let Some(block_id) = ready.pop_front() {
                order.push(block_id);
                let successors = match &function.blocks[block_id].terminator {
                    Terminator::Return(_) => vec![],
                    Terminator::Jump(target, _) => vec![target.0],
                    Terminator::Branch {
                        then_block,
                        else_block,
                        ..
                    } => vec![then_block.0, else_block.0],
                    Terminator::Match { arms, .. } => arms.iter().map(|arm| arm.target.0).collect(),
                    Terminator::ChannelNew {
                        success, failure, ..
                    } => vec![success.0, failure.0],
                    Terminator::ChannelSend { sent, closed, .. } => vec![sent.0, closed.0],
                    Terminator::ChannelReceive { value, end, .. } => vec![value.0, end.0],
                    Terminator::Spawn {
                        success, failure, ..
                    } => vec![success.0, failure.0],
                };
                for target in successors {
                    if !seen[target] {
                        seen[target] = true;
                        ready.push_back(target);
                    }
                }
            }
            for block_id in order {
                let block = &function.blocks[block_id];
                builder.switch_to_block(blocks[block_id]);
                for instruction in &block.instructions {
                    let get = |id: core::ValueId| {
                        values[id.0].clone().expect("verified value is defined")
                    };
                    let one = |id: core::ValueId| get(id)[0];
                    let value = match &instruction.operation {
                        Operation::Const(value) => {
                            Some(vec![builder.ins().iconst(types::I64, *value)])
                        }
                        Operation::Bool(value) => {
                            Some(vec![builder.ins().iconst(types::I8, i64::from(*value))])
                        }
                        Operation::Move(value) | Operation::Copy(value) => Some(get(*value)),
                        Operation::Variant(id, tag, payload) => {
                            let mut components =
                                vec![builder.ins().iconst(types::I64, *tag as i64)];
                            if let Some(value) = payload {
                                let shape = shapes.types(function.value_types[value.0]);
                                for (component, ty) in get(*value).into_iter().zip(shape) {
                                    components.push(if ty == types::I8 {
                                        builder.ins().uextend(types::I64, component)
                                    } else {
                                        component
                                    });
                                }
                            }
                            let words = shapes.data[id.0].len() - 1;
                            while components.len() < 1 + words {
                                components.push(builder.ins().iconst(types::I64, 0));
                            }
                            Some(components)
                        }
                        Operation::Product(_, fields) => {
                            Some(fields.iter().flat_map(|field| get(*field)).collect())
                        }
                        Operation::Field(owner, index) => {
                            let Type::Data(id) = function.value_types[owner.0] else {
                                unreachable!("verified field owner")
                            };
                            let fields = &program.data[id.0].fields;
                            let start = fields[..*index]
                                .iter()
                                .map(|(_, ty)| shapes.types(*ty).len())
                                .sum::<usize>();
                            let width = shapes.types(fields[*index].1).len();
                            Some(get(*owner)[start..start + width].to_vec())
                        }
                        Operation::ReadField(place, index) => {
                            let Type::Data(id) = function.place_types[place.0] else {
                                unreachable!("verified place field owner")
                            };
                            let fields = &program.data[id.0].fields;
                            let offset = fields[..*index]
                                .iter()
                                .map(|(_, ty)| shapes.types(*ty).len())
                                .sum::<usize>();
                            let ty = shapes.types(fields[*index].1)[0];
                            Some(vec![builder.ins().stack_load(
                                pointer_type,
                                ty,
                                places[place.0][offset],
                                0,
                            )])
                        }
                        Operation::Neg(a) => Some(vec![builder.ins().ineg(one(*a))]),
                        Operation::Add(a, b) => Some(vec![builder.ins().iadd(one(*a), one(*b))]),
                        Operation::Sub(a, b) => Some(vec![builder.ins().isub(one(*a), one(*b))]),
                        Operation::Mul(a, b) => Some(vec![builder.ins().imul(one(*a), one(*b))]),
                        Operation::Eq(a, b) => {
                            Some(vec![builder.ins().icmp(IntCC::Equal, one(*a), one(*b))])
                        }
                        Operation::Lt(a, b) => Some(vec![builder.ins().icmp(
                            IntCC::SignedLessThan,
                            one(*a),
                            one(*b),
                        )]),
                        Operation::Call(target, args) => {
                            let callee = module.declare_func_in_func(ids[target.0].0, builder.func);
                            let mut args = args
                                .iter()
                                .flat_map(|arg| match arg {
                                    CallArg::Value(value) | CallArg::Ref(value, _) => get(*value),
                                    CallArg::Borrow(place, _) => {
                                        vec![builder.ins().stack_addr(
                                            pointer_type,
                                            places[place.0][0],
                                            0,
                                        )]
                                    }
                                })
                                .collect::<Vec<_>>();
                            if context_functions[target.0] {
                                let context = runtime_context.expect("caller context propagation");
                                args.insert(0, context);
                            }
                            let call = builder.ins().call(callee, &args);
                            Some(builder.inst_results(call).to_vec())
                        }
                        Operation::LoadRef(reference) => Some(vec![builder.ins().load(
                            types::I64,
                            MemFlagsData::new(),
                            one(*reference),
                            0,
                        )]),
                        Operation::StoreRef(reference, value) => {
                            builder.ins().store(
                                MemFlagsData::new(),
                                one(*value),
                                one(*reference),
                                0,
                            );
                            None
                        }
                        Operation::CloseHandle(value) => {
                            let (_, close, _) = runtime_imports.expect("channel runtime imports");
                            let callee = module.declare_func_in_func(close, builder.func);
                            let kind = match function.value_types[value.0] {
                                Type::SenderI64 => 0,
                                Type::ReceiverI64 => 1,
                                _ => unreachable!("verified handle kind"),
                            };
                            let kind = builder.ins().iconst(types::I64, kind);
                            builder.ins().call(
                                callee,
                                &[runtime_context.expect("runtime context"), one(*value), kind],
                            );
                            None
                        }
                        Operation::PlaceInit(place, value) | Operation::Store(place, value) => {
                            for (component, slot) in get(*value).into_iter().zip(&places[place.0]) {
                                builder.ins().stack_store(pointer_type, component, *slot, 0);
                            }
                            None
                        }
                        Operation::Load(place)
                        | Operation::Take(place)
                        | Operation::CopyPlace(place) => Some(
                            shapes
                                .types(function.place_types[place.0])
                                .into_iter()
                                .zip(&places[place.0])
                                .map(|(ty, slot)| {
                                    builder.ins().stack_load(pointer_type, ty, *slot, 0)
                                })
                                .collect(),
                        ),
                    };
                    if let Some(result) = instruction.result {
                        values[result.0] = value;
                    }
                }
                let get =
                    |id: core::ValueId| values[id.0].clone().expect("verified value is defined");
                match &block.terminator {
                    Terminator::Return(value) => {
                        builder.ins().return_(&get(*value));
                    }
                    Terminator::Jump(target, args) => {
                        let args = args
                            .iter()
                            .flat_map(|arg| get(*arg))
                            .map(Into::into)
                            .collect::<Vec<_>>();
                        builder.ins().jump(blocks[target.0], &args);
                    }
                    Terminator::Branch {
                        condition,
                        then_block,
                        else_block,
                    } => {
                        builder.ins().brif(
                            get(*condition)[0],
                            blocks[then_block.0],
                            &[],
                            blocks[else_block.0],
                            &[],
                        );
                    }
                    Terminator::Match { subject, arms } => {
                        let Type::Data(id) = function.value_types[subject.0] else {
                            unreachable!("verified match subject")
                        };
                        let data = &program.data[id.0];
                        let components = get(*subject);
                        for (tag, arm) in arms.iter().enumerate() {
                            let args = match arm.binding {
                                MatchBinding::Discard => vec![],
                                MatchBinding::Whole => {
                                    components.iter().copied().map(Into::into).collect()
                                }
                                MatchBinding::Payload => {
                                    let ty = data.payloads[tag].expect("verified payload arm");
                                    shapes
                                        .types(ty)
                                        .into_iter()
                                        .enumerate()
                                        .map(|(index, shape)| {
                                            let component = components[1 + index];
                                            if shape == types::I8 {
                                                builder.ins().ireduce(types::I8, component).into()
                                            } else {
                                                component.into()
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                }
                            };
                            if tag + 1 == arms.len() {
                                builder.ins().jump(blocks[arm.target.0], &args);
                            } else {
                                let next = builder.create_block();
                                let expected = builder.ins().iconst(types::I64, tag as i64);
                                let equal =
                                    builder.ins().icmp(IntCC::Equal, components[0], expected);
                                builder
                                    .ins()
                                    .brif(equal, blocks[arm.target.0], &args, next, &[]);
                                builder.switch_to_block(next);
                            }
                        }
                    }
                    Terminator::ChannelNew {
                        capacity,
                        success,
                        failure,
                    } => {
                        let (create, _, _) = runtime_imports.expect("channel runtime imports");
                        let callee = module.declare_func_in_func(create, builder.func);
                        let sender_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let receiver_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let sender_address = builder.ins().stack_addr(pointer_type, sender_slot, 0);
                        let receiver_address =
                            builder.ins().stack_addr(pointer_type, receiver_slot, 0);
                        let call = builder.ins().call(
                            callee,
                            &[
                                runtime_context.expect("runtime context"),
                                get(*capacity)[0],
                                sender_address,
                                receiver_address,
                            ],
                        );
                        let status = builder.inst_results(call)[0];
                        let zero = builder.ins().iconst(types::I64, 0);
                        let succeeded = builder.ins().icmp(IntCC::Equal, status, zero);
                        let success_load = builder.create_block();
                        builder.ins().brif(
                            succeeded,
                            success_load,
                            &[],
                            blocks[failure.0],
                            &[status.into()],
                        );
                        builder.switch_to_block(success_load);
                        let sender =
                            builder
                                .ins()
                                .stack_load(pointer_type, types::I64, sender_slot, 0);
                        let receiver =
                            builder
                                .ins()
                                .stack_load(pointer_type, types::I64, receiver_slot, 0);
                        builder
                            .ins()
                            .jump(blocks[success.0], &[sender.into(), receiver.into()]);
                    }
                    Terminator::Spawn {
                        child,
                        args,
                        success,
                        failure,
                    } => {
                        let (_, _, spawn) = runtime_imports.expect("spawn runtime import");
                        let entry = spawn_wrappers[child.0]
                            .as_ref()
                            .expect("declared spawn wrapper");
                        let layout = &entry.layout;
                        let wrapper_ref = module.declare_func_in_func(entry.id, builder.func);
                        let entry = builder.ins().func_addr(pointer_type, wrapper_ref);
                        let mut initial = Vec::new();
                        for (arg, slot) in args.iter().zip(&layout.entry_params) {
                            for (part, (value, ty)) in get(*arg)
                                .into_iter()
                                .zip(shapes.types(function.value_types[arg.0]))
                                .enumerate()
                            {
                                let word = if ty == types::I8 {
                                    builder.ins().uextend(types::I64, value)
                                } else {
                                    value
                                };
                                initial.push((slot.words.start + part, word));
                            }
                        }
                        let initial_ptr = if initial.is_empty() {
                            builder.ins().iconst(pointer_type, 0)
                        } else {
                            let bytes = u32::try_from(
                                initial
                                    .len()
                                    .checked_mul(16)
                                    .ok_or("spawn frame arguments overflow")?,
                            )
                            .map_err(|_| "spawn frame arguments exceed stack slot")?;
                            let slot = builder.create_sized_stack_slot(StackSlotData::new(
                                StackSlotKind::ExplicitSlot,
                                bytes,
                                3,
                            ));
                            for (index, (word, value)) in initial.iter().enumerate() {
                                let word = builder.ins().iconst(
                                    types::I64,
                                    i64::try_from(*word)
                                        .map_err(|_| "spawn frame offset overflow")?,
                                );
                                builder.ins().stack_store(
                                    pointer_type,
                                    word,
                                    slot,
                                    i32::try_from(index * 16)
                                        .map_err(|_| "spawn argument offset overflow")?,
                                );
                                builder.ins().stack_store(
                                    pointer_type,
                                    *value,
                                    slot,
                                    i32::try_from(index * 16 + 8)
                                        .map_err(|_| "spawn argument offset overflow")?,
                                );
                            }
                            builder.ins().stack_addr(pointer_type, slot, 0)
                        };
                        let transfers = args
                            .iter()
                            .filter_map(|arg| match function.value_types[arg.0] {
                                Type::SenderI64 | Type::ReceiverI64 => Some(get(*arg)[0]),
                                _ => None,
                            })
                            .collect::<Vec<_>>();
                        let transfers_ptr = if transfers.is_empty() {
                            builder.ins().iconst(pointer_type, 0)
                        } else {
                            let bytes = u32::try_from(
                                transfers
                                    .len()
                                    .checked_mul(8)
                                    .ok_or("spawn transfer list overflow")?,
                            )
                            .map_err(|_| "spawn transfer list exceeds stack slot")?;
                            let slot = builder.create_sized_stack_slot(StackSlotData::new(
                                StackSlotKind::ExplicitSlot,
                                bytes,
                                3,
                            ));
                            for (index, value) in transfers.iter().enumerate() {
                                builder.ins().stack_store(
                                    pointer_type,
                                    *value,
                                    slot,
                                    i32::try_from(index * 8)
                                        .map_err(|_| "spawn transfer offset overflow")?,
                                );
                            }
                            builder.ins().stack_addr(pointer_type, slot, 0)
                        };
                        let words = builder.ins().iconst(
                            types::I64,
                            i64::try_from(layout.total_words)
                                .map_err(|_| "spawn frame size overflow")?,
                        );
                        let initial_count = builder.ins().iconst(
                            types::I64,
                            i64::try_from(initial.len())
                                .map_err(|_| "spawn argument count overflow")?,
                        );
                        let transfer_count = builder.ins().iconst(
                            types::I64,
                            i64::try_from(transfers.len())
                                .map_err(|_| "spawn transfer count overflow")?,
                        );
                        let callee = module.declare_func_in_func(spawn, builder.func);
                        let call = builder.ins().call(
                            callee,
                            &[
                                runtime_context.expect("runtime context"),
                                entry,
                                words,
                                initial_ptr,
                                initial_count,
                                transfers_ptr,
                                transfer_count,
                            ],
                        );
                        let status = builder.inst_results(call)[0];
                        let zero = builder.ins().iconst(types::I64, 0);
                        let admitted = builder.ins().icmp(IntCC::Equal, status, zero);
                        builder.ins().brif(
                            admitted,
                            blocks[success.0],
                            &[],
                            blocks[failure.0],
                            &[status.into()],
                        );
                    }
                    Terminator::ChannelSend { .. } | Terminator::ChannelReceive { .. } => {
                        return Err("native channel traffic requires retained task frames".into());
                    }
                }
            }
            builder.seal_all_blocks();
            builder.finalize(module.target_config());
        }
        if let Some(output) = clif.as_deref_mut() {
            writeln!(
                output,
                "; Phoebe function {}\n{}",
                function.name,
                ctx.func.display()
            )
            .map_err(|error| error.to_string())?;
        }
        module
            .define_function(*id, &mut ctx)
            .map_err(|e| e.to_string())?;
        module.clear_context(&mut ctx);
    }
    for (child, wrapper) in spawn_wrappers.iter().enumerate() {
        let Some(entry) = wrapper else {
            continue;
        };
        if !entry.needs_wrapper {
            continue;
        }
        let wrapper_id = entry.id;
        let layout = &entry.layout;
        let mut signature = module.make_signature();
        signature
            .params
            .extend([AbiParam::new(pointer_type), AbiParam::new(pointer_type)]);
        signature.returns.push(AbiParam::new(types::I64));
        ctx.func.signature = signature;
        ctx.func.name = UserFuncName::user(0, wrapper_id.as_u32());
        {
            let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_context);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            let bridge = builder.block_params(entry)[0];
            let frame = builder.block_params(entry)[1];
            let mut args = Vec::new();
            if context_functions[child] {
                args.push(bridge);
            }
            let function = &program.functions[child];
            for slot in &layout.entry_params {
                for (part, ty) in shapes
                    .types(function.value_types[slot.id.0])
                    .into_iter()
                    .enumerate()
                {
                    let offset = i32::try_from((slot.words.start + part) * 8)
                        .map_err(|_| "spawn frame argument offset overflow")?;
                    args.push(builder.ins().load(ty, MemFlagsData::new(), frame, offset));
                }
            }
            let target = module.declare_func_in_func(ids[child].0, builder.func);
            let call = builder.ins().call(target, &args);
            let result = builder.inst_results(call)[0];
            let offset = i32::try_from(layout.result.start * 8)
                .map_err(|_| "spawn result offset overflow")?;
            builder
                .ins()
                .store(MemFlagsData::new(), result, frame, offset);
            let ready = builder.ins().iconst(types::I64, 0);
            builder.ins().return_(&[ready]);
            builder.seal_all_blocks();
            builder.finalize(module.target_config());
        }
        if let Some(output) = clif.as_deref_mut() {
            writeln!(
                output,
                "; Phoebe spawn poll wrapper for {}\n{}",
                program.functions[child].name,
                ctx.func.display()
            )
            .map_err(|error| error.to_string())?;
        }
        module
            .define_function(wrapper_id, &mut ctx)
            .map_err(|e| e.to_string())?;
        module.clear_context(&mut ctx);
    }
    Ok(ids)
}
