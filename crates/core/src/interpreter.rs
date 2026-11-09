use super::*;
use phoebe_generic_core as generic_core;
use std::sync::OnceLock;

mod message;
#[cfg(test)]
mod typed_channels;

fn eval_generic(
    module: &CheckedModule,
    callee: usize,
    args: Vec<Value>,
    depth: usize,
) -> Result<Value, String> {
    if depth >= 1024 {
        return Err("generic call depth exceeded".into());
    }
    let function = &module.as_module().functions[callee];
    let mut values = vec![None; function.value_types.len()];
    let mut block = 0;
    let mut incoming = args;
    loop {
        let current = &function.blocks[block];
        for (param, value) in current.params.iter().zip(incoming.drain(..)) {
            values[param.0] = Some(value);
        }
        for instruction in &current.instructions {
            let result = {
                let get = |id: generic_core::ValueId| {
                    values[id.0]
                        .as_ref()
                        .expect("verified generic value")
                        .clone()
                };
                match &instruction.operation {
                    generic_core::Operation::Move(value)
                    | generic_core::Operation::CopyScalar(value) => get(*value),
                    generic_core::Operation::Call { callee, args, .. } => eval_generic(
                        module,
                        *callee,
                        args.iter().map(|value| get(*value)).collect(),
                        depth + 1,
                    )?,
                }
            };
            values[instruction.result.0] = Some(result);
        }
        let get = |id: generic_core::ValueId| {
            values[id.0]
                .as_ref()
                .expect("verified generic value")
                .clone()
        };
        match &current.terminator {
            generic_core::Terminator::Return(value) => return Ok(get(*value)),
            generic_core::Terminator::Jump { target, args } => {
                incoming = args.iter().map(|value| get(*value)).collect();
                block = target.0;
            }
            generic_core::Terminator::Branch {
                condition,
                when_true,
                true_args,
                when_false,
                false_args,
            } => {
                let (target, args) = if as_bool(get(*condition)) {
                    (when_true, true_args)
                } else {
                    (when_false, false_args)
                };
                incoming = args.iter().map(|value| get(*value)).collect();
                block = target.0;
            }
        }
    }
}

fn close_caps(value: Value, context: &mut Option<&mut Context<'_>>) -> Result<(), String> {
    match value {
        Value::Product(_, fields) => {
            for field in fields.into_iter().rev() {
                close_caps(field, context)?;
            }
        }
        Value::Data(_, _, Some(payload)) => close_caps(*payload, context)?,
        Value::Sender(token) => context
            .as_deref_mut()
            .expect("capability close requires runtime context")
            .close_sender(runtime::Sender::from_token(token))?,
        Value::Receiver(token) => context
            .as_deref_mut()
            .expect("capability close requires runtime context")
            .close_receiver(runtime::Receiver::from_token(token))?,
        Value::ReplyTo(token) => context
            .as_deref_mut()
            .expect("capability close requires runtime context")
            .close_reply_to(runtime::ReplyTo::from_token(token))?,
        Value::ReplyReceiver(token) => context
            .as_deref_mut()
            .expect("capability close requires runtime context")
            .cancel_reply(runtime::ReplyReceiver::from_token(token))?,
        Value::Request(_, token) => context
            .as_deref_mut()
            .expect("capability close requires runtime context")
            .close_reply_to(runtime::ReplyTo::from_token(token))?,
        _ => {}
    }
    Ok(())
}

fn collect_capability_transfers(value: &Value, out: &mut Vec<runtime::CapabilityTransfer>) {
    match value {
        Value::Product(_, fields) => {
            for field in fields {
                collect_capability_transfers(field, out);
            }
        }
        Value::Data(_, _, Some(payload)) => collect_capability_transfers(payload, out),
        Value::Sender(token) | Value::Receiver(token) => {
            out.push(runtime::CapabilityTransfer::Channel(*token));
        }
        Value::ReplyTo(token) | Value::Request(_, token) => {
            out.push(runtime::CapabilityTransfer::ReplyRight(*token));
        }
        Value::ReplyReceiver(token) => {
            out.push(runtime::CapabilityTransfer::ReplyReceiver(*token));
        }
        _ => {}
    }
}

impl Program {
    pub fn function(&self, name: &str) -> Option<(FunctionId, &Function)> {
        self.functions
            .iter()
            .enumerate()
            .find(|(_, function)| function.name == name)
            .map(|(index, function)| (FunctionId(index), function))
    }

    /// A runtime pointer is needed only by functions that perform a runtime
    /// operation and their transitive callers. Value types alone do not force
    /// a runtime dependency.
    pub fn runtime_context_functions(&self) -> Vec<bool> {
        let capability_layout = verification::capability_layout::Layout::new(&self.data);
        let mut callers = vec![Vec::new(); self.functions.len()];
        let mut needed = vec![false; self.functions.len()];
        let mut ready = VecDeque::new();
        for (caller, function) in self.functions.iter().enumerate() {
            for block in &function.blocks {
                if !needed[caller]
                    && (matches!(
                        block.terminator,
                        Terminator::ChannelNew { .. }
                            | Terminator::TypedChannelNew { .. }
                            | Terminator::ReplyNew { .. }
                            | Terminator::RequestChannelNew { .. }
                            | Terminator::RequestSend { .. }
                            | Terminator::RequestReceive { .. }
                            | Terminator::Fork { .. }
                            | Terminator::Yield { .. }
                            | Terminator::ScopeJoin { .. }
                            | Terminator::ChannelSend { .. }
                            | Terminator::TypedSend { .. }
                            | Terminator::ChannelReceive { .. }
                            | Terminator::TypedReceive { .. }
                            | Terminator::ReplyComplete { .. }
                            | Terminator::ReplyReceive { .. }
                            | Terminator::Spawn { .. }
                    ) || block.instructions.iter().any(|instruction| {
                        matches!(
                            instruction.operation,
                            Operation::CloseHandle(_)
                                | Operation::CloseReply(_)
                                | Operation::CloseRequest(_)
                                | Operation::ScopeEnter(_)
                        ) || matches!(
                            instruction.operation,
                            Operation::CloseCaps(value)
                                if capability_layout.leaves(function.value_types[value.0]) > 0
                        )
                    }))
                {
                    needed[caller] = true;
                    ready.push_back(caller);
                }
                for instruction in &block.instructions {
                    if let Operation::Call(target, _) = instruction.operation {
                        callers[target.0].push(caller);
                    }
                }
            }
        }
        while let Some(callee) = ready.pop_front() {
            for &caller in &callers[callee] {
                if !needed[caller] {
                    needed[caller] = true;
                    ready.push_back(caller);
                }
            }
        }
        needed
    }

    pub fn interpret(&self, name: &str, args: &[i64]) -> Result<i64, String> {
        self.interpret_with_limits(name, args, ScopeLimits::default())
    }

    pub fn interpret_with_limits(
        &self,
        name: &str,
        args: &[i64],
        limits: ScopeLimits,
    ) -> Result<i64, String> {
        verify(self)?;
        self.interpret_verified(name, args, limits, None)
    }

    fn interpret_verified(
        &self,
        name: &str,
        args: &[i64],
        limits: ScopeLimits,
        generics: Option<Arc<CheckedModule>>,
    ) -> Result<i64, String> {
        let (id, function) = self
            .function(name)
            .ok_or_else(|| format!("unknown function {name}"))?;
        if function
            .param_modes
            .iter()
            .any(|mode| *mode != ParamMode::Own)
        {
            return Err("borrowed parameters require a Phoebe caller".into());
        }
        if function.return_type != Type::I64
            || function.blocks[0]
                .params
                .iter()
                .any(|param| function.value_types[param.0] != Type::I64)
        {
            return Err("external interpreter entry requires an I64 signature".into());
        }
        let args = args
            .iter()
            .map(|value| Value::I64(*value))
            .collect::<Vec<_>>();
        let root = CoreTask::new(Arc::new(self.clone()), generics, id, args);
        if self.runtime_context_functions()[id.0] {
            let mut scheduler = Scheduler::with_limits(root, limits)?;
            scheduler.run()
        } else {
            let mut root = root;
            match root.poll_step(None)? {
                Poll::Ready(value) => Ok(value),
                Poll::Pending => unreachable!("pure Core cannot suspend"),
            }
        }
    }
}

impl CheckedProgram {
    pub fn interpret(&self, name: &str, args: &[i64]) -> Result<i64, String> {
        self.interpret_with_limits(name, args, ScopeLimits::default())
    }

    pub fn interpret_with_limits(
        &self,
        name: &str,
        args: &[i64],
        limits: ScopeLimits,
    ) -> Result<i64, String> {
        self.0.interpret_verified(name, args, limits, None)
    }
}

impl CheckedLinkedProgram {
    pub fn interpret(&self, name: &str, args: &[i64]) -> Result<i64, String> {
        self.interpret_with_limits(name, args, ScopeLimits::default())
    }

    pub fn interpret_with_limits(
        &self,
        name: &str,
        args: &[i64],
        limits: ScopeLimits,
    ) -> Result<i64, String> {
        self.program
            .interpret_verified(name, args, limits, Some(Arc::new(self.generics.clone())))
    }
}

struct CoreTask {
    program: Arc<Program>,
    generics: Option<Arc<CheckedModule>>,
    message_catalog: OnceLock<phoebe_message::MessageCatalog>,
    request_descriptor: OnceLock<phoebe_message::RuntimeMessageDescriptor>,
    typed_descriptors: HashMap<MessageType, phoebe_message::RuntimeMessageDescriptor>,
    frames: Vec<InterpreterFrame>,
    stores: Vec<Option<Value>>,
}

struct InterpreterFrame {
    function: FunctionId,
    values: Vec<Option<Value>>,
    base: usize,
    block: usize,
    instruction: usize,
    return_to: Option<ValueId>,
}

impl CoreTask {
    fn new(
        program: Arc<Program>,
        generics: Option<Arc<CheckedModule>>,
        entry: FunctionId,
        args: Vec<Value>,
    ) -> Self {
        let mut task = Self {
            program,
            generics,
            message_catalog: OnceLock::new(),
            request_descriptor: OnceLock::new(),
            typed_descriptors: HashMap::new(),
            frames: Vec::new(),
            stores: Vec::new(),
        };
        task.push_frame(entry, args, None);
        task
    }

    fn message_catalog(&self) -> &phoebe_message::MessageCatalog {
        self.message_catalog.get_or_init(|| {
            phoebe_message::MessageCatalog::new(self.program.as_ref())
                .expect("verified Core has finite message layouts")
        })
    }

    fn request_descriptor(&self) -> &phoebe_message::RuntimeMessageDescriptor {
        self.request_descriptor.get_or_init(|| {
            self.message_catalog()
                .descriptor(Type::RequestI64)
                .expect("verified request has a runtime message descriptor")
        })
    }

    fn typed_descriptor(
        &mut self,
        message: MessageType,
    ) -> Result<&phoebe_message::RuntimeMessageDescriptor, String> {
        if !self.typed_descriptors.contains_key(&message) {
            let descriptor = self.message_catalog().descriptor(message.value_type())?;
            self.typed_descriptors.insert(message, descriptor);
        }
        Ok(self
            .typed_descriptors
            .get(&message)
            .expect("inserted typed descriptor"))
    }

    fn push_frame(&mut self, function: FunctionId, args: Vec<Value>, return_to: Option<ValueId>) {
        let signature = &self.program.functions[function.0];
        let mut values = vec![None; signature.value_types.len()];
        for (param, arg) in signature.blocks[0].params.iter().zip(args) {
            values[param.0] = Some(arg);
        }
        let base = self.stores.len();
        self.stores.resize(base + signature.place_types.len(), None);
        self.frames.push(InterpreterFrame {
            function,
            values,
            base,
            block: 0,
            instruction: 0,
            return_to,
        });
    }

    fn value(&self, id: ValueId) -> Value {
        self.frames.last().expect("active frame").values[id.0]
            .clone()
            .expect("verified value is defined")
    }

    fn enter(&mut self, target: BlockId, args: Vec<Value>) {
        let function = self.frames.last().expect("active frame").function;
        let frame = self.frames.last_mut().expect("active frame");
        for (param, arg) in self.program.functions[function.0].blocks[target.0]
            .params
            .iter()
            .zip(args)
        {
            frame.values[param.0] = Some(arg);
        }
        frame.block = target.0;
        frame.instruction = 0;
    }

    fn eval_instruction(
        &mut self,
        instruction: &Instruction,
        mut context: Option<&mut Context<'_>>,
    ) -> Result<Option<Value>, String> {
        let get = |id| self.value(id);
        let base = self.frames.last().expect("active frame").base;
        Ok(match &instruction.operation {
            Operation::Const(x) => Some(Value::I64(*x)),
            Operation::Bool(x) => Some(Value::Bool(*x)),
            Operation::Unit => Some(Value::Unit),
            Operation::Move(value) | Operation::Copy(value) => Some(get(*value)),
            Operation::Variant(id, tag, payload) => Some(Value::Data(
                *id,
                *tag,
                payload.map(|value| Box::new(get(value))),
            )),
            Operation::Product(id, fields) => Some(Value::Product(
                *id,
                fields.iter().map(|field| get(*field)).collect(),
            )),
            Operation::Field(owner, index) => {
                let Value::Product(_, fields) = get(*owner) else {
                    unreachable!("verified product")
                };
                Some(fields[*index].clone())
            }
            Operation::ReadField(place, index) => {
                let Some(Value::Product(_, fields)) = &self.stores[base + place.0] else {
                    unreachable!("verified product place")
                };
                Some(fields[*index].clone())
            }
            Operation::Neg(a) => Some(Value::I64(as_i64(get(*a)).wrapping_neg())),
            Operation::Not(a) => Some(Value::Bool(!as_bool(get(*a)))),
            Operation::Add(a, b) => Some(Value::I64(as_i64(get(*a)).wrapping_add(as_i64(get(*b))))),
            Operation::Sub(a, b) => Some(Value::I64(as_i64(get(*a)).wrapping_sub(as_i64(get(*b))))),
            Operation::Mul(a, b) => Some(Value::I64(as_i64(get(*a)).wrapping_mul(as_i64(get(*b))))),
            Operation::Eq(a, b) => Some(Value::Bool(get(*a) == get(*b))),
            Operation::Ne(a, b) => Some(Value::Bool(get(*a) != get(*b))),
            Operation::Lt(a, b) => Some(Value::Bool(as_i64(get(*a)) < as_i64(get(*b)))),
            Operation::Le(a, b) => Some(Value::Bool(as_i64(get(*a)) <= as_i64(get(*b)))),
            Operation::Gt(a, b) => Some(Value::Bool(as_i64(get(*a)) > as_i64(get(*b)))),
            Operation::Ge(a, b) => Some(Value::Bool(as_i64(get(*a)) >= as_i64(get(*b)))),
            Operation::Call(_, _) => unreachable!("calls push a retained frame"),
            Operation::GenericCall { callee, args, .. } => Some(eval_generic(
                self.generics.as_deref().ok_or("unlinked generic call")?,
                *callee,
                args.iter().map(|value| get(*value)).collect(),
                0,
            )?),
            Operation::PlaceInit(place, value) | Operation::Store(place, value) => {
                let value = get(*value);
                self.stores[base + place.0] = Some(value);
                None
            }
            Operation::Load(place) | Operation::CopyPlace(place) => {
                Some(self.stores[base + place.0].clone().expect("verified place"))
            }
            Operation::Take(place) => Some(
                self.stores[base + place.0]
                    .take()
                    .expect("verified owned place"),
            ),
            Operation::BorrowBegin(place, _) => Some(Value::Ref(base + place.0)),
            Operation::BorrowReborrow(parent, _) => Some(get(*parent)),
            Operation::BorrowEnd(_) => None,
            Operation::LoadRef(reference) => {
                let Value::Ref(slot) = get(*reference) else {
                    unreachable!("verified reference")
                };
                Some(self.stores[slot].clone().expect("verified borrowed place"))
            }
            Operation::StoreRef(reference, value) => {
                let Value::Ref(slot) = get(*reference) else {
                    unreachable!("verified reference")
                };
                let value = get(*value);
                self.stores[slot] = Some(value);
                None
            }
            Operation::CloseHandle(value) => {
                let context = context.expect("runtime operation has a context");
                match get(*value) {
                    Value::Sender(token) => {
                        context.close_sender(runtime::Sender::from_token(token))?
                    }
                    Value::Receiver(token) => {
                        context.close_receiver(runtime::Receiver::from_token(token))?
                    }
                    _ => unreachable!("verified channel endpoint"),
                }
                None
            }
            Operation::CloseReply(value) => {
                let context = context.expect("runtime operation has a context");
                match get(*value) {
                    Value::ReplyTo(token) => {
                        context.close_reply_to(runtime::ReplyTo::from_token(token))?
                    }
                    Value::ReplyReceiver(token) => {
                        context.cancel_reply(runtime::ReplyReceiver::from_token(token))?
                    }
                    _ => unreachable!("verified reply capability"),
                }
                None
            }
            Operation::PackRequest { value, right } => {
                let Value::ReplyTo(token) = get(*right) else {
                    unreachable!("verified reply right")
                };
                Some(Value::Request(as_i64(get(*value)), token))
            }
            Operation::CloseRequest(value) => {
                let Value::Request(_, token) = get(*value) else {
                    unreachable!("verified request")
                };
                context
                    .expect("runtime operation has a context")
                    .close_reply_to(runtime::ReplyTo::from_token(token))?;
                None
            }
            Operation::CloseCaps(value) => {
                close_caps(get(*value), &mut context)?;
                None
            }
            Operation::ScopeEnter(_) => {
                context
                    .expect("scope enter requires a runtime task")
                    .scope_enter()?;
                None
            }
        })
    }
}

impl CoreTask {
    fn poll_step(&mut self, mut context: Option<&mut Context<'_>>) -> Result<Poll<i64>, String> {
        loop {
            let frame = self.frames.last().expect("active frame");
            let block = &self.program.functions[frame.function.0].blocks[frame.block];
            if frame.instruction < block.instructions.len() {
                let instruction = block.instructions[frame.instruction].clone();
                if let Operation::Call(target, args) = &instruction.operation {
                    let base = frame.base;
                    let args = args
                        .iter()
                        .map(|arg| match arg {
                            CallArg::Value(value) | CallArg::Ref(value, _) => self.value(*value),
                            CallArg::Borrow(place, _) => Value::Ref(base + place.0),
                        })
                        .collect();
                    if self.frames.len() >= 1024 {
                        return Err("interpreter call depth exceeded 1024".into());
                    }
                    self.frames.last_mut().expect("caller frame").instruction += 1;
                    self.push_frame(*target, args, instruction.result);
                    continue;
                }
                let result = self.eval_instruction(&instruction, context.as_deref_mut())?;
                let frame = self.frames.last_mut().expect("active frame");
                if let Some(id) = instruction.result {
                    frame.values[id.0] = result;
                }
                frame.instruction += 1;
                continue;
            }
            let terminator = block.terminator.clone();
            match terminator {
                Terminator::Yield { resume, .. } => {
                    self.enter(resume, Vec::new());
                    context
                        .as_deref_mut()
                        .ok_or("yield requires a runtime task")?
                        .yield_now()?;
                    return Ok(Poll::Pending);
                }
                Terminator::ScopeJoin { resume, .. } => {
                    match context
                        .as_deref_mut()
                        .ok_or("scope join requires a runtime task")?
                        .scope_join_current()?
                    {
                        Poll::Ready(()) => self.enter(resume, Vec::new()),
                        Poll::Pending => return Ok(Poll::Pending),
                    }
                }
                Terminator::Return(value) => {
                    let result = self.value(value);
                    let frame = self.frames.pop().expect("active frame");
                    self.stores.truncate(frame.base);
                    if let Some(parent) = self.frames.last_mut() {
                        parent.values[frame.return_to.expect("call has result").0] = Some(result);
                    } else {
                        return Ok(Poll::Ready(match result {
                            Value::Unit => 0,
                            other => as_i64(other),
                        }));
                    }
                }
                Terminator::Jump(target, args) => {
                    let args = args.into_iter().map(|arg| self.value(arg)).collect();
                    self.enter(target, args);
                }
                Terminator::Branch {
                    condition,
                    then_block,
                    else_block,
                } => {
                    self.enter(
                        if as_bool(self.value(condition)) {
                            then_block
                        } else {
                            else_block
                        },
                        Vec::new(),
                    );
                }
                Terminator::Match { subject, arms } => {
                    let original = self.value(subject);
                    let Value::Data(_, tag, payload) = &original else {
                        unreachable!("verified sum")
                    };
                    let arm = arms[*tag];
                    let args = match arm.binding {
                        MatchBinding::Discard => Vec::new(),
                        MatchBinding::Payload => vec![*payload.clone().expect("verified payload")],
                        MatchBinding::Whole => vec![original],
                    };
                    self.enter(arm.target, args);
                }
                Terminator::ChannelNew {
                    capacity,
                    success,
                    failure,
                } => {
                    let result = usize::try_from(as_i64(self.value(capacity)))
                        .map_err(|_| ResourceError::InvalidChannelCapacity)
                        .and_then(|capacity| {
                            context
                                .as_deref_mut()
                                .expect("runtime context")
                                .channel_typed(capacity)
                        });
                    match result {
                        Ok((sender, receiver)) => self.enter(
                            success,
                            vec![
                                Value::Sender(sender.token()),
                                Value::Receiver(receiver.token()),
                            ],
                        ),
                        Err(error) => self.enter(failure, vec![Value::ResourceError(error)]),
                    }
                }
                Terminator::TypedChannelNew {
                    message,
                    capacity,
                    success,
                    failure,
                } => {
                    let capacity_value = as_i64(self.value(capacity));
                    let descriptor = self.typed_descriptor(message)?;
                    let result = usize::try_from(capacity_value)
                        .map_err(|_| ResourceError::InvalidChannelCapacity)
                        .and_then(|capacity| {
                            context
                                .as_deref_mut()
                                .expect("runtime context")
                                .typed_right_channel(
                                    capacity,
                                    descriptor.type_id,
                                    descriptor.width,
                                    descriptor.rights.clone(),
                                )
                        });
                    match result {
                        Ok((sender, receiver)) => self.enter(
                            success,
                            vec![
                                Value::Sender(sender.token()),
                                Value::Receiver(receiver.token()),
                            ],
                        ),
                        Err(error) => self.enter(failure, vec![Value::ResourceError(error)]),
                    }
                }
                Terminator::ReplyNew { success, failure } => {
                    match context
                        .as_deref_mut()
                        .expect("runtime context")
                        .reply_typed()
                    {
                        Ok((right, receiver)) => self.enter(
                            success,
                            vec![
                                Value::ReplyTo(right.token()),
                                Value::ReplyReceiver(receiver.token()),
                            ],
                        ),
                        Err(error) => self.enter(failure, vec![Value::ResourceError(error)]),
                    }
                }
                Terminator::RequestChannelNew {
                    capacity,
                    success,
                    failure,
                } => {
                    let descriptor = self.request_descriptor();
                    let result = usize::try_from(as_i64(self.value(capacity)))
                        .map_err(|_| ResourceError::InvalidChannelCapacity)
                        .and_then(|capacity| {
                            context
                                .as_deref_mut()
                                .expect("runtime context")
                                .typed_right_channel(
                                    capacity,
                                    descriptor.type_id,
                                    descriptor.width,
                                    descriptor.rights.clone(),
                                )
                        });
                    match result {
                        Ok((sender, receiver)) => self.enter(
                            success,
                            vec![
                                Value::Sender(sender.token()),
                                Value::Receiver(receiver.token()),
                            ],
                        ),
                        Err(error) => self.enter(failure, vec![Value::ResourceError(error)]),
                    }
                }
                Terminator::RequestUnpack { request, next } => {
                    let Value::Request(value, right) = self.value(request) else {
                        unreachable!("verified request")
                    };
                    self.enter(next, vec![Value::I64(value), Value::ReplyTo(right)]);
                }
                Terminator::ProductUnpack { product, next } => {
                    let Value::Product(_, fields) = self.value(product) else {
                        unreachable!("verified product")
                    };
                    self.enter(next, fields);
                }
                Terminator::RequestSend {
                    sender,
                    request,
                    sent,
                    closed,
                } => {
                    let Value::Sender(sender_token) = self.value(sender) else {
                        unreachable!("verified sender")
                    };
                    let descriptor = self.request_descriptor();
                    let request_value = self.value(request);
                    let mut words = [0; 2];
                    message::encode_into(
                        &self.program.data,
                        self.message_catalog().shapes(),
                        Type::RequestI64,
                        &request_value,
                        &mut words,
                    )?;
                    match context
                        .as_deref_mut()
                        .expect("runtime context")
                        .send_typed_words(
                            &runtime::Sender::from_token(sender_token),
                            descriptor.type_id,
                            &words,
                        )? {
                        runtime::TypedSendResult::Sent => self.enter(sent, Vec::new()),
                        runtime::TypedSendResult::Closed => {
                            self.enter(closed, vec![request_value]);
                        }
                        runtime::TypedSendResult::Pending => return Ok(Poll::Pending),
                    }
                }
                Terminator::TypedSend {
                    sender,
                    message,
                    sent,
                    closed,
                } => {
                    let Value::Sender(token) = self.value(sender) else {
                        unreachable!("verified typed sender")
                    };
                    let function = self.frames.last().expect("active frame").function;
                    let Type::TypedSender(message_type) =
                        self.program.functions[function.0].value_types[sender.0]
                    else {
                        unreachable!("verified typed sender type")
                    };
                    let value = self.value(message);
                    let words = message::encode(
                        &self.program.data,
                        self.message_catalog().shapes(),
                        message_type.value_type(),
                        &value,
                    )?;
                    let type_id = self.typed_descriptor(message_type)?.type_id;
                    match context
                        .as_deref_mut()
                        .expect("runtime context")
                        .send_typed_words(&runtime::Sender::from_token(token), type_id, &words)?
                    {
                        runtime::TypedSendResult::Sent => self.enter(sent, Vec::new()),
                        runtime::TypedSendResult::Closed => self.enter(closed, vec![value]),
                        runtime::TypedSendResult::Pending => return Ok(Poll::Pending),
                    }
                }
                Terminator::RequestReceive {
                    receiver,
                    value,
                    end,
                } => {
                    let Value::Receiver(token) = self.value(receiver) else {
                        unreachable!("verified receiver")
                    };
                    let descriptor = self.request_descriptor();
                    let mut words = [0; 2];
                    match context
                        .as_deref_mut()
                        .expect("runtime context")
                        .receive_typed_words(
                            &runtime::Receiver::from_token(token),
                            descriptor.type_id,
                            &mut words,
                        )? {
                        runtime::TypedReceiveResult::Value => self.enter(
                            value,
                            vec![message::decode(
                                &self.program.data,
                                self.message_catalog().shapes(),
                                Type::RequestI64,
                                &words,
                            )?],
                        ),
                        runtime::TypedReceiveResult::End => self.enter(end, Vec::new()),
                        runtime::TypedReceiveResult::Pending => return Ok(Poll::Pending),
                    }
                }
                Terminator::TypedReceive {
                    receiver,
                    value,
                    end,
                } => {
                    let Value::Receiver(token) = self.value(receiver) else {
                        unreachable!("verified typed receiver")
                    };
                    let function = self.frames.last().expect("active frame").function;
                    let Type::TypedReceiver(message_type) =
                        self.program.functions[function.0].value_types[receiver.0]
                    else {
                        unreachable!("verified typed receiver type")
                    };
                    let descriptor = self.typed_descriptor(message_type)?;
                    let mut words = vec![0; descriptor.width];
                    match context
                        .as_deref_mut()
                        .expect("runtime context")
                        .receive_typed_words(
                            &runtime::Receiver::from_token(token),
                            descriptor.type_id,
                            &mut words,
                        )? {
                        runtime::TypedReceiveResult::Value => self.enter(
                            value,
                            vec![message::decode(
                                &self.program.data,
                                self.message_catalog().shapes(),
                                message_type.value_type(),
                                &words,
                            )?],
                        ),
                        runtime::TypedReceiveResult::End => self.enter(end, Vec::new()),
                        runtime::TypedReceiveResult::Pending => return Ok(Poll::Pending),
                    }
                }
                Terminator::Fork {
                    source,
                    success,
                    failure,
                } => {
                    let result = match self.value(source) {
                        Value::Sender(token) => context
                            .as_deref_mut()
                            .expect("runtime context")
                            .fork_sender_typed(&runtime::Sender::from_token(token))?
                            .map(|handle| Value::Sender(handle.token())),
                        Value::Receiver(token) => context
                            .as_deref_mut()
                            .expect("runtime context")
                            .fork_receiver_typed(&runtime::Receiver::from_token(token))?
                            .map(|handle| Value::Receiver(handle.token())),
                        _ => unreachable!("verified endpoint"),
                    };
                    match result {
                        Ok(handle) => self.enter(success, vec![handle]),
                        Err(error) => self.enter(failure, vec![Value::ResourceError(error)]),
                    }
                }
                Terminator::ChannelSend {
                    sender,
                    message,
                    sent,
                    closed,
                } => {
                    let Value::Sender(token) = self.value(sender) else {
                        unreachable!("verified sender")
                    };
                    let message = as_i64(self.value(message));
                    match context
                        .as_deref_mut()
                        .expect("runtime context")
                        .send(&runtime::Sender::from_token(token), message)?
                    {
                        runtime::SendResult::Sent => self.enter(sent, Vec::new()),
                        runtime::SendResult::Closed(_) => self.enter(closed, Vec::new()),
                        runtime::SendResult::Pending(_) => return Ok(Poll::Pending),
                    }
                }
                Terminator::ChannelReceive {
                    receiver,
                    value,
                    end,
                } => {
                    let Value::Receiver(token) = self.value(receiver) else {
                        unreachable!("verified receiver")
                    };
                    match context
                        .as_deref_mut()
                        .expect("runtime context")
                        .receive(&runtime::Receiver::from_token(token))?
                    {
                        runtime::ReceiveResult::Value(message) => {
                            self.enter(value, vec![Value::I64(message)])
                        }
                        runtime::ReceiveResult::End => self.enter(end, Vec::new()),
                        runtime::ReceiveResult::Pending => return Ok(Poll::Pending),
                    }
                }
                Terminator::ReplyComplete {
                    right,
                    value,
                    delivered,
                    closed,
                } => {
                    let Value::ReplyTo(token) = self.value(right) else {
                        unreachable!("verified reply right")
                    };
                    let value = as_i64(self.value(value));
                    match context
                        .as_deref_mut()
                        .expect("runtime context")
                        .complete(runtime::ReplyTo::from_token(token), value)?
                    {
                        runtime::CompleteResult::Delivered => self.enter(delivered, Vec::new()),
                        runtime::CompleteResult::Closed(_) => self.enter(closed, Vec::new()),
                    }
                }
                Terminator::ReplyReceive {
                    receiver,
                    value,
                    end,
                } => {
                    let Value::ReplyReceiver(token) = self.value(receiver) else {
                        unreachable!("verified reply receiver")
                    };
                    match context
                        .as_deref_mut()
                        .expect("runtime context")
                        .receive_reply(&runtime::ReplyReceiver::from_token(token))?
                    {
                        runtime::ReplyReceiveResult::Value(message) => {
                            self.enter(value, vec![Value::I64(message)])
                        }
                        runtime::ReplyReceiveResult::End => self.enter(end, Vec::new()),
                        runtime::ReplyReceiveResult::Pending => return Ok(Poll::Pending),
                    }
                }
                Terminator::Spawn {
                    child,
                    args,
                    success,
                    failure,
                    ..
                } => {
                    let child_values = args
                        .into_iter()
                        .map(|arg| self.value(arg))
                        .collect::<Vec<_>>();
                    let mut transfers = Vec::new();
                    for value in &child_values {
                        collect_capability_transfers(value, &mut transfers);
                    }
                    let child_task = CoreTask::new(
                        Arc::clone(&self.program),
                        self.generics.clone(),
                        child,
                        child_values,
                    );
                    match context
                        .as_deref_mut()
                        .expect("runtime context")
                        .spawn_capabilities_owned(child_task, &transfers)
                    {
                        Ok(_) => self.enter(success, Vec::new()),
                        Err(error) => match error.resource_error {
                            Some(resource) => {
                                self.enter(failure, vec![Value::ResourceError(resource)])
                            }
                            None => {
                                return Err(format!(
                                    "runtime spawn invariant failed: {}",
                                    error.message
                                ));
                            }
                        },
                    }
                }
            }
        }
    }
}

impl Task for CoreTask {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        self.poll_step(Some(context))
    }
}

fn as_i64(value: Value) -> i64 {
    match value {
        Value::I64(x) => x,
        Value::Bool(_)
        | Value::Unit
        | Value::Data(_, _, _)
        | Value::Product(_, _)
        | Value::Ref(_)
        | Value::Sender(_)
        | Value::Receiver(_)
        | Value::ReplyTo(_)
        | Value::ReplyReceiver(_)
        | Value::Request(_, _)
        | Value::ResourceError(_) => {
            unreachable!("verified I64 operand")
        }
    }
}

fn as_bool(value: Value) -> bool {
    match value {
        Value::Bool(x) => x,
        Value::I64(_)
        | Value::Unit
        | Value::Data(_, _, _)
        | Value::Product(_, _)
        | Value::Ref(_)
        | Value::Sender(_)
        | Value::Receiver(_)
        | Value::ReplyTo(_)
        | Value::ReplyReceiver(_)
        | Value::Request(_, _)
        | Value::ResourceError(_) => {
            unreachable!("verified Bool operand")
        }
    }
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for function in &self.functions {
            write!(f, "fn {}(", function.name)?;
            for (index, param) in function.blocks[0].params.iter().enumerate() {
                if index > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "v{}: {:?}", param.0, function.value_types[param.0])?;
            }
            writeln!(f, ") -> {:?} {{", function.return_type)?;
            for region in &function.loan_regions {
                write!(
                    f,
                    "  region r{}: begin block{}.i{} roots(",
                    region.id.0, region.call_block.0, region.call_instruction
                )?;
                for (index, root) in region.roots.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    match root {
                        LoanRoot::Place(place, false) => write!(f, "&p{}", place.0)?,
                        LoanRoot::Place(place, true) => write!(f, "&mut p{}", place.0)?,
                        LoanRoot::Forwarded(value, false) => write!(f, "&v{}", value.0)?,
                        LoanRoot::Forwarded(value, true) => write!(f, "&mut v{}", value.0)?,
                    }
                }
                writeln!(f, ") end call return/unwind")?;
            }
            for (block_id, block) in function.blocks.iter().enumerate() {
                write!(f, "  block{block_id}(")?;
                for (index, param) in block.params.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "v{}: {:?}", param.0, function.value_types[param.0])?;
                }
                writeln!(f, "):")?;
                for instruction in &block.instructions {
                    if let Some(result) = instruction.result {
                        write!(
                            f,
                            "    v{}: {:?} = ",
                            result.0, function.value_types[result.0]
                        )?;
                    } else {
                        write!(f, "    ")?;
                    }
                    match &instruction.operation {
                        Operation::Const(x) => write!(f, "const {x}")?,
                        Operation::Bool(x) => write!(f, "bool {x}")?,
                        Operation::Unit => write!(f, "unit")?,
                        Operation::Move(value) => write!(f, "move v{}", value.0)?,
                        Operation::Copy(value) => write!(f, "copy.value v{}", value.0)?,
                        Operation::Variant(id, tag, payload) => write!(
                            f,
                            "variant {}.{}",
                            self.data[id.0].name, self.data[id.0].variants[*tag]
                        )
                        .and_then(|()| match payload {
                            Some(value) => write!(f, "(v{})", value.0),
                            None => Ok(()),
                        })?,
                        Operation::Product(id, fields) => {
                            write!(f, "product {}(", self.data[id.0].name)?;
                            for (index, field) in fields.iter().enumerate() {
                                if index > 0 {
                                    write!(f, ", ")?;
                                }
                                write!(f, "v{}", field.0)?;
                            }
                            write!(f, ")")?;
                        }
                        Operation::Field(owner, index) => {
                            write!(f, "field v{}, {}", owner.0, index)?
                        }
                        Operation::ReadField(place, index) => {
                            write!(f, "place.field p{}, {}", place.0, index)?
                        }
                        Operation::Neg(a) => write!(f, "neg v{}", a.0)?,
                        Operation::Not(a) => write!(f, "not v{}", a.0)?,
                        Operation::Add(a, b) => write!(f, "add v{}, v{}", a.0, b.0)?,
                        Operation::Sub(a, b) => write!(f, "sub v{}, v{}", a.0, b.0)?,
                        Operation::Mul(a, b) => write!(f, "mul v{}, v{}", a.0, b.0)?,
                        Operation::Eq(a, b) => write!(f, "eq v{}, v{}", a.0, b.0)?,
                        Operation::Ne(a, b) => write!(f, "ne v{}, v{}", a.0, b.0)?,
                        Operation::Lt(a, b) => write!(f, "lt v{}, v{}", a.0, b.0)?,
                        Operation::Le(a, b) => write!(f, "le v{}, v{}", a.0, b.0)?,
                        Operation::Gt(a, b) => write!(f, "gt v{}, v{}", a.0, b.0)?,
                        Operation::Ge(a, b) => write!(f, "ge v{}, v{}", a.0, b.0)?,
                        Operation::Call(target, args) => {
                            write!(f, "call {}(", self.functions[target.0].name)?;
                            for (index, arg) in args.iter().enumerate() {
                                if index > 0 {
                                    write!(f, ", ")?;
                                }
                                match arg {
                                    CallArg::Value(value) => write!(f, "v{}", value.0)?,
                                    CallArg::Borrow(place, false) => write!(f, "&p{}", place.0)?,
                                    CallArg::Borrow(place, true) => write!(f, "&mut p{}", place.0)?,
                                    CallArg::Ref(value, false) => write!(f, "&v{}", value.0)?,
                                    CallArg::Ref(value, true) => write!(f, "&mut v{}", value.0)?,
                                }
                            }
                            write!(f, ")")?;
                        }
                        Operation::GenericCall {
                            callee,
                            type_args,
                            args,
                        } => {
                            write!(f, "call.generic f{callee}<{type_args:?}>(")?;
                            for (index, value) in args.iter().enumerate() {
                                if index > 0 {
                                    write!(f, ", ")?;
                                }
                                write!(f, "v{}", value.0)?;
                            }
                            write!(f, ")")?;
                        }
                        Operation::PlaceInit(place, value) => {
                            write!(f, "place.init p{}, v{}", place.0, value.0)?
                        }
                        Operation::Load(place) => write!(f, "place.load p{}", place.0)?,
                        Operation::Take(place) => write!(f, "place.take p{}", place.0)?,
                        Operation::CopyPlace(place) => write!(f, "copy.value p{}", place.0)?,
                        Operation::Store(place, value) => {
                            write!(f, "place.store p{}, v{}", place.0, value.0)?
                        }
                        Operation::BorrowBegin(place, false) => {
                            write!(f, "borrow.begin &p{}", place.0)?
                        }
                        Operation::BorrowBegin(place, true) => {
                            write!(f, "borrow.begin &mut p{}", place.0)?
                        }
                        Operation::BorrowReborrow(parent, false) => {
                            write!(f, "borrow.reborrow &v{}", parent.0)?
                        }
                        Operation::BorrowReborrow(parent, true) => {
                            write!(f, "borrow.reborrow &mut v{}", parent.0)?
                        }
                        Operation::BorrowEnd(reference) => {
                            write!(f, "borrow.end v{}", reference.0)?
                        }
                        Operation::LoadRef(reference) => write!(f, "ref.load v{}", reference.0)?,
                        Operation::StoreRef(reference, value) => {
                            write!(f, "ref.store v{}, v{}", reference.0, value.0)?
                        }
                        Operation::CloseHandle(value) => write!(f, "close.handle v{}", value.0)?,
                        Operation::CloseReply(value) => write!(f, "close.reply v{}", value.0)?,
                        Operation::PackRequest { value, right } => {
                            write!(f, "request.pack v{}, v{}", value.0, right.0)?
                        }
                        Operation::CloseRequest(value) => write!(f, "close.request v{}", value.0)?,
                        Operation::CloseCaps(value) => write!(f, "close.caps v{}", value.0)?,
                        Operation::ScopeEnter(scope) => write!(f, "scope.enter r{}", scope.0)?,
                    }
                    writeln!(f, "  @{}..{}", instruction.span.start, instruction.span.end)?;
                }
                match &block.terminator {
                    Terminator::Yield {
                        resume, automatic, ..
                    } => {
                        let name = if *automatic { "yield.auto" } else { "yield" };
                        writeln!(f, "    {name} resume block{}", resume.0)?
                    }
                    Terminator::ScopeJoin { scope, resume, .. } => {
                        writeln!(f, "    scope.join r{} resume block{}", scope.0, resume.0)?
                    }
                    Terminator::Return(value) => writeln!(f, "    return v{}", value.0)?,
                    Terminator::Jump(target, args) => {
                        write!(f, "    jump block{}(", target.0)?;
                        for (index, arg) in args.iter().enumerate() {
                            if index > 0 {
                                write!(f, ", ")?;
                            }
                            write!(f, "v{}", arg.0)?;
                        }
                        writeln!(f, ")")?;
                    }
                    Terminator::Branch {
                        condition,
                        then_block,
                        else_block,
                    } => {
                        writeln!(
                            f,
                            "    branch v{}, block{}, block{}",
                            condition.0, then_block.0, else_block.0
                        )?;
                    }
                    Terminator::Match { subject, arms } => {
                        write!(f, "    match v{}", subject.0)?;
                        for (tag, arm) in arms.iter().enumerate() {
                            write!(f, ", {}->block{}:{:?}", tag, arm.target.0, arm.binding)?;
                        }
                        writeln!(f)?;
                    }
                    Terminator::ChannelNew {
                        capacity,
                        success,
                        failure,
                    } => writeln!(
                        f,
                        "    channel.new.i64 v{} success block{} failure block{}",
                        capacity.0, success.0, failure.0
                    )?,
                    Terminator::TypedChannelNew {
                        message,
                        capacity,
                        success,
                        failure,
                    } => writeln!(
                        f,
                        "    channel.new.typed {message:?} v{} success block{} failure block{}",
                        capacity.0, success.0, failure.0
                    )?,
                    Terminator::ReplyNew { success, failure } => writeln!(
                        f,
                        "    reply.new.i64 success block{} failure block{}",
                        success.0, failure.0
                    )?,
                    Terminator::RequestChannelNew {
                        capacity,
                        success,
                        failure,
                    } => writeln!(
                        f,
                        "    channel.new.request.i64 v{} success block{} failure block{}",
                        capacity.0, success.0, failure.0
                    )?,
                    Terminator::RequestUnpack { request, next } => {
                        writeln!(f, "    request.unpack v{} next block{}", request.0, next.0)?
                    }
                    Terminator::ProductUnpack { product, next } => {
                        writeln!(f, "    product.unpack v{} next block{}", product.0, next.0)?
                    }
                    Terminator::RequestSend {
                        sender,
                        request,
                        sent,
                        closed,
                    } => writeln!(
                        f,
                        "    channel.send.request.i64 v{}, v{} sent block{} closed block{} pending retry",
                        sender.0, request.0, sent.0, closed.0
                    )?,
                    Terminator::RequestReceive {
                        receiver,
                        value,
                        end,
                    } => writeln!(
                        f,
                        "    channel.receive.request.i64 v{} value block{} end block{} pending retry",
                        receiver.0, value.0, end.0
                    )?,
                    Terminator::Fork {
                        source,
                        success,
                        failure,
                    } => writeln!(
                        f,
                        "    handle.fork v{} success block{} failure block{}",
                        source.0, success.0, failure.0
                    )?,
                    Terminator::ChannelSend {
                        sender,
                        message,
                        sent,
                        closed,
                    } => writeln!(
                        f,
                        "    channel.send.i64 v{}, v{} sent block{} closed block{} pending retry",
                        sender.0, message.0, sent.0, closed.0
                    )?,
                    Terminator::TypedSend {
                        sender,
                        message,
                        sent,
                        closed,
                    } => writeln!(
                        f,
                        "    channel.send.typed v{}, v{} sent block{} closed block{} pending retry",
                        sender.0, message.0, sent.0, closed.0
                    )?,
                    Terminator::ChannelReceive {
                        receiver,
                        value,
                        end,
                    } => writeln!(
                        f,
                        "    channel.receive.i64 v{} value block{} end block{} pending retry",
                        receiver.0, value.0, end.0
                    )?,
                    Terminator::TypedReceive {
                        receiver,
                        value,
                        end,
                    } => writeln!(
                        f,
                        "    channel.receive.typed v{} value block{} end block{} pending retry",
                        receiver.0, value.0, end.0
                    )?,
                    Terminator::ReplyComplete {
                        right,
                        value,
                        delivered,
                        closed,
                    } => writeln!(
                        f,
                        "    reply.complete.i64 v{}, v{} delivered block{} closed block{}",
                        right.0, value.0, delivered.0, closed.0
                    )?,
                    Terminator::ReplyReceive {
                        receiver,
                        value,
                        end,
                    } => writeln!(
                        f,
                        "    reply.receive.i64 v{} value block{} end block{} pending retry",
                        receiver.0, value.0, end.0
                    )?,
                    Terminator::Spawn {
                        child,
                        args,
                        success,
                        failure,
                        ..
                    } => {
                        write!(f, "    spawn {}(", self.functions[child.0].name)?;
                        for (index, arg) in args.iter().enumerate() {
                            if index > 0 {
                                write!(f, ", ")?;
                            }
                            write!(f, "v{}", arg.0)?;
                        }
                        writeln!(f, ") success block{} failure block{}", success.0, failure.0)?;
                    }
                }
            }
            writeln!(f, "}}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod interpreter_frame_tests {
    use super::*;
    use phoebe_runtime::{Sender, TaskId};

    fn instruction(result: Option<ValueId>, operation: Operation) -> Instruction {
        Instruction {
            result,
            operation,
            span: Span { start: 0, end: 0 },
        }
    }

    fn receiver_program() -> Program {
        Program {
            data: vec![],
            functions: vec![Function {
                name: "child".into(),
                param_modes: vec![ParamMode::Own],
                return_type: Type::I64,
                value_types: vec![Type::ReceiverI64, Type::I64, Type::I64],
                place_types: vec![],
                loan_regions: vec![],
                blocks: vec![
                    Block {
                        params: vec![ValueId(0)],
                        instructions: vec![],
                        terminator: Terminator::ChannelReceive {
                            receiver: ValueId(0),
                            value: BlockId(1),
                            end: BlockId(2),
                        },
                    },
                    Block {
                        params: vec![ValueId(1)],
                        instructions: vec![instruction(None, Operation::CloseHandle(ValueId(0)))],
                        terminator: Terminator::Return(ValueId(1)),
                    },
                    Block {
                        params: vec![],
                        instructions: vec![
                            instruction(None, Operation::CloseHandle(ValueId(0))),
                            instruction(Some(ValueId(2)), Operation::Const(0)),
                        ],
                        terminator: Terminator::Return(ValueId(2)),
                    },
                ],
            }],
        }
    }

    struct Root {
        program: Arc<Program>,
        sender: Option<Sender>,
        child: Option<TaskId>,
        phase: u8,
    }

    impl Task for Root {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            if self.phase == 0 {
                let (sender, receiver) = context.channel(1)?;
                let token = receiver.token();
                let child = CoreTask::new(
                    Arc::clone(&self.program),
                    None,
                    FunctionId(0),
                    vec![Value::Receiver(token)],
                );
                self.child = Some(context.spawn(child, &[token])?);
                self.sender = Some(sender);
                self.phase = 1;
                context.yield_now()?;
                return Ok(Poll::Pending);
            }
            if self.phase == 1 {
                assert_eq!(
                    context.send(self.sender.as_ref().expect("sender"), 42)?,
                    runtime::SendResult::Sent
                );
                context.close_sender(self.sender.take().expect("sender"))?;
                self.phase = 2;
            }
            match context.join()? {
                Poll::Pending => Ok(Poll::Pending),
                Poll::Ready(()) => Ok(Poll::Ready(
                    context
                        .task_result(self.child.expect("child"))?
                        .expect("child result"),
                )),
            }
        }
    }

    #[test]
    fn retained_core_child_frame_resumes_after_receive_wake() {
        let program = receiver_program();
        verify(&program).unwrap();
        let mut scheduler = Scheduler::new(
            Root {
                program: Arc::new(program),
                sender: None,
                child: None,
                phase: 0,
            },
            2,
            1,
            0,
        )
        .unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        assert!(scheduler.stats().parks >= 1);
        assert!(scheduler.stats().wakes >= 1);
        assert_eq!(scheduler.open_handles(), 0);
        scheduler.verify().unwrap();
    }
}
