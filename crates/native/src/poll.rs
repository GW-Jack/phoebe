//! Retained native code path for checked scalar channel traffic, same-task
//! calls, and spawned poll children.

use super::*;
use cranelift_frontend::Variable;

pub struct PollExecutable {
    module: JITModule,
    entry: FuncId,
    layout: FrameWords,
    clif: String,
}

pub type PollLeafExecutable = PollExecutable;

impl PollExecutable {
    pub fn layout(&self) -> &FrameWords {
        &self.layout
    }

    pub fn clif(&self) -> &str {
        &self.clif
    }

    /// # Safety
    /// The executable must outlive every task using this pointer. Its frame
    /// must be allocated from `layout()` and initialized at `entry_params`.
    pub unsafe fn entry(&self) -> runtime::NativePollEntry {
        let pointer = self.module.get_finalized_function(self.entry);
        unsafe { mem::transmute(pointer) }
    }
}

/// Compatibility entry for callers that previously compiled a channel leaf.
pub fn compile_poll_leaf_jit(program: &Program, name: &str) -> Result<PollLeafExecutable, String> {
    compile_poll_jit(program, name)
}

/// Compile the reachable retained-call graph from an I64 task entry.
/// Unsupported operations are rejected before any JIT definition is published.
pub fn compile_poll_jit(program: &Program, name: &str) -> Result<PollExecutable, String> {
    let mut flags = settings::builder();
    flags
        .set("use_colocated_libcalls", "false")
        .map_err(|e| e.to_string())?;
    flags.set("is_pic", "false").map_err(|e| e.to_string())?;
    let isa = native_builder()
        .map_err(|e| format!("unsupported native host: {e}"))?
        .finish(settings::Flags::new(flags))
        .map_err(|e| e.to_string())?;
    let mut jit = JITBuilder::with_isa(isa, default_libcall_names());
    jit.symbol(
        "phoebe_rt_v1_receive_i64",
        runtime::phoebe_receive_i64 as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_send_i64",
        runtime::phoebe_send_i64 as *const u8,
    );
    jit.symbol("phoebe_rt_v1_yield", runtime::phoebe_yield as *const u8);
    jit.symbol(
        "phoebe_rt_v1_scope_enter",
        runtime::phoebe_scope_enter as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_scope_join",
        runtime::phoebe_scope_join as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_close_handle_i64",
        runtime::phoebe_close_handle_i64 as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_call_poll",
        runtime::phoebe_call_poll as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_channel_new_i64",
        runtime::phoebe_channel_new_i64 as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_fork_handle_i64",
        runtime::phoebe_fork_handle_i64 as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_spawn_poll",
        runtime::phoebe_spawn_poll as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_reply_new_i64",
        runtime::phoebe_reply_new_i64 as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_reply_close_i64",
        runtime::phoebe_reply_close_i64 as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_reply_complete_i64",
        runtime::phoebe_reply_complete_i64 as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_reply_receive_i64",
        runtime::phoebe_reply_receive_i64 as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_spawn_poll_caps",
        runtime::phoebe_spawn_poll_caps as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_request_channel_new_i64",
        runtime::phoebe_request_channel_new_i64 as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_send_request_i64",
        runtime::phoebe_send_request_i64 as *const u8,
    );
    jit.symbol(
        "phoebe_rt_v1_receive_request_i64",
        runtime::phoebe_receive_request_i64 as *const u8,
    );
    let mut module = JITModule::new(jit);
    let (entry, layout, clif) = compile_poll_module(&mut module, program, name, None)?;
    module.finalize_definitions().map_err(|e| e.to_string())?;
    Ok(PollExecutable {
        module,
        entry,
        layout,
        clif,
    })
}

pub(crate) fn emit_poll_object(
    program: &Program,
    name: &str,
    symbol: &str,
) -> Result<(Vec<u8>, usize), String> {
    let mut flags = settings::builder();
    flags
        .set("use_colocated_libcalls", "false")
        .map_err(|e| e.to_string())?;
    flags.set("is_pic", "true").map_err(|e| e.to_string())?;
    let isa = native_builder()
        .map_err(|e| format!("unsupported native host: {e}"))?
        .finish(settings::Flags::new(flags))
        .map_err(|e| e.to_string())?;
    let builder =
        ObjectBuilder::new(isa, "phoebe", default_libcall_names()).map_err(|e| e.to_string())?;
    let mut module = ObjectModule::new(builder);
    let (_, layout, _) = compile_poll_module(&mut module, program, name, Some(symbol))?;
    let object = module.finish().emit().map_err(|e| e.to_string())?;
    Ok((object, layout.total_words))
}

fn compile_poll_module<M: Module>(
    module: &mut M,
    program: &Program,
    name: &str,
    export_symbol: Option<&str>,
) -> Result<(FuncId, FrameWords, String), String> {
    core::verify(program)?;
    let (root, entry_function) = program
        .function(name)
        .ok_or_else(|| format!("unknown function `{name}`"))?;
    if entry_function.return_type != Type::I64 {
        return Err("external poll entry requires an I64 result".into());
    }
    if entry_function
        .param_modes
        .iter()
        .any(|mode| *mode != ParamMode::Own)
    {
        return Err("external poll entry requires owned parameters".into());
    }
    let suspendable = frame_layout::suspendable_functions(program)?;
    let mut reachable = Vec::new();
    let mut seen = vec![false; program.functions.len()];
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if seen[id.0] {
            continue;
        }
        seen[id.0] = true;
        reachable.push(id);
        for instruction in program.functions[id.0]
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
        {
            if let Operation::Call(child, _) = &instruction.operation
                && suspendable[child.0]
            {
                pending.push(*child);
            }
        }
        for block in &program.functions[id.0].blocks {
            if let Terminator::Spawn { child, .. } = block.terminator
                && suspendable[child.0]
            {
                pending.push(child);
            }
        }
    }
    reachable.sort_by_key(|id| id.0);
    let mut layouts = vec![None; program.functions.len()];
    for layout in frame_word_layouts(program)? {
        let index = layout.function.0;
        layouts[index] = Some(layout);
    }
    for id in &reachable {
        let layout = layouts[id.0]
            .as_ref()
            .ok_or("retained poll graph contains a direct-only callee")?;
        validate_poll_function(&program.functions[id.0], layout, &layouts)?;
    }
    let mut direct = vec![false; program.functions.len()];
    let mut pending_direct = reachable
        .iter()
        .flat_map(|id| &program.functions[id.0].blocks)
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| match instruction.operation {
            Operation::Call(child, _) if !suspendable[child.0] => Some(child),
            _ => None,
        })
        .collect::<Vec<_>>();
    pending_direct.extend(
        reachable
            .iter()
            .flat_map(|id| &program.functions[id.0].blocks)
            .filter_map(|block| match block.terminator {
                Terminator::Spawn { child, .. } if !suspendable[child.0] => Some(child),
                _ => None,
            }),
    );
    while let Some(id) = pending_direct.pop() {
        if direct[id.0] {
            continue;
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
                    return Err("direct helper spawning a suspendable child is not yet supported in a poll module".into());
                }
                pending_direct.push(child);
            }
        }
    }
    let root_layout = layouts[root.0].clone().expect("validated root frame");
    let pointer = module.target_config().pointer_type();
    let shapes = AbiShapes::new(program, pointer)?;
    let context_functions = program.runtime_context_functions();
    let mut clif = String::new();
    // The direct pass defines direct callees and spawned direct children.
    // It scans only reachable poll parents for their spawn wrappers.
    let mut selected = direct.clone();
    for id in &reachable {
        selected[id.0] = true;
    }
    let direct_ids = if direct.iter().any(|selected| *selected) {
        Some(compile_functions(
            module,
            program,
            &context_functions,
            false,
            Some(&mut clif),
            Some(&selected),
        )?)
    } else {
        None
    };
    let mut direct_spawn = vec![None; program.functions.len()];
    for id in &reachable {
        for block in &program.functions[id.0].blocks {
            if let Terminator::Spawn { child, .. } = block.terminator
                && !suspendable[child.0]
                && direct_spawn[child.0].is_none()
            {
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
                    .extend([AbiParam::new(pointer), AbiParam::new(pointer)]);
                signature.returns.push(AbiParam::new(types::I64));
                let wrapper = module
                    .declare_function(
                        &spawn_poll_symbol(&program.functions[child.0].name),
                        Linkage::Local,
                        &signature,
                    )
                    .map_err(|error| error.to_string())?;
                direct_spawn[child.0] = Some((wrapper, layout));
            }
        }
    }

    let mut receive_signature = module.make_signature();
    receive_signature.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(pointer),
    ]);
    receive_signature.returns.push(AbiParam::new(types::I64));
    let receive = module
        .declare_function(
            "phoebe_rt_v1_receive_i64",
            Linkage::Import,
            &receive_signature,
        )
        .map_err(|e| e.to_string())?;
    let mut send_signature = module.make_signature();
    send_signature.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(types::I64),
    ]);
    send_signature.returns.push(AbiParam::new(types::I64));
    let send = module
        .declare_function("phoebe_rt_v1_send_i64", Linkage::Import, &send_signature)
        .map_err(|e| e.to_string())?;
    let mut yield_signature = module.make_signature();
    yield_signature.params.push(AbiParam::new(pointer));
    yield_signature.returns.push(AbiParam::new(types::I64));
    let yield_function = module
        .declare_function("phoebe_rt_v1_yield", Linkage::Import, &yield_signature)
        .map_err(|e| e.to_string())?;
    let scope_enter = module
        .declare_function(
            "phoebe_rt_v1_scope_enter",
            Linkage::Import,
            &yield_signature,
        )
        .map_err(|e| e.to_string())?;
    let scope_join = module
        .declare_function("phoebe_rt_v1_scope_join", Linkage::Import, &yield_signature)
        .map_err(|e| e.to_string())?;
    let mut close_signature = module.make_signature();
    close_signature.params.extend([
        AbiParam::new(pointer),
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
        .map_err(|e| e.to_string())?;
    let mut call_signature = module.make_signature();
    call_signature.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(types::I64),
        AbiParam::new(types::I64),
    ]);
    call_signature.returns.push(AbiParam::new(types::I64));
    let call = module
        .declare_function("phoebe_rt_v1_call_poll", Linkage::Import, &call_signature)
        .map_err(|e| e.to_string())?;
    let mut create_signature = module.make_signature();
    create_signature.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(pointer),
        AbiParam::new(pointer),
    ]);
    create_signature.returns.push(AbiParam::new(types::I64));
    let create = module
        .declare_function(
            "phoebe_rt_v1_channel_new_i64",
            Linkage::Import,
            &create_signature,
        )
        .map_err(|e| e.to_string())?;
    let mut fork_signature = module.make_signature();
    fork_signature.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(types::I64),
        AbiParam::new(pointer),
    ]);
    fork_signature.returns.push(AbiParam::new(types::I64));
    let fork = module
        .declare_function(
            "phoebe_rt_v1_fork_handle_i64",
            Linkage::Import,
            &fork_signature,
        )
        .map_err(|e| e.to_string())?;
    let mut spawn_signature = module.make_signature();
    spawn_signature.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
    ]);
    spawn_signature.returns.push(AbiParam::new(types::I64));
    let spawn = module
        .declare_function(
            "phoebe_rt_v1_spawn_poll_caps",
            Linkage::Import,
            &spawn_signature,
        )
        .map_err(|e| e.to_string())?;
    let reply_imports = runtime_imports::declare_reply_imports(module, pointer)?;
    let request_imports = runtime_imports::declare_request_imports(module, pointer)?;
    let mut signature = module.make_signature();
    signature
        .params
        .extend([AbiParam::new(pointer), AbiParam::new(pointer)]);
    signature.returns.push(AbiParam::new(types::I64));
    let mut ids = vec![None; program.functions.len()];
    for id in &reachable {
        let (symbol, linkage) = if *id == root {
            match export_symbol {
                Some(symbol) => (symbol.to_owned(), Linkage::Export),
                None => (format!("phoebe_poll_{}", id.0), Linkage::Local),
            }
        } else {
            (format!("phoebe_poll_{}", id.0), Linkage::Local)
        };
        ids[id.0] = Some(
            module
                .declare_function(&symbol, linkage, &signature)
                .map_err(|e| e.to_string())?,
        );
    }
    let entry = ids[root.0].expect("declared root poll");
    for id in reachable {
        let function = &program.functions[id.0];
        let layout = layouts[id.0].as_ref().expect("validated poll frame");
        let function_id = ids[id.0].expect("declared poll function");
        let mut ctx = module.make_context();
        ctx.func.signature = signature.clone();
        ctx.func.name = UserFuncName::user(0, function_id.as_u32());
        let mut builder_context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_context);
            let dispatch = builder.create_block();
            let first = builder.create_block();
            let fault = builder.create_block();
            let blocks = function
                .blocks
                .iter()
                .map(|_| builder.create_block())
                .collect::<Vec<_>>();
            let resume_blocks = layout
                .resumes
                .iter()
                .map(|_| builder.create_block())
                .collect::<Vec<_>>();
            let retry_blocks = layout
                .resumes
                .iter()
                .map(|_| builder.create_block())
                .collect::<Vec<_>>();
            builder.append_block_params_for_function_params(dispatch);
            let bridge = builder.block_params(dispatch)[0];
            let frame = builder.block_params(dispatch)[1];
            let variables = function
                .value_types
                .iter()
                .map(|ty| {
                    shapes
                        .types(*ty)
                        .into_iter()
                        .map(|component| builder.declare_var(component))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();

            builder.switch_to_block(dispatch);
            let places = function
                .place_types
                .iter()
                .enumerate()
                .map(|(index, ty)| {
                    let retained = layout.places.iter().find(|slot| slot.id.0 == index);
                    shapes
                        .types(*ty)
                        .into_iter()
                        .enumerate()
                        .map(|(part, component)| {
                            if let Some(slot) = retained {
                                Ok(builder.ins().iadd_imm_s(
                                    frame,
                                    i64::from(word_offset(slot.words.start + part)),
                                ))
                            } else {
                                let (size, align) = if component == types::I8 {
                                    (1, 0)
                                } else {
                                    (8, 3)
                                };
                                let slot = builder.create_sized_stack_slot(StackSlotData::new(
                                    StackSlotKind::ExplicitSlot,
                                    size,
                                    align,
                                ));
                                Ok(builder.ins().stack_addr(pointer, slot, 0))
                            }
                        })
                        .collect::<Result<Vec<_>, String>>()
                })
                .collect::<Result<Vec<_>, String>>()?;
            let pc = builder
                .ins()
                .load(types::I64, MemFlagsData::new(), frame, 0);
            let initial = builder.ins().icmp_imm_s(IntCC::Equal, pc, 0);
            let mut check = builder.create_block();
            builder.ins().brif(initial, first, &[], check, &[]);
            for (index, slot) in layout.resumes.iter().enumerate() {
                builder.switch_to_block(check);
                let valid = builder.ins().icmp_imm_s(IntCC::Equal, pc, slot.pc);
                let next = if index + 1 == layout.resumes.len() {
                    fault
                } else {
                    builder.create_block()
                };
                builder
                    .ins()
                    .brif(valid, resume_blocks[index], &[], next, &[]);
                check = next;
            }

            builder.switch_to_block(first);
            for slot in &layout.entry_params {
                load_slot(&mut builder, frame, slot, &variables, &shapes, function);
            }
            builder.ins().jump(blocks[0], &[]);

            for (index, point) in layout.resumes.iter().enumerate() {
                builder.switch_to_block(resume_blocks[index]);
                for value in &point.values {
                    let slot = layout
                        .values
                        .iter()
                        .find(|slot| slot.id == *value)
                        .expect("spill slot");
                    load_slot(&mut builder, frame, slot, &variables, &shapes, function);
                }
                if let Some(instruction_id) = point.instruction {
                    let result = function.blocks[point.block.0].instructions[instruction_id]
                        .result
                        .expect("verified call result");
                    let scratch = layout.call_result.as_ref().expect("call result buffer");
                    for (part, (var, ty)) in variables[result.0]
                        .iter()
                        .zip(shapes.types(function.value_types[result.0]))
                        .enumerate()
                    {
                        let raw = builder.ins().load(
                            types::I64,
                            MemFlagsData::new(),
                            frame,
                            word_offset(scratch.start + part),
                        );
                        let value = if ty == types::I8 {
                            builder.ins().ireduce(types::I8, raw)
                        } else {
                            raw
                        };
                        builder.def_var(*var, value);
                    }
                }
                if let Terminator::Yield { resume, .. } = function.blocks[point.block.0].terminator
                {
                    builder.ins().jump(blocks[resume.0], &[]);
                } else {
                    builder.ins().jump(retry_blocks[index], &[]);
                }
            }

            builder.switch_to_block(fault);
            let fault_code = builder.ins().iconst(types::I64, 255);
            builder.ins().return_(&[fault_code]);

            let receive_ref = module.declare_func_in_func(receive, builder.func);
            let send_ref = module.declare_func_in_func(send, builder.func);
            let yield_ref = module.declare_func_in_func(yield_function, builder.func);
            let scope_enter_ref = module.declare_func_in_func(scope_enter, builder.func);
            let scope_join_ref = module.declare_func_in_func(scope_join, builder.func);
            let close_ref = module.declare_func_in_func(close, builder.func);
            let call_ref = module.declare_func_in_func(call, builder.func);
            let create_ref = module.declare_func_in_func(create, builder.func);
            let fork_ref = module.declare_func_in_func(fork, builder.func);
            let spawn_ref = module.declare_func_in_func(spawn, builder.func);
            let reply_new_ref = module.declare_func_in_func(reply_imports.create, builder.func);
            let reply_close_ref = module.declare_func_in_func(reply_imports.close, builder.func);
            let reply_complete_ref =
                module.declare_func_in_func(reply_imports.complete, builder.func);
            let reply_receive_ref =
                module.declare_func_in_func(reply_imports.receive, builder.func);
            let request_new_ref = module.declare_func_in_func(request_imports.create, builder.func);
            let request_send_ref = module.declare_func_in_func(request_imports.send, builder.func);
            let request_receive_ref =
                module.declare_func_in_func(request_imports.receive, builder.func);
            for (block_id, block) in function.blocks.iter().enumerate() {
                builder.switch_to_block(blocks[block_id]);
                for (instruction_id, instruction) in block.instructions.iter().enumerate() {
                    let get = |id: core::ValueId, builder: &mut FunctionBuilder| {
                        variables[id.0]
                            .iter()
                            .map(|var| builder.use_var(*var))
                            .collect::<Vec<_>>()
                    };
                    if let Operation::Call(target, args) = &instruction.operation {
                        if !suspendable[target.0] {
                            let (direct_id, _) = direct_ids
                                .as_ref()
                                .expect("compiled direct helper graph")[target.0];
                            let target_ref = module.declare_func_in_func(direct_id, builder.func);
                            let mut operands = Vec::new();
                            if context_functions[target.0] {
                                operands.push(bridge);
                            }
                            for arg in args {
                                match arg {
                                    CallArg::Value(value) | CallArg::Ref(value, _) => {
                                        operands.extend(get(*value, &mut builder));
                                    }
                                    CallArg::Borrow(place, _) => {
                                        operands.push(places[place.0][0]);
                                    }
                                }
                            }
                            let result = instruction.result.expect("verified direct call result");
                            let returned = codegen::emit_direct_call(
                                &mut builder,
                                target_ref,
                                operands,
                                &shapes.types(program.functions[target.0].return_type),
                                pointer,
                                codegen::direct_register_results(module),
                            )?;
                            for (var, value) in variables[result.0].iter().zip(returned) {
                                builder.def_var(*var, value);
                            }
                            continue;
                        }
                        let point_index = layout
                            .resumes
                            .iter()
                            .position(|point| {
                                point.block.0 == block_id
                                    && point.instruction == Some(instruction_id)
                            })
                            .expect("validated retained call point");
                        let point = &layout.resumes[point_index];
                        let child_layout = layouts[target.0].as_ref().expect("poll callee frame");
                        let child_function = &program.functions[target.0];
                        let child_id = ids[target.0].expect("declared poll callee");
                        let child_ref = module.declare_func_in_func(child_id, builder.func);
                        let child_entry = builder.ins().func_addr(pointer, child_ref);
                        let (initial_ptr, initial_count) = emit_frame_initial(
                            &mut builder,
                            args,
                            child_layout,
                            &variables,
                            &places,
                            pointer,
                        )?;
                        for value in &point.values {
                            let slot = layout
                                .values
                                .iter()
                                .find(|slot| slot.id == *value)
                                .expect("call spill slot");
                            for (part, var) in variables[value.0].iter().enumerate() {
                                let raw = builder.use_var(*var);
                                let raw = if shapes.types(function.value_types[value.0])[part]
                                    == types::I8
                                {
                                    builder.ins().uextend(types::I64, raw)
                                } else {
                                    raw
                                };
                                builder.ins().store(
                                    MemFlagsData::new(),
                                    raw,
                                    frame,
                                    word_offset(slot.words.start + part),
                                );
                            }
                        }
                        let pc = builder.ins().iconst(types::I64, point.pc);
                        builder.ins().store(MemFlagsData::new(), pc, frame, 0);
                        let scratch = layout.call_result.as_ref().expect("call result buffer");
                        let frame_words = builder.ins().iconst(
                            types::I64,
                            i64::try_from(child_layout.total_words)
                                .map_err(|_| "child frame size overflow")?,
                        );
                        let initial_count = builder.ins().iconst(types::I64, initial_count);
                        let result_start = builder.ins().iconst(
                            types::I64,
                            i64::try_from(scratch.start)
                                .map_err(|_| "call result offset overflow")?,
                        );
                        let result_words = builder.ins().iconst(
                            types::I64,
                            i64::try_from(shapes.types(child_function.return_type).len())
                                .map_err(|_| "call result width overflow")?,
                        );
                        let call_inst = builder.ins().call(
                            call_ref,
                            &[
                                bridge,
                                child_entry,
                                frame_words,
                                initial_ptr,
                                initial_count,
                                result_start,
                                result_words,
                            ],
                        );
                        let status = builder.inst_results(call_inst)[0];
                        let accepted = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                        let queued = builder.create_block();
                        builder.ins().brif(accepted, queued, &[], fault, &[]);
                        builder.switch_to_block(queued);
                        let call_code = builder.ins().iconst(types::I64, 3);
                        builder.ins().return_(&[call_code]);
                        builder.switch_to_block(retry_blocks[point_index]);
                        continue;
                    }
                    let result = match &instruction.operation {
                        Operation::ScopeEnter(_) => {
                            let call = builder.ins().call(scope_enter_ref, &[bridge]);
                            let status = builder.inst_results(call)[0];
                            let succeeded = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                            let next = builder.create_block();
                            builder.ins().brif(succeeded, next, &[], fault, &[]);
                            builder.switch_to_block(next);
                            None
                        }
                        Operation::Const(value) => {
                            Some(vec![builder.ins().iconst(types::I64, *value)])
                        }
                        Operation::Bool(value) => {
                            Some(vec![builder.ins().iconst(types::I8, i64::from(*value))])
                        }
                        Operation::Move(value) | Operation::Copy(value) => {
                            Some(get(*value, &mut builder))
                        }
                        Operation::PlaceInit(place, value) | Operation::Store(place, value) => {
                            for (component, address) in
                                get(*value, &mut builder).into_iter().zip(&places[place.0])
                            {
                                builder
                                    .ins()
                                    .store(MemFlagsData::new(), component, *address, 0);
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
                                .map(|(ty, address)| {
                                    builder.ins().load(ty, MemFlagsData::new(), *address, 0)
                                })
                                .collect(),
                        ),
                        Operation::ReadField(place, index) => {
                            let Type::Data(data_id) = function.place_types[place.0] else {
                                unreachable!("verified place field owner")
                            };
                            let fields = &program.data[data_id.0].fields;
                            let offset = fields[..*index]
                                .iter()
                                .map(|(_, ty)| shapes.types(*ty).len())
                                .sum::<usize>();
                            let ty = shapes.types(fields[*index].1)[0];
                            Some(vec![builder.ins().load(
                                ty,
                                MemFlagsData::new(),
                                places[place.0][offset],
                                0,
                            )])
                        }
                        Operation::Variant(id, tag, payload) => {
                            let mut parts = vec![builder.ins().iconst(types::I64, *tag as i64)];
                            if let Some(value) = payload {
                                for (part, ty) in get(*value, &mut builder)
                                    .into_iter()
                                    .zip(shapes.types(function.value_types[value.0]))
                                {
                                    parts.push(if ty == types::I8 {
                                        builder.ins().uextend(types::I64, part)
                                    } else {
                                        part
                                    });
                                }
                            }
                            let width = shapes.types(Type::Data(*id)).len();
                            while parts.len() < width {
                                parts.push(builder.ins().iconst(types::I64, 0));
                            }
                            Some(parts)
                        }
                        Operation::Product(_, fields) => Some(
                            fields
                                .iter()
                                .flat_map(|field| get(*field, &mut builder))
                                .collect(),
                        ),
                        Operation::Field(owner, index) => {
                            let Type::Data(data_id) = function.value_types[owner.0] else {
                                unreachable!("verified field owner")
                            };
                            let fields = &program.data[data_id.0].fields;
                            let offset = fields[..*index]
                                .iter()
                                .map(|(_, ty)| shapes.types(*ty).len())
                                .sum::<usize>();
                            let width = shapes.types(fields[*index].1).len();
                            Some(get(*owner, &mut builder)[offset..offset + width].to_vec())
                        }
                        Operation::Neg(value) => {
                            let value = get(*value, &mut builder)[0];
                            Some(vec![builder.ins().ineg(value)])
                        }
                        Operation::Not(value) => {
                            let value = get(*value, &mut builder)[0];
                            Some(vec![builder.ins().icmp_imm_u(IntCC::Equal, value, 0)])
                        }
                        Operation::Add(a, b) | Operation::Sub(a, b) => {
                            let a = get(*a, &mut builder)[0];
                            let b = get(*b, &mut builder)[0];
                            Some(vec![
                                if matches!(instruction.operation, Operation::Add(_, _)) {
                                    builder.ins().iadd(a, b)
                                } else {
                                    builder.ins().isub(a, b)
                                },
                            ])
                        }
                        Operation::Mul(a, b) => {
                            let a = get(*a, &mut builder)[0];
                            let b = get(*b, &mut builder)[0];
                            Some(vec![builder.ins().imul(a, b)])
                        }
                        Operation::Eq(a, b)
                        | Operation::Ne(a, b)
                        | Operation::Lt(a, b)
                        | Operation::Le(a, b)
                        | Operation::Gt(a, b)
                        | Operation::Ge(a, b) => {
                            let a = get(*a, &mut builder)[0];
                            let b = get(*b, &mut builder)[0];
                            let cc = match instruction.operation {
                                Operation::Eq(_, _) => IntCC::Equal,
                                Operation::Ne(_, _) => IntCC::NotEqual,
                                Operation::Lt(_, _) => IntCC::SignedLessThan,
                                Operation::Le(_, _) => IntCC::SignedLessThanOrEqual,
                                Operation::Gt(_, _) => IntCC::SignedGreaterThan,
                                Operation::Ge(_, _) => IntCC::SignedGreaterThanOrEqual,
                                _ => unreachable!(),
                            };
                            Some(vec![builder.ins().icmp(cc, a, b)])
                        }
                        Operation::LoadRef(reference) => {
                            let address = get(*reference, &mut builder)[0];
                            Some(vec![builder.ins().load(
                                types::I64,
                                MemFlagsData::new(),
                                address,
                                0,
                            )])
                        }
                        Operation::StoreRef(reference, value) => {
                            let address = get(*reference, &mut builder)[0];
                            let value = get(*value, &mut builder)[0];
                            builder.ins().store(MemFlagsData::new(), value, address, 0);
                            None
                        }
                        Operation::CloseHandle(value) => {
                            let token = get(*value, &mut builder)[0];
                            let kind = match function.value_types[value.0] {
                                Type::SenderI64 | Type::RequestSenderI64 => 0,
                                Type::ReceiverI64 | Type::RequestReceiverI64 => 1,
                                _ => unreachable!(),
                            };
                            let kind = builder.ins().iconst(types::I64, kind);
                            builder.ins().call(close_ref, &[bridge, token, kind]);
                            None
                        }
                        Operation::CloseReply(value) => {
                            let token = get(*value, &mut builder)[0];
                            let kind =
                                i64::from(function.value_types[value.0] == Type::ReplyReceiverI64);
                            let kind = builder.ins().iconst(types::I64, kind);
                            let call = builder.ins().call(reply_close_ref, &[bridge, token, kind]);
                            let status = builder.inst_results(call)[0];
                            let valid = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                            let next = builder.create_block();
                            builder.ins().brif(valid, next, &[], fault, &[]);
                            builder.switch_to_block(next);
                            None
                        }
                        Operation::PackRequest { value, right } => Some(vec![
                            get(*value, &mut builder)[0],
                            get(*right, &mut builder)[0],
                        ]),
                        Operation::CloseRequest(value) => {
                            let right = get(*value, &mut builder)[1];
                            let kind = builder.ins().iconst(types::I64, 0);
                            let call = builder.ins().call(reply_close_ref, &[bridge, right, kind]);
                            let status = builder.inst_results(call)[0];
                            let valid = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                            let next = builder.create_block();
                            builder.ins().brif(valid, next, &[], fault, &[]);
                            builder.switch_to_block(next);
                            None
                        }
                        _ => unreachable!("validated poll operation"),
                    };
                    if let Some(id) = instruction.result {
                        for (var, value) in
                            variables[id.0].iter().zip(result.expect("result shape"))
                        {
                            builder.def_var(*var, value);
                        }
                    }
                }
                match &block.terminator {
                    Terminator::Return(value) => {
                        for (part, (var, ty)) in variables[value.0]
                            .iter()
                            .zip(shapes.types(function.return_type))
                            .enumerate()
                        {
                            let value = builder.use_var(*var);
                            let raw = if ty == types::I8 {
                                builder.ins().uextend(types::I64, value)
                            } else {
                                value
                            };
                            builder.ins().store(
                                MemFlagsData::new(),
                                raw,
                                frame,
                                word_offset(layout.result.start + part),
                            );
                        }
                        let ready = builder.ins().iconst(types::I64, 0);
                        builder.ins().return_(&[ready]);
                    }
                    Terminator::Jump(target, args) => {
                        let values = args
                            .iter()
                            .map(|id| {
                                variables[id.0]
                                    .iter()
                                    .map(|var| builder.use_var(*var))
                                    .collect::<Vec<_>>()
                            })
                            .collect::<Vec<_>>();
                        for (param, values) in function.blocks[target.0].params.iter().zip(values) {
                            for (var, value) in variables[param.0].iter().zip(values) {
                                builder.def_var(*var, value);
                            }
                        }
                        builder.ins().jump(blocks[target.0], &[]);
                    }
                    Terminator::Branch {
                        condition,
                        then_block,
                        else_block,
                    } => {
                        let condition = builder.use_var(variables[condition.0][0]);
                        builder.ins().brif(
                            condition,
                            blocks[then_block.0],
                            &[],
                            blocks[else_block.0],
                            &[],
                        );
                    }
                    Terminator::Match { subject, arms } => {
                        let Type::Data(data_id) = function.value_types[subject.0] else {
                            unreachable!()
                        };
                        let tag_var = variables[subject.0][0];
                        let parts = variables[subject.0]
                            .iter()
                            .map(|var| builder.use_var(*var))
                            .collect::<Vec<_>>();
                        for (tag, arm) in arms.iter().enumerate() {
                            let adapter = builder.create_block();
                            let next = if tag + 1 == arms.len() {
                                builder.ins().jump(adapter, &[]);
                                None
                            } else {
                                let subject_tag = builder.use_var(tag_var);
                                let equal =
                                    builder
                                        .ins()
                                        .icmp_imm_s(IntCC::Equal, subject_tag, tag as i64);
                                let next = builder.create_block();
                                builder.ins().brif(equal, adapter, &[], next, &[]);
                                Some(next)
                            };
                            builder.switch_to_block(adapter);
                            if let Some(param) = function.blocks[arm.target.0].params.first() {
                                let payload = match arm.binding {
                                    MatchBinding::Discard => unreachable!(),
                                    MatchBinding::Whole => parts.clone(),
                                    MatchBinding::Payload => {
                                        let ty = program.data[data_id.0].payloads[tag]
                                            .expect("verified payload");
                                        shapes
                                            .types(ty)
                                            .into_iter()
                                            .enumerate()
                                            .map(|(index, ty)| {
                                                if ty == types::I8 {
                                                    builder
                                                        .ins()
                                                        .ireduce(types::I8, parts[index + 1])
                                                } else {
                                                    parts[index + 1]
                                                }
                                            })
                                            .collect()
                                    }
                                };
                                for (var, value) in variables[param.0].iter().zip(payload) {
                                    builder.def_var(*var, value);
                                }
                            }
                            builder.ins().jump(blocks[arm.target.0], &[]);
                            if let Some(next) = next {
                                builder.switch_to_block(next);
                            }
                        }
                    }
                    Terminator::ChannelNew {
                        capacity,
                        success,
                        failure,
                    }
                    | Terminator::RequestChannelNew {
                        capacity,
                        success,
                        failure,
                    } => {
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
                        let sender_out = builder.ins().stack_addr(pointer, sender_slot, 0);
                        let receiver_out = builder.ins().stack_addr(pointer, receiver_slot, 0);
                        let capacity = builder.use_var(variables[capacity.0][0]);
                        let create =
                            if matches!(block.terminator, Terminator::RequestChannelNew { .. }) {
                                request_new_ref
                            } else {
                                create_ref
                            };
                        let created = builder
                            .ins()
                            .call(create, &[bridge, capacity, sender_out, receiver_out]);
                        let status = builder.inst_results(created)[0];
                        let valid = builder.ins().icmp_imm_s(IntCC::NotEqual, status, 255);
                        let check = builder.create_block();
                        builder.ins().brif(valid, check, &[], fault, &[]);
                        builder.switch_to_block(check);
                        let succeeded = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                        let created = builder.create_block();
                        let failed = builder.create_block();
                        builder.ins().brif(succeeded, created, &[], failed, &[]);
                        builder.switch_to_block(created);
                        let sender = builder
                            .ins()
                            .stack_load(pointer, types::I64, sender_slot, 0);
                        let receiver =
                            builder
                                .ins()
                                .stack_load(pointer, types::I64, receiver_slot, 0);
                        let params = &function.blocks[success.0].params;
                        builder.def_var(variables[params[0].0][0], sender);
                        builder.def_var(variables[params[1].0][0], receiver);
                        builder.ins().jump(blocks[success.0], &[]);
                        builder.switch_to_block(failed);
                        let error = function.blocks[failure.0].params[0];
                        builder.def_var(variables[error.0][0], status);
                        builder.ins().jump(blocks[failure.0], &[]);
                    }
                    Terminator::ReplyNew { success, failure } => {
                        let right_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let receiver_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let right_out = builder.ins().stack_addr(pointer, right_slot, 0);
                        let receiver_out = builder.ins().stack_addr(pointer, receiver_slot, 0);
                        let call = builder
                            .ins()
                            .call(reply_new_ref, &[bridge, right_out, receiver_out]);
                        let status = builder.inst_results(call)[0];
                        let valid = builder.ins().icmp_imm_s(IntCC::NotEqual, status, 255);
                        let check = builder.create_block();
                        builder.ins().brif(valid, check, &[], fault, &[]);
                        builder.switch_to_block(check);
                        let succeeded = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                        let created = builder.create_block();
                        let failed = builder.create_block();
                        builder.ins().brif(succeeded, created, &[], failed, &[]);
                        builder.switch_to_block(created);
                        let right = builder.ins().stack_load(pointer, types::I64, right_slot, 0);
                        let receiver =
                            builder
                                .ins()
                                .stack_load(pointer, types::I64, receiver_slot, 0);
                        let params = &function.blocks[success.0].params;
                        builder.def_var(variables[params[0].0][0], right);
                        builder.def_var(variables[params[1].0][0], receiver);
                        builder.ins().jump(blocks[success.0], &[]);
                        builder.switch_to_block(failed);
                        let error = function.blocks[failure.0].params[0];
                        builder.def_var(variables[error.0][0], status);
                        builder.ins().jump(blocks[failure.0], &[]);
                    }
                    Terminator::Fork {
                        source,
                        success,
                        failure,
                    } => {
                        let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let out = builder.ins().stack_addr(pointer, out_slot, 0);
                        let token = builder.use_var(variables[source.0][0]);
                        let kind = builder.ins().iconst(
                            types::I64,
                            i64::from(function.value_types[source.0] == Type::ReceiverI64),
                        );
                        let call = builder.ins().call(fork_ref, &[bridge, token, kind, out]);
                        let status = builder.inst_results(call)[0];
                        let valid = builder.ins().icmp_imm_s(IntCC::NotEqual, status, 255);
                        let check = builder.create_block();
                        builder.ins().brif(valid, check, &[], fault, &[]);
                        builder.switch_to_block(check);
                        let succeeded = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                        let created = builder.create_block();
                        let failed = builder.create_block();
                        builder.ins().brif(succeeded, created, &[], failed, &[]);
                        builder.switch_to_block(created);
                        let fresh = builder.ins().stack_load(pointer, types::I64, out_slot, 0);
                        let param = function.blocks[success.0].params[0];
                        builder.def_var(variables[param.0][0], fresh);
                        builder.ins().jump(blocks[success.0], &[]);
                        builder.switch_to_block(failed);
                        let error = function.blocks[failure.0].params[0];
                        builder.def_var(variables[error.0][0], status);
                        builder.ins().jump(blocks[failure.0], &[]);
                    }
                    Terminator::Spawn {
                        child,
                        args,
                        success,
                        failure,
                        ..
                    } => {
                        let child_layout = layouts[child.0]
                            .as_ref()
                            .or_else(|| direct_spawn[child.0].as_ref().map(|(_, layout)| layout))
                            .expect("compiled spawn child frame");
                        let child_id = ids[child.0]
                            .or_else(|| direct_spawn[child.0].as_ref().map(|(id, _)| *id))
                            .expect("declared spawn child");
                        let child_ref = module.declare_func_in_func(child_id, builder.func);
                        let child_entry = builder.ins().func_addr(pointer, child_ref);
                        let (initial_ptr, initial_count) = emit_frame_initial(
                            &mut builder,
                            &args.iter().copied().map(CallArg::Value).collect::<Vec<_>>(),
                            child_layout,
                            &variables,
                            &places,
                            pointer,
                        )?;
                        let transfers = args
                            .iter()
                            .filter_map(|arg| {
                                let kind = match function.value_types[arg.0] {
                                    Type::SenderI64
                                    | Type::ReceiverI64
                                    | Type::RequestSenderI64
                                    | Type::RequestReceiverI64 => 0,
                                    Type::ReplyToI64 => 1,
                                    Type::ReplyReceiverI64 => 2,
                                    Type::RequestI64 => 1,
                                    _ => return None,
                                };
                                let part =
                                    usize::from(function.value_types[arg.0] == Type::RequestI64);
                                Some((kind, builder.use_var(variables[arg.0][part])))
                            })
                            .collect::<Vec<_>>();
                        let transfer_count = i64::try_from(transfers.len())
                            .map_err(|_| "spawn transfer count overflow")?;
                        let transfer_ptr = if transfers.is_empty() {
                            builder.ins().iconst(pointer, 0)
                        } else {
                            let bytes = u32::try_from(
                                transfers
                                    .len()
                                    .checked_mul(16)
                                    .ok_or("spawn transfer overflow")?,
                            )
                            .map_err(|_| "spawn transfer list exceeds stack slot")?;
                            let slot = builder.create_sized_stack_slot(StackSlotData::new(
                                StackSlotKind::ExplicitSlot,
                                bytes,
                                3,
                            ));
                            for (index, (kind, token)) in transfers.iter().enumerate() {
                                let kind = builder.ins().iconst(types::I64, *kind);
                                builder.ins().stack_store(
                                    pointer,
                                    kind,
                                    slot,
                                    i32::try_from(index * 16)
                                        .map_err(|_| "spawn transfer offset overflow")?,
                                );
                                builder.ins().stack_store(
                                    pointer,
                                    *token,
                                    slot,
                                    i32::try_from(index * 16 + 8)
                                        .map_err(|_| "spawn transfer offset overflow")?,
                                );
                            }
                            builder.ins().stack_addr(pointer, slot, 0)
                        };
                        let frame_words = builder.ins().iconst(
                            types::I64,
                            i64::try_from(child_layout.total_words)
                                .map_err(|_| "spawn frame size overflow")?,
                        );
                        let initial_count = builder.ins().iconst(types::I64, initial_count);
                        let transfer_count = builder.ins().iconst(types::I64, transfer_count);
                        let spawned = builder.ins().call(
                            spawn_ref,
                            &[
                                bridge,
                                child_entry,
                                frame_words,
                                initial_ptr,
                                initial_count,
                                transfer_ptr,
                                transfer_count,
                            ],
                        );
                        let status = builder.inst_results(spawned)[0];
                        let valid = builder.ins().icmp_imm_s(IntCC::NotEqual, status, 255);
                        let check = builder.create_block();
                        builder.ins().brif(valid, check, &[], fault, &[]);
                        builder.switch_to_block(check);
                        let admitted = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                        let failed = builder.create_block();
                        builder
                            .ins()
                            .brif(admitted, blocks[success.0], &[], failed, &[]);
                        builder.switch_to_block(failed);
                        let error = function.blocks[failure.0].params[0];
                        builder.def_var(variables[error.0][0], status);
                        builder.ins().jump(blocks[failure.0], &[]);
                    }
                    Terminator::ReplyComplete {
                        right,
                        value,
                        delivered,
                        closed,
                    } => {
                        let right = builder.use_var(variables[right.0][0]);
                        let value = builder.use_var(variables[value.0][0]);
                        let call = builder
                            .ins()
                            .call(reply_complete_ref, &[bridge, right, value]);
                        let status = builder.inst_results(call)[0];
                        let delivered_status = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                        let check = builder.create_block();
                        builder
                            .ins()
                            .brif(delivered_status, blocks[delivered.0], &[], check, &[]);
                        builder.switch_to_block(check);
                        let closed_status = builder.ins().icmp_imm_s(IntCC::Equal, status, 2);
                        builder
                            .ins()
                            .brif(closed_status, blocks[closed.0], &[], fault, &[]);
                    }
                    Terminator::RequestUnpack { request, next } => {
                        let params = &function.blocks[next.0].params;
                        for (part, param) in params.iter().enumerate() {
                            let value = builder.use_var(variables[request.0][part]);
                            builder.def_var(variables[param.0][0], value);
                        }
                        builder.ins().jump(blocks[next.0], &[]);
                    }
                    Terminator::Yield { .. }
                    | Terminator::ScopeJoin { .. }
                    | Terminator::ChannelReceive { .. }
                    | Terminator::ChannelSend { .. }
                    | Terminator::ReplyReceive { .. }
                    | Terminator::RequestSend { .. }
                    | Terminator::RequestReceive { .. } => {
                        let index = layout
                            .resumes
                            .iter()
                            .position(|slot| slot.block.0 == block_id)
                            .expect("verified retry point");
                        builder.ins().jump(retry_blocks[index], &[]);
                    }
                }
            }

            for (index, point) in layout.resumes.iter().enumerate() {
                if point.instruction.is_some() {
                    continue;
                }
                builder.switch_to_block(retry_blocks[index]);
                let terminator = &function.blocks[point.block.0].terminator;
                let request_slots = if matches!(terminator, Terminator::RequestReceive { .. }) {
                    Some((
                        builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        )),
                        builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        )),
                    ))
                } else {
                    None
                };
                let status = match terminator {
                    Terminator::Yield { .. } => {
                        let call = builder.ins().call(yield_ref, &[bridge]);
                        builder.inst_results(call)[0]
                    }
                    Terminator::ScopeJoin { .. } => {
                        let call = builder.ins().call(scope_join_ref, &[bridge]);
                        builder.inst_results(call)[0]
                    }
                    Terminator::ChannelReceive { receiver, .. } => {
                        let token = builder.use_var(variables[receiver.0][0]);
                        let out = builder
                            .ins()
                            .iadd_imm_s(frame, word_offset(layout.result.start) as i64);
                        let call = builder.ins().call(receive_ref, &[bridge, token, out]);
                        builder.inst_results(call)[0]
                    }
                    Terminator::ReplyReceive { receiver, .. } => {
                        let token = builder.use_var(variables[receiver.0][0]);
                        let out = builder
                            .ins()
                            .iadd_imm_s(frame, word_offset(layout.result.start) as i64);
                        let call = builder.ins().call(reply_receive_ref, &[bridge, token, out]);
                        builder.inst_results(call)[0]
                    }
                    Terminator::RequestReceive { receiver, .. } => {
                        let token = builder.use_var(variables[receiver.0][0]);
                        let (value_slot, right_slot) = request_slots.expect("request output slots");
                        let value_out = builder.ins().stack_addr(pointer, value_slot, 0);
                        let right_out = builder.ins().stack_addr(pointer, right_slot, 0);
                        let call = builder
                            .ins()
                            .call(request_receive_ref, &[bridge, token, value_out, right_out]);
                        builder.inst_results(call)[0]
                    }
                    Terminator::ChannelSend {
                        sender, message, ..
                    } => {
                        let token = builder.use_var(variables[sender.0][0]);
                        let message = builder.use_var(variables[message.0][0]);
                        let call = builder.ins().call(send_ref, &[bridge, token, message]);
                        builder.inst_results(call)[0]
                    }
                    Terminator::RequestSend {
                        sender, request, ..
                    } => {
                        let token = builder.use_var(variables[sender.0][0]);
                        let value = builder.use_var(variables[request.0][0]);
                        let right = builder.use_var(variables[request.0][1]);
                        let call = builder
                            .ins()
                            .call(request_send_ref, &[bridge, token, value, right]);
                        builder.inst_results(call)[0]
                    }
                    _ => unreachable!("verified channel retry"),
                };
                let pending = builder.create_block();
                let completed = builder.create_block();
                let is_pending = builder.ins().icmp_imm_s(IntCC::Equal, status, 1);
                builder.ins().brif(is_pending, pending, &[], completed, &[]);
                builder.switch_to_block(pending);
                for value in &point.values {
                    let slot = layout
                        .values
                        .iter()
                        .find(|slot| slot.id == *value)
                        .expect("spill slot");
                    for (part, var) in variables[value.0].iter().enumerate() {
                        let component = builder.use_var(*var);
                        builder.ins().store(
                            MemFlagsData::new(),
                            component,
                            frame,
                            word_offset(slot.words.start + part),
                        );
                    }
                }
                let pc = builder.ins().iconst(types::I64, point.pc);
                builder.ins().store(MemFlagsData::new(), pc, frame, 0);
                let pending_code = builder.ins().iconst(types::I64, 1);
                builder.ins().return_(&[pending_code]);

                builder.switch_to_block(completed);
                if matches!(terminator, Terminator::Yield { .. }) {
                    builder.ins().jump(fault, &[]);
                    continue;
                }
                let is_success = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                let success = builder.create_block();
                let check_other = builder.create_block();
                builder
                    .ins()
                    .brif(is_success, success, &[], check_other, &[]);
                builder.switch_to_block(success);
                match terminator {
                    Terminator::ChannelReceive { value, .. }
                    | Terminator::ReplyReceive { value, .. } => {
                        let received = builder.ins().load(
                            types::I64,
                            MemFlagsData::new(),
                            frame,
                            word_offset(layout.result.start),
                        );
                        let param = function.blocks[value.0].params[0];
                        builder.def_var(variables[param.0][0], received);
                        builder.ins().jump(blocks[value.0], &[]);
                    }
                    Terminator::ChannelSend { sent, .. } => {
                        builder.ins().jump(blocks[sent.0], &[]);
                    }
                    Terminator::ScopeJoin { resume, .. } => {
                        builder.ins().jump(blocks[resume.0], &[]);
                    }
                    Terminator::RequestReceive { value, .. } => {
                        let (value_slot, right_slot) = request_slots.expect("request output slots");
                        let received = builder.ins().stack_load(pointer, types::I64, value_slot, 0);
                        let right = builder.ins().stack_load(pointer, types::I64, right_slot, 0);
                        let param = function.blocks[value.0].params[0];
                        builder.def_var(variables[param.0][0], received);
                        builder.def_var(variables[param.0][1], right);
                        builder.ins().jump(blocks[value.0], &[]);
                    }
                    Terminator::RequestSend { sent, .. } => {
                        builder.ins().jump(blocks[sent.0], &[]);
                    }
                    _ => unreachable!(),
                }
                builder.switch_to_block(check_other);
                let is_terminal = builder.ins().icmp_imm_s(IntCC::Equal, status, 2);
                let terminal = builder.create_block();
                builder.ins().brif(is_terminal, terminal, &[], fault, &[]);
                builder.switch_to_block(terminal);
                match terminator {
                    Terminator::ChannelReceive { end, .. }
                    | Terminator::ReplyReceive { end, .. }
                    | Terminator::RequestReceive { end, .. } => {
                        builder.ins().jump(blocks[end.0], &[]);
                    }
                    Terminator::ChannelSend {
                        message, closed, ..
                    } => {
                        if let Some(param) = function.blocks[closed.0].params.first() {
                            let message = builder.use_var(variables[message.0][0]);
                            builder.def_var(variables[param.0][0], message);
                        }
                        builder.ins().jump(blocks[closed.0], &[]);
                    }
                    Terminator::RequestSend {
                        request, closed, ..
                    } => {
                        let param = function.blocks[closed.0].params[0];
                        for (source, destination) in
                            variables[request.0].iter().zip(&variables[param.0])
                        {
                            let component = builder.use_var(*source);
                            builder.def_var(*destination, component);
                        }
                        builder.ins().jump(blocks[closed.0], &[]);
                    }
                    Terminator::ScopeJoin { .. } => {
                        builder.ins().jump(fault, &[]);
                    }
                    _ => unreachable!(),
                }
            }

            builder.seal_all_blocks();
            builder.finalize(module.target_config());
        }
        writeln!(
            clif,
            "; Phoebe poll function {}\n{}",
            function.name,
            ctx.func.display()
        )
        .map_err(|error| error.to_string())?;
        module
            .define_function(function_id, &mut ctx)
            .map_err(|e| e.to_string())?;
        module.clear_context(&mut ctx);
    }
    Ok((entry, root_layout, clif))
}

fn word_offset(word: usize) -> i32 {
    i32::try_from(word * 8).expect("checked frame address")
}

fn emit_frame_initial(
    builder: &mut FunctionBuilder<'_>,
    args: &[CallArg],
    child: &FrameWords,
    variables: &[Vec<Variable>],
    places: &[Vec<ClValue>],
    pointer: cranelift_codegen::ir::Type,
) -> Result<(ClValue, i64), String> {
    let mut initial = Vec::new();
    for (arg, slot) in args.iter().zip(&child.entry_params) {
        match arg {
            CallArg::Value(arg) | CallArg::Ref(arg, _) => {
                for (part, var) in variables[arg.0].iter().enumerate() {
                    let value = builder.use_var(*var);
                    let ty = builder.func.dfg.value_type(value);
                    let word = if ty == types::I8 {
                        builder.ins().uextend(types::I64, value)
                    } else {
                        value
                    };
                    initial.push((slot.words.start + part, word));
                }
            }
            CallArg::Borrow(place, _) => {
                initial.push((slot.words.start, places[place.0][0]));
            }
        }
    }
    let count = i64::try_from(initial.len()).map_err(|_| "frame argument count overflow")?;
    if initial.is_empty() {
        return Ok((builder.ins().iconst(pointer, 0), count));
    }
    let bytes = u32::try_from(
        initial
            .len()
            .checked_mul(16)
            .ok_or("frame arguments overflow")?,
    )
    .map_err(|_| "frame arguments exceed stack slot")?;
    let slot =
        builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, bytes, 3));
    for (index, (word, value)) in initial.iter().enumerate() {
        let word = builder.ins().iconst(
            types::I64,
            i64::try_from(*word).map_err(|_| "frame argument offset overflow")?,
        );
        builder.ins().stack_store(
            pointer,
            word,
            slot,
            i32::try_from(index * 16).map_err(|_| "frame argument offset overflow")?,
        );
        builder.ins().stack_store(
            pointer,
            *value,
            slot,
            i32::try_from(index * 16 + 8).map_err(|_| "frame argument offset overflow")?,
        );
    }
    Ok((builder.ins().stack_addr(pointer, slot, 0), count))
}

fn validate_poll_function(
    function: &core::Function,
    layout: &FrameWords,
    layouts: &[Option<FrameWords>],
) -> Result<(), String> {
    if layout.resumes.is_empty() {
        return Err("retained poll requires a suspension point".into());
    }
    for point in &layout.resumes {
        if let Some(index) = point.instruction {
            let Operation::Call(target, _) =
                &function.blocks[point.block.0].instructions[index].operation
            else {
                return Err("retained call point does not name a call".into());
            };
            if layouts[target.0].is_none() {
                return Err("retained call requires a poll callee".into());
            }
        } else if !matches!(
            function.blocks[point.block.0].terminator,
            Terminator::ChannelReceive { .. }
                | Terminator::ChannelSend { .. }
                | Terminator::ReplyReceive { .. }
                | Terminator::RequestReceive { .. }
                | Terminator::RequestSend { .. }
                | Terminator::Yield { .. }
                | Terminator::ScopeJoin { .. }
        ) {
            return Err("unsupported retained poll point".into());
        }
    }
    for (block_id, block) in function.blocks.iter().enumerate() {
        for (index, instruction) in block.instructions.iter().enumerate() {
            if !matches!(
                instruction.operation,
                Operation::Const(_)
                    | Operation::Bool(_)
                    | Operation::Move(_)
                    | Operation::Copy(_)
                    | Operation::PlaceInit(_, _)
                    | Operation::Load(_)
                    | Operation::Take(_)
                    | Operation::CopyPlace(_)
                    | Operation::Store(_, _)
                    | Operation::ReadField(_, _)
                    | Operation::Variant(_, _, _)
                    | Operation::Product(_, _)
                    | Operation::Field(_, _)
                    | Operation::Neg(_)
                    | Operation::Not(_)
                    | Operation::Add(_, _)
                    | Operation::Sub(_, _)
                    | Operation::Mul(_, _)
                    | Operation::Eq(_, _)
                    | Operation::Ne(_, _)
                    | Operation::Lt(_, _)
                    | Operation::Le(_, _)
                    | Operation::Gt(_, _)
                    | Operation::Ge(_, _)
                    | Operation::LoadRef(_)
                    | Operation::StoreRef(_, _)
                    | Operation::CloseHandle(_)
                    | Operation::CloseReply(_)
                    | Operation::PackRequest { .. }
                    | Operation::CloseRequest(_)
                    | Operation::Call(_, _)
                    | Operation::ScopeEnter(_)
            ) {
                return Err("operation is not yet supported by retained poll".into());
            }
            if let Operation::Call(target, _) = &instruction.operation
                && layouts[target.0].is_some()
                && !layout
                    .resumes
                    .iter()
                    .any(|point| point.block.0 == block_id && point.instruction == Some(index))
            {
                return Err("retained call has no continuation point".into());
            }
        }
        if !matches!(
            block.terminator,
            Terminator::Return(_)
                | Terminator::Jump(_, _)
                | Terminator::Branch { .. }
                | Terminator::Match { .. }
                | Terminator::ChannelNew { .. }
                | Terminator::ReplyNew { .. }
                | Terminator::Fork { .. }
                | Terminator::Spawn { .. }
                | Terminator::ChannelReceive { .. }
                | Terminator::ChannelSend { .. }
                | Terminator::ReplyComplete { .. }
                | Terminator::ReplyReceive { .. }
                | Terminator::RequestChannelNew { .. }
                | Terminator::RequestUnpack { .. }
                | Terminator::RequestSend { .. }
                | Terminator::RequestReceive { .. }
                | Terminator::Yield { .. }
                | Terminator::ScopeJoin { .. }
        ) || matches!(
            block.terminator,
            Terminator::ChannelReceive { .. }
                | Terminator::ChannelSend { .. }
                | Terminator::ReplyReceive { .. }
                | Terminator::RequestSend { .. }
                | Terminator::RequestReceive { .. }
                | Terminator::Yield { .. }
                | Terminator::ScopeJoin { .. }
        ) && !layout
            .resumes
            .iter()
            .any(|point| point.block.0 == block_id && point.instruction.is_none())
        {
            return Err("terminator is not yet supported by retained poll".into());
        }
    }
    Ok(())
}

fn load_slot(
    builder: &mut FunctionBuilder,
    frame: ClValue,
    slot: &frame_layout::WordSlot<core::ValueId>,
    variables: &[Vec<Variable>],
    shapes: &AbiShapes,
    function: &core::Function,
) {
    for (index, (var, ty)) in variables[slot.id.0]
        .iter()
        .zip(shapes.types(function.value_types[slot.id.0]))
        .enumerate()
    {
        let value = builder.ins().load(
            ty,
            MemFlagsData::new(),
            frame,
            word_offset(slot.words.start + index),
        );
        builder.def_var(*var, value);
    }
}

#[cfg(test)]
mod tests;
