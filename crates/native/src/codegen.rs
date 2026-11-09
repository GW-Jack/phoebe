use super::*;
use std::borrow::Cow;
use std::collections::HashMap;

use super::runtime_imports::{
    declare_reply_imports, declare_request_imports, declare_typed_imports,
};

/// Phoebe currently flattens only integer and pointer-shaped return parts.
/// Cranelift's supported host conventions allow eight such return registers
/// on AArch64 and two on x86-64. Other hosts conservatively use two.
pub(super) fn direct_register_results<M: Module>(module: &M) -> usize {
    if module.isa().name() == "aarch64" {
        8
    } else {
        2
    }
}

pub(super) fn emit_direct_call(
    builder: &mut FunctionBuilder<'_>,
    callee: cranelift_codegen::ir::FuncRef,
    mut args: Vec<ClValue>,
    results: &[cranelift_codegen::ir::Type],
    pointer: cranelift_codegen::ir::Type,
    register_results: usize,
) -> Result<Vec<ClValue>, String> {
    if results.len() <= register_results {
        let call = builder.ins().call(callee, &args);
        return Ok(builder.inst_results(call).to_vec());
    }
    let bytes = results
        .len()
        .checked_mul(8)
        .and_then(|bytes| u32::try_from(bytes).ok())
        .ok_or("direct call result shape exceeds stack slot")?;
    let slot =
        builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, bytes, 3));
    args.push(builder.ins().stack_addr(pointer, slot, 0));
    builder.ins().call(callee, &args);
    results
        .iter()
        .enumerate()
        .map(|(part, ty)| {
            let offset =
                i32::try_from(part * 8).map_err(|_| "direct call result offset overflow")?;
            let raw = builder.ins().stack_load(pointer, types::I64, slot, offset);
            Ok(if *ty == types::I8 {
                builder.ins().ireduce(types::I8, raw)
            } else {
                raw
            })
        })
        .collect()
}

pub(super) struct AbiShapes {
    shapes: phoebe_abi::Shapes,
    pointer_type: cranelift_codegen::ir::Type,
}

impl AbiShapes {
    pub(super) fn new(
        program: &Program,
        pointer_type: cranelift_codegen::ir::Type,
    ) -> Result<Self, String> {
        Ok(Self {
            shapes: phoebe_abi::Shapes::new(program)?,
            pointer_type,
        })
    }

    pub(super) fn types(&self, ty: Type) -> Vec<cranelift_codegen::ir::Type> {
        self.shapes
            .parts(ty)
            .expect("checked Core type has an ABI shape")
            .iter()
            .map(|part| match part {
                phoebe_abi::Part::Byte => types::I8,
                phoebe_abi::Part::Word => types::I64,
                phoebe_abi::Part::Address => self.pointer_type,
            })
            .collect()
    }

    pub(super) fn field_offset(&self, root: Type, path: &[usize]) -> usize {
        self.shapes
            .field_path(root, path)
            .expect("verified Core field path")
            .1
    }

    pub(super) fn capability_parts(&self, ty: Type) -> Cow<'_, [phoebe_abi::CapabilityPart]> {
        self.shapes
            .capability_parts(ty)
            .expect("checked Core type has an ABI capability layout")
    }

    pub(super) fn close_plan(&self, ty: Type) -> phoebe_abi::ClosePlan {
        self.shapes
            .close_plan(ty)
            .expect("checked Core type has a structural close plan")
    }

    pub(super) fn message_descriptor(
        &self,
        message: core::MessageType,
    ) -> Result<phoebe_message::RuntimeMessageDescriptor, String> {
        phoebe_message::runtime_message_descriptor(
            &self.shapes.message_layout(message.value_type())?,
        )
    }

    /// Native spawn transfer descriptors use one runtime kind and token per
    /// capability leaf, including rights nested in products and requests.
    pub(super) fn append_transfer_descriptors<T>(
        &self,
        ty: Type,
        out: &mut Vec<(i64, T)>,
        mut part: impl FnMut(usize) -> T,
    ) {
        for leaf in self.capability_parts(ty).iter() {
            let kind = match leaf.kind {
                Type::SenderI64
                | Type::ReceiverI64
                | Type::RequestSenderI64
                | Type::RequestReceiverI64
                | Type::TypedSender(_)
                | Type::TypedReceiver(_) => 0,
                Type::ReplyToI64 => 1,
                Type::ReplyReceiverI64 => 2,
                _ => unreachable!("capability ABI leaf"),
            };
            out.push((kind, part(leaf.part)));
        }
    }
}

/// Physical word ranges for the retained functions, derived from the same
/// flattened shapes used by direct Cranelift calls and block arguments.
pub fn frame_word_layouts(program: &Program) -> Result<Vec<FrameWords>, String> {
    let frames = frame_layout::analyze(program)?;
    let shapes = AbiShapes::new(program, types::I64)?;
    frames
        .iter()
        .map(|frame| frame_layout::word_layout(program, frame, |ty| shapes.types(ty).len()))
        .collect()
}

pub(super) fn frame_word_layouts_linked(
    linked: &core::CheckedLinkedProgram,
) -> Result<Vec<FrameWords>, String> {
    let program = linked.program();
    let frames = frame_layout::analyze_linked(linked)?;
    let shapes = AbiShapes::new(program, types::I64)?;
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

pub(super) struct GenericTargets {
    pub calls: HashMap<(usize, usize, usize), FuncId>,
    pub suspendable: Vec<bool>,
    pub frame_layouts: Vec<FrameWords>,
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
    generics: Option<&GenericTargets>,
) -> Result<Vec<(FuncId, Signature)>, String> {
    let uses_runtime = context_functions
        .iter()
        .enumerate()
        .any(|(index, needed)| *needed && selected.is_none_or(|set| set[index]));
    let suspendable = if let Some(generics) = generics {
        generics.suspendable.clone()
    } else {
        frame_layout::suspendable_functions(program)?
    };
    let mut poll_layouts = vec![None; program.functions.len()];
    if program.functions.iter().any(|function| {
        function.blocks.iter().any(|block| {
            matches!(block.terminator, Terminator::Spawn { child, .. } if suspendable[child.0])
        })
    }) {
        let layouts = if let Some(generics) = generics {
            generics.frame_layouts.clone()
        } else {
            frame_word_layouts(program)?
        };
        for layout in layouts {
            let index = layout.function.0;
            poll_layouts[index] = Some(layout);
        }
    }
    let pointer_type = module.target_config().pointer_type();
    let register_results = direct_register_results(module);
    let shapes = AbiShapes::new(program, pointer_type)?;
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
        let mut fork_signature = module.make_signature();
        fork_signature.params.extend([
            AbiParam::new(pointer_type),
            AbiParam::new(types::I64),
            AbiParam::new(types::I64),
            AbiParam::new(pointer_type),
        ]);
        fork_signature.returns.push(AbiParam::new(types::I64));
        let fork = module
            .declare_function(
                "phoebe_rt_v1_fork_handle_i64",
                Linkage::Import,
                &fork_signature,
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
            .declare_function(
                "phoebe_rt_v1_spawn_poll_caps",
                Linkage::Import,
                &spawn_signature,
            )
            .map_err(|error| error.to_string())?;
        Some((create, close, spawn, fork))
    } else {
        None
    };
    let reply_imports = if uses_runtime {
        Some(declare_reply_imports(module, pointer_type)?)
    } else {
        None
    };
    let request_imports = if uses_runtime {
        Some(declare_request_imports(module, pointer_type)?)
    } else {
        None
    };
    let typed_imports = if uses_runtime {
        Some(declare_typed_imports(module, pointer_type)?)
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
        let result_types = shapes.types(function.return_type);
        if result_types.len() > register_results {
            signature.params.push(AbiParam::special(
                pointer_type,
                ArgumentPurpose::StructReturn,
            ));
        } else {
            signature
                .returns
                .extend(result_types.into_iter().map(AbiParam::new));
        }
        let symbol = direct_symbol(&function.name);
        let linkage = if suspendable[index] || selected.is_some_and(|set| !set[index]) {
            Linkage::Import
        } else if export_main && (selected.is_some() || function.name == "main") {
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
                            &spawn_poll_symbol(&program.functions[child.0].name),
                            if selected.is_some_and(|set| set[child.0]) {
                                Linkage::Local
                            } else {
                                Linkage::Import
                            },
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
                        &spawn_poll_symbol(&program.functions[child.0].name),
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
    let mut typed_plans = typed::PlanData::default();
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
            let return_address = (shapes.types(function.return_type).len() > register_results)
                .then(|| {
                    *builder
                        .block_params(blocks[0])
                        .last()
                        .expect("sret parameter")
                });
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
                    Terminator::Yield { resume, .. } | Terminator::ScopeJoin { resume, .. } => {
                        vec![resume.0]
                    }
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
                    }
                    | Terminator::TypedChannelNew {
                        success, failure, ..
                    } => vec![success.0, failure.0],
                    Terminator::ReplyNew { success, failure } => vec![success.0, failure.0],
                    Terminator::RequestChannelNew {
                        success, failure, ..
                    } => vec![success.0, failure.0],
                    Terminator::RequestUnpack { next, .. }
                    | Terminator::ProductUnpack { next, .. } => vec![next.0],
                    Terminator::RequestSend { sent, closed, .. } => vec![sent.0, closed.0],
                    Terminator::RequestReceive { value, end, .. } => vec![value.0, end.0],
                    Terminator::Fork {
                        success, failure, ..
                    } => vec![success.0, failure.0],
                    Terminator::ChannelSend { sent, closed, .. }
                    | Terminator::TypedSend { sent, closed, .. } => vec![sent.0, closed.0],
                    Terminator::ChannelReceive { value, end, .. }
                    | Terminator::TypedReceive { value, end, .. } => vec![value.0, end.0],
                    Terminator::ReplyComplete {
                        delivered, closed, ..
                    } => {
                        vec![delivered.0, closed.0]
                    }
                    Terminator::ReplyReceive { value, end, .. } => vec![value.0, end.0],
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
                for (instruction_index, instruction) in block.instructions.iter().enumerate() {
                    let get = |id: core::ValueId| {
                        values[id.0].clone().expect("verified value is defined")
                    };
                    let one = |id: core::ValueId| get(id)[0];
                    let value = match &instruction.operation {
                        Operation::ScopeEnter(_) => {
                            return Err("scope enter requires retained task code".into());
                        }
                        Operation::Const(value) => {
                            Some(vec![builder.ins().iconst(types::I64, *value)])
                        }
                        Operation::Bool(value) => {
                            Some(vec![builder.ins().iconst(types::I8, i64::from(*value))])
                        }
                        Operation::Unit => Some(Vec::new()),
                        Operation::ArrayI64(elements) => {
                            Some(elements.iter().map(|element| one(*element)).collect())
                        }
                        Operation::ArrayGet(array, index) => Some(vec![get(*array)[*index]]),
                        Operation::ArrayGetPlace(place, index) => {
                            Some(vec![builder.ins().stack_load(
                                pointer_type,
                                types::I64,
                                places[place.0][*index],
                                0,
                            )])
                        }
                        Operation::ArraySetPlace(place, index, value) => {
                            let component = one(*value);
                            builder.ins().stack_store(
                                pointer_type,
                                component,
                                places[place.0][*index],
                                0,
                            );
                            None
                        }
                        Operation::ArrayLen(array) => {
                            let Type::ArrayI64(len) = function.value_types[array.0] else {
                                unreachable!("verified array length")
                            };
                            Some(vec![builder.ins().iconst(types::I64, len as i64)])
                        }
                        Operation::Move(value)
                        | Operation::Copy(value)
                        | Operation::InferredCopy(value) => Some(get(*value)),
                        Operation::BorrowBegin(place, _) => Some(vec![builder.ins().stack_addr(
                            pointer_type,
                            places[place.0][0],
                            0,
                        )]),
                        Operation::BorrowBeginPath(place, path, _) => {
                            let offset = shapes.field_offset(function.place_types[place.0], path);
                            Some(vec![builder.ins().stack_addr(
                                pointer_type,
                                places[place.0][offset],
                                0,
                            )])
                        }
                        Operation::BorrowReborrow(parent, _) => Some(get(*parent)),
                        Operation::BorrowEnd(_) => None,
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
                            let words = shapes.types(Type::Data(*id)).len() - 1;
                            while components.len() < 1 + words {
                                components.push(builder.ins().iconst(types::I64, 0));
                            }
                            Some(components)
                        }
                        Operation::Product(_, fields) => {
                            Some(fields.iter().flat_map(|field| get(*field)).collect())
                        }
                        Operation::DestBegin(_) => None,
                        Operation::DestField(place, index, value)
                        | Operation::StoreField(place, index, value) => {
                            let Type::Data(id) = function.place_types[place.0] else {
                                unreachable!("verified destination type")
                            };
                            let fields = &program.data[id.0].fields;
                            let offset = fields[..*index]
                                .iter()
                                .map(|(_, ty)| shapes.types(*ty).len())
                                .sum::<usize>();
                            for (component, slot) in
                                get(*value).into_iter().zip(&places[place.0][offset..])
                            {
                                builder.ins().stack_store(pointer_type, component, *slot, 0);
                            }
                            None
                        }
                        Operation::DestPublish(place) => Some(
                            shapes
                                .types(function.place_types[place.0])
                                .into_iter()
                                .zip(&places[place.0])
                                .map(|(ty, slot)| {
                                    builder.ins().stack_load(pointer_type, ty, *slot, 0)
                                })
                                .collect(),
                        ),
                        Operation::DestTakeField(place, index)
                        | Operation::TakeField(place, index) => {
                            let Type::Data(id) = function.place_types[place.0] else {
                                unreachable!("verified destination type")
                            };
                            let fields = &program.data[id.0].fields;
                            let offset = fields[..*index]
                                .iter()
                                .map(|(_, ty)| shapes.types(*ty).len())
                                .sum::<usize>();
                            let mut taken = Vec::new();
                            for (ty, slot) in shapes
                                .types(fields[*index].1)
                                .into_iter()
                                .zip(&places[place.0][offset..])
                            {
                                taken.push(builder.ins().stack_load(pointer_type, ty, *slot, 0));
                                let empty = builder.ins().iconst(ty, 0);
                                builder.ins().stack_store(pointer_type, empty, *slot, 0);
                            }
                            Some(taken)
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
                            Some(
                                shapes
                                    .types(fields[*index].1)
                                    .into_iter()
                                    .enumerate()
                                    .map(|(part, ty)| {
                                        builder.ins().stack_load(
                                            pointer_type,
                                            ty,
                                            places[place.0][offset + part],
                                            0,
                                        )
                                    })
                                    .collect(),
                            )
                        }
                        Operation::Neg(a) => Some(vec![builder.ins().ineg(one(*a))]),
                        Operation::Not(a) => {
                            Some(vec![builder.ins().icmp_imm_u(IntCC::Equal, one(*a), 0)])
                        }
                        Operation::Add(a, b) => Some(vec![builder.ins().iadd(one(*a), one(*b))]),
                        Operation::Sub(a, b) => Some(vec![builder.ins().isub(one(*a), one(*b))]),
                        Operation::Mul(a, b) => Some(vec![builder.ins().imul(one(*a), one(*b))]),
                        Operation::Eq(a, b) => {
                            Some(vec![builder.ins().icmp(IntCC::Equal, one(*a), one(*b))])
                        }
                        Operation::Ne(a, b) => {
                            Some(vec![builder.ins().icmp(IntCC::NotEqual, one(*a), one(*b))])
                        }
                        Operation::Lt(a, b) => Some(vec![builder.ins().icmp(
                            IntCC::SignedLessThan,
                            one(*a),
                            one(*b),
                        )]),
                        Operation::Le(a, b) => Some(vec![builder.ins().icmp(
                            IntCC::SignedLessThanOrEqual,
                            one(*a),
                            one(*b),
                        )]),
                        Operation::Gt(a, b) => Some(vec![builder.ins().icmp(
                            IntCC::SignedGreaterThan,
                            one(*a),
                            one(*b),
                        )]),
                        Operation::Ge(a, b) => Some(vec![builder.ins().icmp(
                            IntCC::SignedGreaterThanOrEqual,
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
                                    CallArg::BorrowPath(place, path, _) => {
                                        let offset = shapes
                                            .field_offset(function.place_types[place.0], path);
                                        vec![builder.ins().stack_addr(
                                            pointer_type,
                                            places[place.0][offset],
                                            0,
                                        )]
                                    }
                                })
                                .collect::<Vec<_>>();
                            if context_functions[target.0] {
                                let context = runtime_context.expect("caller context propagation");
                                args.insert(0, context);
                            }
                            Some(emit_direct_call(
                                &mut builder,
                                callee,
                                args,
                                &shapes.types(program.functions[target.0].return_type),
                                pointer_type,
                                register_results,
                            )?)
                        }
                        Operation::GenericCall { args, .. } => {
                            let target = generics
                                .and_then(|generics| {
                                    generics.calls.get(&(index, block_id, instruction_index))
                                })
                                .ok_or("linked generic call has no planned native instance")?;
                            let callee = module.declare_func_in_func(*target, builder.func);
                            let arguments = args.iter().flat_map(|value| get(*value)).collect();
                            Some(emit_direct_call(
                                &mut builder,
                                callee,
                                arguments,
                                &shapes.types(function.value_types[instruction.result.unwrap().0]),
                                pointer_type,
                                register_results,
                            )?)
                        }
                        Operation::LoadRef(reference) => Some(vec![builder.ins().load(
                            if function.value_types[instruction.result.unwrap().0] == Type::Bool {
                                types::I8
                            } else {
                                types::I64
                            },
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
                            let (_, close, _, _) =
                                runtime_imports.expect("channel runtime imports");
                            let callee = module.declare_func_in_func(close, builder.func);
                            let kind = match function.value_types[value.0] {
                                Type::SenderI64 | Type::RequestSenderI64 | Type::TypedSender(_) => {
                                    0
                                }
                                Type::ReceiverI64
                                | Type::RequestReceiverI64
                                | Type::TypedReceiver(_) => 1,
                                _ => unreachable!("verified handle kind"),
                            };
                            let kind = builder.ins().iconst(types::I64, kind);
                            builder.ins().call(
                                callee,
                                &[runtime_context.expect("runtime context"), one(*value), kind],
                            );
                            None
                        }
                        Operation::CloseCaps(value) => {
                            let parts = get(*value);
                            let plan = shapes.close_plan(function.value_types[value.0]);
                            close::emit(&mut builder, &plan, &parts, &mut |builder, ty, token| {
                                let kind = match ty {
                                    Type::SenderI64
                                    | Type::RequestSenderI64
                                    | Type::TypedSender(_) => 0,
                                    Type::ReceiverI64
                                    | Type::RequestReceiverI64
                                    | Type::TypedReceiver(_) => 1,
                                    Type::ReplyToI64 => 0,
                                    Type::ReplyReceiverI64 => 1,
                                    _ => return Err("unsupported structural close leaf".into()),
                                };
                                let kind = builder.ins().iconst(types::I64, kind);
                                let close = match ty {
                                    Type::SenderI64
                                    | Type::ReceiverI64
                                    | Type::RequestSenderI64
                                    | Type::RequestReceiverI64
                                    | Type::TypedSender(_)
                                    | Type::TypedReceiver(_) => {
                                        runtime_imports.expect("channel runtime imports").1
                                    }
                                    _ => reply_imports.expect("reply runtime imports").close,
                                };
                                let callee = module.declare_func_in_func(close, builder.func);
                                builder.ins().call(
                                    callee,
                                    &[runtime_context.expect("runtime context"), token, kind],
                                );
                                Ok(())
                            })?;
                            None
                        }
                        Operation::CloseReply(value) => {
                            let imports = reply_imports.expect("reply runtime imports");
                            let callee = module.declare_func_in_func(imports.close, builder.func);
                            let kind =
                                i64::from(function.value_types[value.0] == Type::ReplyReceiverI64);
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
                        | Operation::CopyPlace(place)
                        | Operation::InferredCopyPlace(place) => Some(
                            shapes
                                .types(function.place_types[place.0])
                                .into_iter()
                                .zip(&places[place.0])
                                .map(|(ty, slot)| {
                                    builder.ins().stack_load(pointer_type, ty, *slot, 0)
                                })
                                .collect(),
                        ),
                        Operation::PackRequest { value, right } => {
                            Some(vec![one(*value), one(*right)])
                        }
                        Operation::CloseRequest(value) => {
                            let imports = reply_imports.expect("reply runtime imports");
                            let callee = module.declare_func_in_func(imports.close, builder.func);
                            let kind = builder.ins().iconst(types::I64, 0);
                            builder.ins().call(
                                callee,
                                &[
                                    runtime_context.expect("runtime context"),
                                    get(*value)[1],
                                    kind,
                                ],
                            );
                            None
                        }
                    };
                    if let Some(result) = instruction.result {
                        values[result.0] = value;
                    }
                }
                let get =
                    |id: core::ValueId| values[id.0].clone().expect("verified value is defined");
                match &block.terminator {
                    Terminator::TypedSend { .. } | Terminator::TypedReceive { .. } => {
                        return Err(
                            "native typed channel traffic requires retained task frames".into()
                        );
                    }
                    Terminator::Yield { .. } => {
                        return Err("native yield requires retained task frames".into());
                    }
                    Terminator::ScopeJoin { .. } => {
                        return Err("native scope join requires retained task frames".into());
                    }
                    Terminator::Return(value) => {
                        let returned = get(*value);
                        if let Some(address) = return_address {
                            for (part, (component, ty)) in returned
                                .into_iter()
                                .zip(shapes.types(function.return_type))
                                .enumerate()
                            {
                                let raw = if ty == types::I8 {
                                    builder.ins().uextend(types::I64, component)
                                } else {
                                    component
                                };
                                let offset = i32::try_from(part * 8)
                                    .map_err(|_| "direct result offset overflow")?;
                                builder
                                    .ins()
                                    .store(MemFlagsData::new(), raw, address, offset);
                            }
                            builder.ins().return_(&[]);
                        } else {
                            builder.ins().return_(&returned);
                        }
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
                        ..
                    } => {
                        let (create, _, _, _) = runtime_imports.expect("channel runtime imports");
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
                    Terminator::TypedChannelNew {
                        message,
                        capacity,
                        success,
                        failure,
                        ..
                    } => {
                        let imports = typed_imports.expect("typed runtime imports");
                        let callee = module.declare_func_in_func(imports.create, builder.func);
                        let descriptor = shapes.message_descriptor(*message)?;
                        let (plan, plan_len) =
                            typed_plans.pointer(module, &mut builder, pointer_type, &descriptor)?;
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
                        let sender_out = builder.ins().stack_addr(pointer_type, sender_slot, 0);
                        let receiver_out = builder.ins().stack_addr(pointer_type, receiver_slot, 0);
                        let type_id = builder.ins().iconst(types::I64, descriptor.type_id as i64);
                        let width = builder.ins().iconst(
                            types::I64,
                            i64::try_from(descriptor.width)
                                .map_err(|_| "typed message width exceeds native ABI")?,
                        );
                        let call = builder.ins().call(
                            callee,
                            &[
                                runtime_context.expect("runtime context"),
                                get(*capacity)[0],
                                type_id,
                                width,
                                plan,
                                plan_len,
                                sender_out,
                                receiver_out,
                            ],
                        );
                        let status = builder.inst_results(call)[0];
                        let succeeded = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                        let created = builder.create_block();
                        builder.ins().brif(
                            succeeded,
                            created,
                            &[],
                            blocks[failure.0],
                            &[status.into()],
                        );
                        builder.switch_to_block(created);
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
                    Terminator::ReplyNew { success, failure } => {
                        let imports = reply_imports.expect("reply runtime imports");
                        let callee = module.declare_func_in_func(imports.create, builder.func);
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
                        let right_out = builder.ins().stack_addr(pointer_type, right_slot, 0);
                        let receiver_out = builder.ins().stack_addr(pointer_type, receiver_slot, 0);
                        let call = builder.ins().call(
                            callee,
                            &[
                                runtime_context.expect("runtime context"),
                                right_out,
                                receiver_out,
                            ],
                        );
                        let status = builder.inst_results(call)[0];
                        let succeeded = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                        let created = builder.create_block();
                        builder.ins().brif(
                            succeeded,
                            created,
                            &[],
                            blocks[failure.0],
                            &[status.into()],
                        );
                        builder.switch_to_block(created);
                        let right =
                            builder
                                .ins()
                                .stack_load(pointer_type, types::I64, right_slot, 0);
                        let receiver =
                            builder
                                .ins()
                                .stack_load(pointer_type, types::I64, receiver_slot, 0);
                        builder
                            .ins()
                            .jump(blocks[success.0], &[right.into(), receiver.into()]);
                    }
                    Terminator::Fork {
                        source,
                        success,
                        failure,
                    } => {
                        let (_, _, _, fork) = runtime_imports.expect("fork runtime import");
                        let callee = module.declare_func_in_func(fork, builder.func);
                        let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let out = builder.ins().stack_addr(pointer_type, out_slot, 0);
                        let kind = builder.ins().iconst(
                            types::I64,
                            i64::from(matches!(
                                function.value_types[source.0],
                                Type::ReceiverI64
                                    | Type::RequestReceiverI64
                                    | Type::TypedReceiver(_)
                            )),
                        );
                        let call = builder.ins().call(
                            callee,
                            &[
                                runtime_context.expect("runtime context"),
                                get(*source)[0],
                                kind,
                                out,
                            ],
                        );
                        let status = builder.inst_results(call)[0];
                        let succeeded = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                        let success_load = builder.create_block();
                        builder.ins().brif(
                            succeeded,
                            success_load,
                            &[],
                            blocks[failure.0],
                            &[status.into()],
                        );
                        builder.switch_to_block(success_load);
                        let fresh = builder
                            .ins()
                            .stack_load(pointer_type, types::I64, out_slot, 0);
                        builder.ins().jump(blocks[success.0], &[fresh.into()]);
                    }
                    Terminator::Spawn {
                        child,
                        args,
                        success,
                        failure,
                        ..
                    } => {
                        let (_, _, spawn, _) = runtime_imports.expect("spawn runtime import");
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
                        let transfer_args = args
                            .iter()
                            .map(|arg| (function.value_types[arg.0], get(*arg)))
                            .collect::<Vec<_>>();
                        let (transfers_ptr, transfer_count) = super::transfer::emit(
                            &mut builder,
                            &shapes,
                            &transfer_args,
                            pointer_type,
                        )?;
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
                    Terminator::ReplyComplete {
                        right,
                        value,
                        delivered,
                        closed,
                    } => {
                        let imports = reply_imports.expect("reply runtime imports");
                        let callee = module.declare_func_in_func(imports.complete, builder.func);
                        let call = builder.ins().call(
                            callee,
                            &[
                                runtime_context.expect("runtime context"),
                                get(*right)[0],
                                get(*value)[0],
                            ],
                        );
                        let status = builder.inst_results(call)[0];
                        let succeeded = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                        builder.ins().brif(
                            succeeded,
                            blocks[delivered.0],
                            &[],
                            blocks[closed.0],
                            &[],
                        );
                    }
                    Terminator::ReplyReceive { .. } => {
                        return Err("native reply receive requires retained task frames".into());
                    }
                    Terminator::RequestChannelNew {
                        capacity,
                        success,
                        failure,
                    } => {
                        let imports = request_imports.expect("request runtime imports");
                        let callee = module.declare_func_in_func(imports.create, builder.func);
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
                        let sender_out = builder.ins().stack_addr(pointer_type, sender_slot, 0);
                        let receiver_out = builder.ins().stack_addr(pointer_type, receiver_slot, 0);
                        let call = builder.ins().call(
                            callee,
                            &[
                                runtime_context.expect("runtime context"),
                                get(*capacity)[0],
                                sender_out,
                                receiver_out,
                            ],
                        );
                        let status = builder.inst_results(call)[0];
                        let succeeded = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
                        let created = builder.create_block();
                        builder.ins().brif(
                            succeeded,
                            created,
                            &[],
                            blocks[failure.0],
                            &[status.into()],
                        );
                        builder.switch_to_block(created);
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
                    Terminator::RequestUnpack { request, next } => {
                        let parts = get(*request)
                            .into_iter()
                            .map(Into::into)
                            .collect::<Vec<_>>();
                        builder.ins().jump(blocks[next.0], &parts);
                    }
                    Terminator::ProductUnpack { product, next } => {
                        let parts = get(*product)
                            .into_iter()
                            .map(Into::into)
                            .collect::<Vec<_>>();
                        builder.ins().jump(blocks[next.0], &parts);
                    }
                    Terminator::RequestSend { .. } | Terminator::RequestReceive { .. } => {
                        return Err("native request traffic requires retained task frames".into());
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
            if function.return_type != Type::Unit {
                let result = builder.inst_results(call)[0];
                let offset = i32::try_from(layout.result.start * 8)
                    .map_err(|_| "spawn result offset overflow")?;
                builder
                    .ins()
                    .store(MemFlagsData::new(), result, frame, offset);
            }
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

#[cfg(test)]
mod abi_transfer_tests {
    use super::*;
    use phoebe_core::{DataDef, DataId};

    #[test]
    fn nested_spawn_descriptors_use_flattened_offsets_and_runtime_kinds() {
        let program = Program {
            data: vec![
                DataDef {
                    name: "Pair".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![
                        ("sender".into(), Type::SenderI64),
                        ("receiver".into(), Type::ReceiverI64),
                    ],
                },
                DataDef {
                    name: "Envelope".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![
                        ("number".into(), Type::I64),
                        ("pair".into(), Type::Data(DataId(0))),
                        ("request".into(), Type::RequestI64),
                        ("reply_receiver".into(), Type::ReplyReceiverI64),
                    ],
                },
            ],
            functions: vec![],
        };
        let shapes = AbiShapes::new(&program, types::I64).unwrap();
        let mut transfers = Vec::new();
        shapes.append_transfer_descriptors(Type::Data(DataId(1)), &mut transfers, |part| part);
        assert_eq!(transfers, vec![(0, 1), (0, 2), (1, 4), (2, 5)]);
        shapes.append_transfer_descriptors(Type::I64, &mut transfers, |_| unreachable!());
        assert_eq!(transfers.len(), 4);
    }
}
