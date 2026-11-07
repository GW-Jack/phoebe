//! Retained native code path for verified leaves with channel retries.
//! The accepted Core subset is deliberately explicit until call continuations
//! and task spawn have frame protocols of their own.

use super::*;
use cranelift_frontend::Variable;

pub struct PollLeafExecutable {
    module: JITModule,
    entry: FuncId,
    layout: FrameWords,
    clif: String,
}

impl PollLeafExecutable {
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

/// Compile a checked channel leaf, including exact per-point retry spills.
/// This is an executable stepping stone for the general retained backend.
pub fn compile_poll_leaf_jit(program: &Program, name: &str) -> Result<PollLeafExecutable, String> {
    core::verify(program)?;
    let (id, function) = program
        .function(name)
        .ok_or_else(|| format!("unknown function `{name}`"))?;
    if function.return_type != Type::I64
        || function
            .param_modes
            .iter()
            .any(|mode| *mode != ParamMode::Own)
        || !function.place_types.is_empty()
    {
        return Err(
            "channel poll leaf requires an I64 result, owned parameters, and no places".into(),
        );
    }
    let frames = frame_word_layouts(program)?;
    let layout = frames
        .into_iter()
        .find(|layout| layout.function == id)
        .ok_or("channel poll leaf has no suspension frame")?;
    if layout.resumes.is_empty() || layout.resumes.iter().any(|slot| slot.instruction.is_some()) {
        return Err("channel poll leaf requires only channel retry points".into());
    }
    for slot in &layout.resumes {
        if !matches!(
            function.blocks[slot.block.0].terminator,
            Terminator::ChannelReceive { .. } | Terminator::ChannelSend { .. }
        ) {
            return Err("channel poll leaf has a non-channel suspension".into());
        }
    }
    for (block_id, block) in function.blocks.iter().enumerate() {
        for instruction in &block.instructions {
            if !matches!(
                instruction.operation,
                Operation::Const(_)
                    | Operation::Bool(_)
                    | Operation::Move(_)
                    | Operation::Copy(_)
                    | Operation::Variant(_, _, _)
                    | Operation::Add(_, _)
                    | Operation::Sub(_, _)
                    | Operation::CloseHandle(_)
            ) {
                return Err("operation is not yet supported by channel poll leaf".into());
            }
        }
        if !matches!(
            block.terminator,
            Terminator::Return(_)
                | Terminator::Jump(_, _)
                | Terminator::Branch { .. }
                | Terminator::Match { .. }
                | Terminator::ChannelReceive { .. }
                | Terminator::ChannelSend { .. }
        ) || matches!(
            block.terminator,
            Terminator::ChannelReceive { .. } | Terminator::ChannelSend { .. }
        ) && !layout.resumes.iter().any(|slot| slot.block.0 == block_id)
        {
            return Err("terminator is not yet supported by channel poll leaf".into());
        }
    }

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
    jit.symbol(
        "phoebe_rt_v1_close_handle_i64",
        runtime::phoebe_close_handle_i64 as *const u8,
    );
    let mut module = JITModule::new(jit);
    let pointer = module.target_config().pointer_type();
    let shapes = AbiShapes::new(program, pointer);

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
    let mut signature = module.make_signature();
    signature
        .params
        .extend([AbiParam::new(pointer), AbiParam::new(pointer)]);
    signature.returns.push(AbiParam::new(types::I64));
    let entry = module
        .declare_function("phoebe_channel_poll_leaf", Linkage::Local, &signature)
        .map_err(|e| e.to_string())?;
    let mut ctx = module.make_context();
    ctx.func.signature = signature;
    ctx.func.name = UserFuncName::user(0, entry.as_u32());
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
            builder.ins().jump(retry_blocks[index], &[]);
        }

        builder.switch_to_block(fault);
        let fault_code = builder.ins().iconst(types::I64, 255);
        builder.ins().return_(&[fault_code]);

        let receive_ref = module.declare_func_in_func(receive, builder.func);
        let send_ref = module.declare_func_in_func(send, builder.func);
        let close_ref = module.declare_func_in_func(close, builder.func);
        for (block_id, block) in function.blocks.iter().enumerate() {
            builder.switch_to_block(blocks[block_id]);
            for instruction in &block.instructions {
                let get = |id: core::ValueId, builder: &mut FunctionBuilder| {
                    variables[id.0]
                        .iter()
                        .map(|var| builder.use_var(*var))
                        .collect::<Vec<_>>()
                };
                let result = match &instruction.operation {
                    Operation::Const(value) => Some(vec![builder.ins().iconst(types::I64, *value)]),
                    Operation::Bool(value) => {
                        Some(vec![builder.ins().iconst(types::I8, i64::from(*value))])
                    }
                    Operation::Move(value) | Operation::Copy(value) => {
                        Some(get(*value, &mut builder))
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
                        while parts.len() < shapes.data[id.0].len() {
                            parts.push(builder.ins().iconst(types::I64, 0));
                        }
                        Some(parts)
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
                    Operation::CloseHandle(value) => {
                        let token = get(*value, &mut builder)[0];
                        let kind = match function.value_types[value.0] {
                            Type::SenderI64 => 0,
                            Type::ReceiverI64 => 1,
                            _ => unreachable!(),
                        };
                        let kind = builder.ins().iconst(types::I64, kind);
                        builder.ins().call(close_ref, &[bridge, token, kind]);
                        None
                    }
                    _ => unreachable!("validated poll operation"),
                };
                if let Some(id) = instruction.result {
                    for (var, value) in variables[id.0].iter().zip(result.expect("result shape")) {
                        builder.def_var(*var, value);
                    }
                }
            }
            match &block.terminator {
                Terminator::Return(value) => {
                    let value = builder.use_var(variables[value.0][0]);
                    builder.ins().store(
                        MemFlagsData::new(),
                        value,
                        frame,
                        word_offset(layout.result.start),
                    );
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
                                                builder.ins().ireduce(types::I8, parts[index + 1])
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
                Terminator::ChannelReceive { .. } | Terminator::ChannelSend { .. } => {
                    let index = layout
                        .resumes
                        .iter()
                        .position(|slot| slot.block.0 == block_id)
                        .expect("verified retry point");
                    builder.ins().jump(retry_blocks[index], &[]);
                }
                _ => unreachable!("validated poll terminator"),
            }
        }

        for (index, point) in layout.resumes.iter().enumerate() {
            builder.switch_to_block(retry_blocks[index]);
            let terminator = &function.blocks[point.block.0].terminator;
            let status = match terminator {
                Terminator::ChannelReceive { receiver, .. } => {
                    let token = builder.use_var(variables[receiver.0][0]);
                    let out = builder
                        .ins()
                        .iadd_imm_s(frame, word_offset(layout.result.start) as i64);
                    let call = builder.ins().call(receive_ref, &[bridge, token, out]);
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
            let is_success = builder.ins().icmp_imm_s(IntCC::Equal, status, 0);
            let success = builder.create_block();
            let check_other = builder.create_block();
            builder
                .ins()
                .brif(is_success, success, &[], check_other, &[]);
            builder.switch_to_block(success);
            match terminator {
                Terminator::ChannelReceive { value, .. } => {
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
                _ => unreachable!(),
            }
            builder.switch_to_block(check_other);
            let is_terminal = builder.ins().icmp_imm_s(IntCC::Equal, status, 2);
            let terminal = builder.create_block();
            builder.ins().brif(is_terminal, terminal, &[], fault, &[]);
            builder.switch_to_block(terminal);
            match terminator {
                Terminator::ChannelReceive { end, .. } => {
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
                _ => unreachable!(),
            }
        }

        builder.seal_all_blocks();
        builder.finalize(module.target_config());
    }
    let clif = ctx.func.display().to_string();
    module
        .define_function(entry, &mut ctx)
        .map_err(|e| e.to_string())?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().map_err(|e| e.to_string())?;
    Ok(PollLeafExecutable {
        module,
        entry,
        layout,
        clif,
    })
}

fn word_offset(word: usize) -> i32 {
    i32::try_from(word * 8).expect("checked frame address")
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
mod tests {
    use super::*;
    use crate::runtime::{NativeFrameInit, NativePollTask, ReceiveResult, Receiver, Sender};
    use std::ffi::c_void;
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };

    const SOURCE: &str = r#"
fn wait(receiver: Receiver<I64>, seed: I64) -> I64 {
  val marker = seed + 1;
  val item = receiver.receive();
  receiver.close();
  when (item) {
    Receive<I64>.Value(value) -> { marker + value },
    Receive<I64>.End -> { marker }
  }
}
fn main() -> I64 = 0
"#;

    struct Capture {
        task: NativePollTask,
        answer: Arc<AtomicI64>,
    }

    impl Task for Capture {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            let result = self.task.poll(context)?;
            if let Poll::Ready(value) = result {
                self.answer.store(value, Ordering::SeqCst);
            }
            Ok(result)
        }
    }

    struct Host {
        entry: runtime::NativePollEntry,
        frame_words: usize,
        receiver_word: usize,
        seed_word: usize,
        answer: Arc<AtomicI64>,
        sender: Option<Sender>,
        phase: u8,
        close_without_value: bool,
    }

    impl Task for Host {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            if self.phase == 0 {
                let (sender, receiver) = context.channel(1)?;
                let token = receiver.token();
                let mut task = unsafe { NativePollTask::new(self.entry, self.frame_words) }
                    .map_err(|error| error.to_string())?;
                task.write_token(self.receiver_word, token)?;
                task.frame_mut()[self.seed_word] = 7;
                context
                    .spawn_owned(
                        Capture {
                            task,
                            answer: self.answer.clone(),
                        },
                        &[token],
                    )
                    .map_err(|error| error.message)?;
                self.sender = Some(sender);
                self.phase = 1;
                context.yield_now()?;
                return Ok(Poll::Pending);
            }
            if self.phase == 1 {
                if !self.close_without_value {
                    context.send(self.sender.as_ref().expect("sender"), 42)?;
                }
                context.close_sender(self.sender.take().expect("sender"))?;
                self.phase = 2;
            }
            match context.join()? {
                Poll::Pending => Ok(Poll::Pending),
                Poll::Ready(()) => Ok(Poll::Ready(self.answer.load(Ordering::SeqCst))),
            }
        }
    }

    #[test]
    fn generated_receive_leaf_parks_resumes_and_handles_end() {
        let program = core::lower(&crate::syntax::parse(SOURCE).unwrap()).unwrap();
        let executable = compile_poll_leaf_jit(&program, "wait").unwrap();
        let layout = executable.layout();
        assert_eq!(layout.entry_params.len(), 2);
        assert_eq!(layout.values.len(), 2);
        assert!(executable.clif().contains("call"));
        for (close_without_value, expected) in [(false, 50), (true, 8)] {
            let answer = Arc::new(AtomicI64::new(-1));
            let host = Host {
                entry: unsafe { executable.entry() },
                frame_words: layout.total_words,
                receiver_word: layout.entry_params[0].words.start,
                seed_word: layout.entry_params[1].words.start,
                answer,
                sender: None,
                phase: 0,
                close_without_value,
            };
            let mut scheduler = Scheduler::new(host, 2, 1, 0).unwrap();
            assert_eq!(scheduler.run().unwrap(), expected);
            assert!(scheduler.stats().parks >= 1);
        }
    }

    struct ProducerHost {
        entry: runtime::NativePollEntry,
        frame_words: usize,
        sender_word: usize,
        receiver: Option<Receiver>,
        received: Vec<i64>,
        phase: u8,
    }

    impl Task for ProducerHost {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            if self.phase == 0 {
                let (sender, receiver) = context.channel(1)?;
                let token = sender.token();
                let mut fault = false;
                let mut bridge = NativeCallContext {
                    context: context as *mut _ as *mut c_void,
                    fault: &mut fault,
                };
                let token_word = token.native_word()?;
                let initial = [NativeFrameInit {
                    word: self.sender_word as i64,
                    value: token_word,
                }];
                let transfers = [token_word];
                let code = unsafe {
                    runtime::phoebe_spawn_poll(
                        &mut bridge,
                        self.entry,
                        self.frame_words as i64,
                        initial.as_ptr(),
                        initial.len() as i64,
                        transfers.as_ptr(),
                        transfers.len() as i64,
                    )
                };
                if fault || code != 0 {
                    return Err(format!("native spawn failed: {code}"));
                }
                self.receiver = Some(receiver);
                self.phase = 1;
            }
            if self.phase == 1 {
                loop {
                    match context.receive(self.receiver.as_ref().expect("receiver"))? {
                        ReceiveResult::Value(value) => self.received.push(value),
                        ReceiveResult::Pending => return Ok(Poll::Pending),
                        ReceiveResult::End => {
                            context.close_receiver(self.receiver.take().expect("receiver"))?;
                            self.phase = 2;
                            break;
                        }
                    }
                }
            }
            match context.join()? {
                Poll::Pending => Ok(Poll::Pending),
                Poll::Ready(()) => {
                    assert_eq!(self.received, [20, 22]);
                    Ok(Poll::Ready(self.received.iter().sum()))
                }
            }
        }
    }

    #[test]
    fn generated_two_send_leaf_retries_after_one_slot_backpressure() {
        let program = core::lower(
            &crate::syntax::parse(include_str!("../../examples/channel_exchange.phoebe")).unwrap(),
        )
        .unwrap();
        let executable = compile_poll_leaf_jit(&program, "producer").unwrap();
        assert_eq!(executable.layout().resumes.len(), 2);
        let host = ProducerHost {
            entry: unsafe { executable.entry() },
            frame_words: executable.layout().total_words,
            sender_word: executable.layout().entry_params[0].words.start,
            receiver: None,
            received: Vec::new(),
            phase: 0,
        };
        let mut scheduler = Scheduler::new(host, 2, 1, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        assert!(scheduler.stats().parks >= 2);
        assert!(scheduler.stats().wakes >= 2);
        scheduler.verify().unwrap();
    }

    struct FailedSpawnHost {
        entry: runtime::NativePollEntry,
        frame_words: usize,
        sender_word: usize,
    }

    impl Task for FailedSpawnHost {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            let (sender, receiver) = context.channel(1)?;
            let token_word = sender.token().native_word()?;
            let initial = [NativeFrameInit {
                word: self.sender_word as i64,
                value: token_word,
            }];
            let transfers = [token_word];
            let mut fault = false;
            let mut bridge = NativeCallContext {
                context: context as *mut _ as *mut c_void,
                fault: &mut fault,
            };
            let code = unsafe {
                runtime::phoebe_spawn_poll(
                    &mut bridge,
                    self.entry,
                    self.frame_words as i64,
                    initial.as_ptr(),
                    initial.len() as i64,
                    transfers.as_ptr(),
                    transfers.len() as i64,
                )
            };
            assert_eq!(code, 5);
            assert!(!fault);
            context.close_sender(sender)?;
            context.close_receiver(receiver)?;
            Ok(Poll::Ready(0))
        }
    }

    #[test]
    fn native_spawn_limit_keeps_parent_rights() {
        let program = core::lower(
            &crate::syntax::parse(include_str!("../../examples/channel_exchange.phoebe")).unwrap(),
        )
        .unwrap();
        let executable = compile_poll_leaf_jit(&program, "producer").unwrap();
        let host = FailedSpawnHost {
            entry: unsafe { executable.entry() },
            frame_words: executable.layout().total_words,
            sender_word: executable.layout().entry_params[0].words.start,
        };
        let mut scheduler = Scheduler::new(host, 1, 1, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), 0);
        assert_eq!(scheduler.stats().tasks_created, 1);
        scheduler.verify().unwrap();
    }

    struct ClosedSendHost {
        entry: runtime::NativePollEntry,
        frame_words: usize,
        sender_word: usize,
        child_return: Arc<AtomicI64>,
        started: bool,
    }

    impl Task for ClosedSendHost {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            if !self.started {
                let (sender, receiver) = context.channel(1)?;
                let token = sender.token();
                let mut child = unsafe { NativePollTask::new(self.entry, self.frame_words) }
                    .map_err(|error| error.to_string())?;
                child.write_token(self.sender_word, token)?;
                context
                    .spawn_owned(
                        Capture {
                            task: child,
                            answer: self.child_return.clone(),
                        },
                        &[token],
                    )
                    .map_err(|error| error.message)?;
                context.close_receiver(receiver)?;
                self.started = true;
            }
            match context.join()? {
                Poll::Pending => Ok(Poll::Pending),
                Poll::Ready(()) => Ok(Poll::Ready(self.child_return.load(Ordering::SeqCst))),
            }
        }
    }

    #[test]
    fn generated_send_leaf_returns_the_original_message_when_closed() {
        let program = core::lower(
            &crate::syntax::parse(include_str!("../../examples/channel_exchange.phoebe")).unwrap(),
        )
        .unwrap();
        let executable = compile_poll_leaf_jit(&program, "producer").unwrap();
        let host = ClosedSendHost {
            entry: unsafe { executable.entry() },
            frame_words: executable.layout().total_words,
            sender_word: executable.layout().entry_params[0].words.start,
            child_return: Arc::new(AtomicI64::new(-1)),
            started: false,
        };
        let mut scheduler = Scheduler::new(host, 2, 1, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), 20);
        scheduler.verify().unwrap();
    }
}
