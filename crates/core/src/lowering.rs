use super::*;

struct DraftBlock {
    params: Vec<ValueId>,
    instructions: Vec<Instruction>,
    terminator: Option<Terminator>,
}

#[derive(Clone)]
struct FunctionSignature {
    id: FunctionId,
    params: Vec<(ParamMode, Type)>,
    return_type: Type,
}

struct CleanupSites {
    returns: Vec<(BlockId, Span)>,
    scopes: Vec<(BlockId, ValueId, Span)>,
}

struct Lowerer<'a> {
    functions: &'a HashMap<String, FunctionSignature>,
    data: &'a [DataDef],
    data_names: &'a HashMap<String, DataId>,
    blocks: Vec<DraftBlock>,
    value_types: Vec<Type>,
    place_types: Vec<Type>,
    param_modes: Vec<ParamMode>,
    return_type: Type,
    current: BlockId,
    implicit_cleanup_returns: Vec<(BlockId, Span)>,
    scoped_cleanup_exits: Vec<(BlockId, ValueId, Span)>,
}

impl<'a> Lowerer<'a> {
    fn new(
        functions: &'a HashMap<String, FunctionSignature>,
        data: &'a [DataDef],
        data_names: &'a HashMap<String, DataId>,
        params: Vec<(ParamMode, Type)>,
        return_type: Type,
    ) -> Self {
        let mut lowerer = Self {
            functions,
            data,
            data_names,
            blocks: vec![DraftBlock {
                params: Vec::new(),
                instructions: Vec::new(),
                terminator: None,
            }],
            value_types: Vec::new(),
            place_types: Vec::new(),
            param_modes: params.iter().map(|(mode, _)| *mode).collect(),
            return_type,
            current: BlockId(0),
            implicit_cleanup_returns: Vec::new(),
            scoped_cleanup_exits: Vec::new(),
        };
        for (mode, own_type) in params {
            let ty = match mode {
                ParamMode::Own => own_type,
                ParamMode::Shared => Type::RefI64,
                ParamMode::Exclusive => Type::RefMutI64,
            };
            let id = lowerer.value(ty);
            lowerer.blocks[0].params.push(id);
        }
        lowerer
    }

    fn value(&mut self, ty: Type) -> ValueId {
        let id = ValueId(self.value_types.len());
        self.value_types.push(ty);
        id
    }

    fn block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len());
        self.blocks.push(DraftBlock {
            params: Vec::new(),
            instructions: Vec::new(),
            terminator: None,
        });
        id
    }

    fn block_param(&mut self, block: BlockId, ty: Type) -> ValueId {
        let id = self.value(ty);
        self.blocks[block.0].params.push(id);
        id
    }

    fn emit(&mut self, operation: Operation, ty: Type, span: Span) -> ValueId {
        let result = self.value(ty);
        self.blocks[self.current.0].instructions.push(Instruction {
            result: Some(result),
            operation,
            span,
        });
        result
    }

    fn emit_effect(&mut self, operation: Operation, span: Span) {
        self.blocks[self.current.0].instructions.push(Instruction {
            result: None,
            operation,
            span,
        });
    }

    fn place(&mut self, ty: Type) -> PlaceId {
        let id = PlaceId(self.place_types.len());
        self.place_types.push(ty);
        id
    }

    fn terminate(&mut self, terminator: Terminator) {
        assert!(self.blocks[self.current.0].terminator.is_none());
        self.blocks[self.current.0].terminator = Some(terminator);
    }

    fn require(&self, actual: Type, expected: Type, span: Span) -> Result<(), Diagnostic> {
        if actual == expected {
            Ok(())
        } else {
            Err(Diagnostic::new(
                span,
                format!("expected {expected:?}, got {actual:?}"),
            ))
        }
    }

    fn resource_result_id(&self, span: Span) -> Result<DataId, Diagnostic> {
        let Type::Data(result_id) = self.return_type else {
            return Err(Diagnostic::new(
                span,
                "? requires Result<_, ResourceError> return type",
            ));
        };
        let result = &self.data[result_id.0];
        if !result.name.starts_with("Result<")
            || result.variants.as_slice() != ["Ok", "Err"]
            || result.payloads[1] != Some(Type::ResourceError)
        {
            return Err(Diagnostic::new(
                span,
                "? requires Result<_, ResourceError> return type",
            ));
        }
        Ok(result_id)
    }

    fn with_body(
        &mut self,
        name: &str,
        span: Span,
        value: &Expr,
        body: &Body,
        locals: &HashMap<String, Binding>,
    ) -> Result<Option<TypedValue>, Diagnostic> {
        let (owner, ty) = self.expr(value, locals)?;
        if !matches!(ty, Type::SenderI64 | Type::ReceiverI64) {
            return Err(Diagnostic::new(
                value.span,
                "with currently owns a Sender<I64> or Receiver<I64>",
            ));
        }
        let mut scoped_locals = locals.clone();
        if scoped_locals
            .insert(name.to_owned(), Binding::Val(owner))
            .is_some()
        {
            return Err(Diagnostic::new(span, format!("duplicate local {name}")));
        }
        let result = self.body(body, &mut scoped_locals)?;
        if let Some((_, ty)) = result
            && matches!(ty, Type::SenderI64 | Type::ReceiverI64)
        {
            return Err(Diagnostic::new(
                body.tail.as_ref().map_or(span, |tail| tail.span),
                "with cannot return its endpoint; move it to another owner inside the block",
            ));
        }
        let exit = self.current;
        let next = self.block();
        let output = if let Some((value, ty)) = result {
            let param = self.block_param(next, ty);
            self.terminate(Terminator::Jump(next, vec![value]));
            Some((param, ty))
        } else {
            self.terminate(Terminator::Jump(next, vec![]));
            None
        };
        self.scoped_cleanup_exits.push((exit, owner, span));
        self.current = next;
        Ok(output)
    }

    fn body(
        &mut self,
        body: &Body,
        locals: &mut HashMap<String, Binding>,
    ) -> Result<Option<TypedValue>, Diagnostic> {
        for statement in &body.statements {
            match statement {
                Statement::ChannelVal {
                    sender,
                    receiver,
                    capacity,
                    span,
                } => {
                    for (name, name_span) in [sender, receiver] {
                        if locals.contains_key(name) || sender.0 == receiver.0 {
                            return Err(Diagnostic::new(
                                *name_span,
                                format!("duplicate local {name}"),
                            ));
                        }
                    }
                    let result_id = self.resource_result_id(*span).map_err(|_| {
                        Diagnostic::new(
                            *span,
                            "channel creation with ? requires Result<_, ResourceError> return type",
                        )
                    })?;
                    let (value, ty) = self.expr(capacity, locals)?;
                    self.require(ty, Type::I64, capacity.span)?;
                    let success = self.block();
                    let send_value = self.block_param(success, Type::SenderI64);
                    let receive_value = self.block_param(success, Type::ReceiverI64);
                    let failure = self.block();
                    let error = self.block_param(failure, Type::ResourceError);
                    self.terminate(Terminator::ChannelNew {
                        capacity: value,
                        success,
                        failure,
                    });
                    self.current = failure;
                    let failed = self.emit(
                        Operation::Variant(result_id, 1, Some(error)),
                        self.return_type,
                        *span,
                    );
                    self.terminate(Terminator::Return(failed));
                    self.implicit_cleanup_returns.push((self.current, *span));
                    self.current = success;
                    locals.insert(sender.0.clone(), Binding::Val(send_value));
                    locals.insert(receiver.0.clone(), Binding::Val(receive_value));
                }
                Statement::Spawn { child, args, span } => {
                    let result_id = self.resource_result_id(*span)?;
                    let signature = self.functions.get(child).cloned().ok_or_else(|| {
                        Diagnostic::new(*span, format!("unknown function {child}"))
                    })?;
                    if signature.return_type != Type::I64
                        || signature.params.len() != args.len()
                        || signature
                            .params
                            .iter()
                            .any(|(mode, _)| *mode != ParamMode::Own)
                    {
                        return Err(Diagnostic::new(
                            *span,
                            "spawn requires an I64-returning function with owned arguments",
                        ));
                    }
                    let mut values = Vec::with_capacity(args.len());
                    for (arg, (_, expected)) in args.iter().zip(&signature.params) {
                        let CallArgument::Value(expr) = arg else {
                            return Err(Diagnostic::new(*span, "spawn arguments must be owned"));
                        };
                        let (value, ty) = if let ExprKind::Name(name) = &expr.kind {
                            match locals.get(name).copied() {
                                Some(Binding::Val(value)) => (value, self.value_types[value.0]),
                                Some(Binding::Var(place))
                                    if matches!(
                                        self.place_types[place.0],
                                        Type::Data(_) | Type::SenderI64 | Type::ReceiverI64
                                    ) =>
                                {
                                    return Err(Diagnostic::new(
                                        expr.span,
                                        "owned spawn arguments must be named vals until retained cleanup supports places",
                                    ));
                                }
                                _ => self.expr(expr, locals)?,
                            }
                        } else {
                            self.expr(expr, locals)?
                        };
                        self.require(ty, *expected, expr.span)?;
                        if matches!(ty, Type::Data(_) | Type::SenderI64 | Type::ReceiverI64)
                            && !matches!(expr.kind, ExprKind::Name(_))
                        {
                            return Err(Diagnostic::new(
                                expr.span,
                                "owned spawn arguments must be named vals until retained cleanup supports temporaries",
                            ));
                        }
                        values.push(value);
                    }
                    let success = self.block();
                    let failure = self.block();
                    let error = self.block_param(failure, Type::ResourceError);
                    self.terminate(Terminator::Spawn {
                        child: signature.id,
                        args: values,
                        success,
                        failure,
                    });
                    self.current = failure;
                    let failed = self.emit(
                        Operation::Variant(result_id, 1, Some(error)),
                        self.return_type,
                        *span,
                    );
                    self.terminate(Terminator::Return(failed));
                    self.implicit_cleanup_returns.push((self.current, *span));
                    self.current = success;
                }
                Statement::With {
                    name,
                    span,
                    value,
                    body,
                } => {
                    if let Some(tail) = &body.tail {
                        return Err(Diagnostic::new(
                            tail.span,
                            "with body cannot end in a value expression; add a semicolon",
                        ));
                    }
                    self.with_body(name, *span, value, body, locals)?;
                }
                Statement::Yield(span) => {
                    let resume = self.block();
                    self.terminate(Terminator::Yield {
                        resume,
                        span: *span,
                    });
                    self.current = resume;
                }
                Statement::Val(name, span, expr) | Statement::Var(name, span, expr) => {
                    if locals.contains_key(name) {
                        return Err(Diagnostic::new(*span, format!("duplicate local {name}")));
                    }
                    if let ExprKind::Try(inner) = &expr.kind
                        && let ExprKind::QualifiedCall(source, method, args) = &inner.kind
                        && method == "fork"
                        && args.is_empty()
                    {
                        if matches!(statement, Statement::Var(..)) {
                            return Err(Diagnostic::new(*span, "fork result must bind to val"));
                        }
                        let endpoint = match locals.get(source).copied() {
                            Some(Binding::Val(value)) => value,
                            _ => {
                                return Err(Diagnostic::new(
                                    inner.span,
                                    "fork requires a named val endpoint",
                                ));
                            }
                        };
                        let ty = self.value_types[endpoint.0];
                        if !matches!(ty, Type::SenderI64 | Type::ReceiverI64) {
                            return Err(Diagnostic::new(
                                inner.span,
                                "fork requires Sender<I64> or Receiver<I64>",
                            ));
                        }
                        let result_id = self.resource_result_id(expr.span)?;
                        let success = self.block();
                        let fresh = self.block_param(success, ty);
                        let failure = self.block();
                        let error = self.block_param(failure, Type::ResourceError);
                        self.terminate(Terminator::Fork {
                            source: endpoint,
                            success,
                            failure,
                        });
                        self.current = failure;
                        let failed = self.emit(
                            Operation::Variant(result_id, 1, Some(error)),
                            self.return_type,
                            expr.span,
                        );
                        self.terminate(Terminator::Return(failed));
                        self.implicit_cleanup_returns
                            .push((self.current, expr.span));
                        self.current = success;
                        locals.insert(name.clone(), Binding::Val(fresh));
                        continue;
                    }
                    let (value, ty) = self.expr(expr, locals)?;
                    if matches!(statement, Statement::Var(..)) {
                        let place = self.place(ty);
                        self.emit_effect(Operation::PlaceInit(place, value), *span);
                        locals.insert(name.clone(), Binding::Var(place));
                    } else {
                        locals.insert(name.clone(), Binding::Val(value));
                    }
                }
                Statement::Assign(name, span, expr) => {
                    let binding = locals
                        .get(name)
                        .copied()
                        .ok_or_else(|| Diagnostic::new(*span, format!("unknown local {name}")))?;
                    let (value, ty) = self.expr(expr, locals)?;
                    match binding {
                        Binding::Var(place) => {
                            self.require(ty, self.place_types[place.0], expr.span)?;
                            self.emit_effect(Operation::Store(place, value), *span);
                        }
                        Binding::Ref(reference, ParamMode::Exclusive) => {
                            self.require(ty, Type::I64, expr.span)?;
                            self.emit_effect(Operation::StoreRef(reference, value), *span);
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                *span,
                                format!("cannot assign to val {name}"),
                            ));
                        }
                    }
                }
                Statement::While(condition, loop_body) => {
                    if let Some(tail) = &loop_body.tail {
                        return Err(Diagnostic::new(
                            tail.span,
                            "while body cannot end in a value expression; add a semicolon",
                        ));
                    }
                    let head = self.block();
                    let body_block = self.block();
                    let exit = self.block();
                    self.terminate(Terminator::Jump(head, vec![]));
                    self.current = head;
                    let (cond, ty) = self.expr(condition, locals)?;
                    self.require(ty, Type::Bool, condition.span)?;
                    self.terminate(Terminator::Branch {
                        condition: cond,
                        then_block: body_block,
                        else_block: exit,
                    });
                    self.current = body_block;
                    self.body(loop_body, &mut locals.clone())?;
                    self.terminate(Terminator::Jump(head, vec![]));
                    self.current = exit;
                }
                Statement::Expr(expr) => {
                    if let ExprKind::QualifiedCall(name, method, args) = &expr.kind
                        && method == "close"
                        && args.is_empty()
                        && let Some(Binding::Val(value)) = locals.get(name).copied()
                        && matches!(
                            self.value_types[value.0],
                            Type::SenderI64 | Type::ReceiverI64
                        )
                    {
                        self.emit_effect(Operation::CloseHandle(value), expr.span);
                    } else {
                        self.expr(expr, locals)?;
                    }
                }
            }
        }
        body.tail
            .as_ref()
            .map(|expr| self.expr(expr, locals))
            .transpose()
    }

    fn pair(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
        locals: &HashMap<String, Binding>,
    ) -> Result<(TypedValue, TypedValue), Diagnostic> {
        let a = self.expr(lhs, locals)?;
        let b = self.expr(rhs, locals)?;
        Ok((a, b))
    }

    fn expr(
        &mut self,
        expr: &Expr,
        locals: &HashMap<String, Binding>,
    ) -> Result<(ValueId, Type), Diagnostic> {
        let (operation, ty) = match &expr.kind {
            ExprKind::Int(value) => (Operation::Const(*value), Type::I64),
            ExprKind::Bool(value) => (Operation::Bool(*value), Type::Bool),
            ExprKind::Member(name, variant) => {
                if let Some(binding) = locals.get(name).copied() {
                    let ty = match binding {
                        Binding::Val(value) => self.value_types[value.0],
                        Binding::Var(place) => self.place_types[place.0],
                        Binding::Ref(_, _) => Type::RefI64,
                    };
                    let Type::Data(id) = ty else {
                        return Err(Diagnostic::new(
                            expr.span,
                            "field access requires a product",
                        ));
                    };
                    let index = self.data[id.0]
                        .fields
                        .iter()
                        .position(|(field, _)| field == variant)
                        .ok_or_else(|| {
                            Diagnostic::new(expr.span, format!("unknown field {variant}"))
                        })?;
                    let field_type = self.data[id.0].fields[index].1;
                    let operation = match binding {
                        Binding::Val(value) => Operation::Field(value, index),
                        Binding::Var(place) if matches!(field_type, Type::Data(_)) => {
                            let whole = self.emit(Operation::Take(place), ty, expr.span);
                            Operation::Field(whole, index)
                        }
                        Binding::Var(place) => Operation::ReadField(place, index),
                        Binding::Ref(_, _) => unreachable!("data reference is unsupported"),
                    };
                    (operation, field_type)
                } else {
                    let id = self.data_names.get(name).copied().ok_or_else(|| {
                        Diagnostic::new(expr.span, format!("unknown data type {name}"))
                    })?;
                    let tag = self.data[id.0]
                        .variants
                        .iter()
                        .position(|candidate| candidate == variant)
                        .ok_or_else(|| {
                            Diagnostic::new(expr.span, format!("unknown variant {name}.{variant}"))
                        })?;
                    if self.data[id.0].payloads[tag].is_some() {
                        return Err(Diagnostic::new(expr.span, "variant requires a payload"));
                    }
                    (Operation::Variant(id, tag, None), Type::Data(id))
                }
            }
            ExprKind::QualifiedCall(name, member, args) => {
                if let Some(Binding::Val(endpoint)) = locals.get(name).copied() {
                    match (self.value_types[endpoint.0], member.as_str()) {
                        (Type::SenderI64, "send") => {
                            let [CallArgument::Value(message)] = args.as_slice() else {
                                return Err(Diagnostic::new(
                                    expr.span,
                                    "send requires one owned I64 value",
                                ));
                            };
                            let (message_value, message_type) = self.expr(message, locals)?;
                            self.require(message_type, Type::I64, message.span)?;
                            let outcome = self.data_names["Send<I64>"];
                            let sent = self.block();
                            let closed = self.block();
                            let join = self.block();
                            let result = self.block_param(join, Type::Data(outcome));
                            self.terminate(Terminator::ChannelSend {
                                sender: endpoint,
                                message: message_value,
                                sent,
                                closed,
                            });
                            self.current = sent;
                            let sent_value = self.emit(
                                Operation::Variant(outcome, 0, None),
                                Type::Data(outcome),
                                expr.span,
                            );
                            self.terminate(Terminator::Jump(join, vec![sent_value]));
                            self.current = closed;
                            let closed_value = self.emit(
                                Operation::Variant(outcome, 1, Some(message_value)),
                                Type::Data(outcome),
                                expr.span,
                            );
                            self.terminate(Terminator::Jump(join, vec![closed_value]));
                            self.current = join;
                            return Ok((result, Type::Data(outcome)));
                        }
                        (Type::ReceiverI64, "receive") => {
                            if !args.is_empty() {
                                return Err(Diagnostic::new(
                                    expr.span,
                                    "receive takes no arguments",
                                ));
                            }
                            let outcome = self.data_names["Receive<I64>"];
                            let value = self.block();
                            let item = self.block_param(value, Type::I64);
                            let end = self.block();
                            let join = self.block();
                            let result = self.block_param(join, Type::Data(outcome));
                            self.terminate(Terminator::ChannelReceive {
                                receiver: endpoint,
                                value,
                                end,
                            });
                            self.current = value;
                            let some_value = self.emit(
                                Operation::Variant(outcome, 0, Some(item)),
                                Type::Data(outcome),
                                expr.span,
                            );
                            self.terminate(Terminator::Jump(join, vec![some_value]));
                            self.current = end;
                            let end_value = self.emit(
                                Operation::Variant(outcome, 1, None),
                                Type::Data(outcome),
                                expr.span,
                            );
                            self.terminate(Terminator::Jump(join, vec![end_value]));
                            self.current = join;
                            return Ok((result, Type::Data(outcome)));
                        }
                        _ => {}
                    }
                }
                if self.data_names.contains_key(name) {
                    let [CallArgument::Value(payload)] = args.as_slice() else {
                        return Err(Diagnostic::new(
                            expr.span,
                            "variant constructor requires one owned payload",
                        ));
                    };
                    return self.expr(
                        &Expr {
                            kind: ExprKind::VariantCall(
                                name.clone(),
                                member.clone(),
                                Box::new(payload.clone()),
                            ),
                            span: expr.span,
                        },
                        locals,
                    );
                }
                return self.expr(
                    &Expr {
                        kind: ExprKind::Call(format!("{name}.{member}"), args.clone()),
                        span: expr.span,
                    },
                    locals,
                );
            }
            ExprKind::VariantCall(name, variant, payload) => {
                let id = self.data_names.get(name).copied().ok_or_else(|| {
                    Diagnostic::new(expr.span, format!("unknown data type {name}"))
                })?;
                let tag = self.data[id.0]
                    .variants
                    .iter()
                    .position(|candidate| candidate == variant)
                    .ok_or_else(|| {
                        Diagnostic::new(expr.span, format!("unknown variant {name}.{variant}"))
                    })?;
                let expected = self.data[id.0].payloads[tag]
                    .ok_or_else(|| Diagnostic::new(expr.span, "variant has no payload"))?;
                let (value, actual) = self.expr(payload, locals)?;
                self.require(actual, expected, payload.span)?;
                (Operation::Variant(id, tag, Some(value)), Type::Data(id))
            }
            ExprKind::Try(inner) => {
                let (value, ty) = self.expr(inner, locals)?;
                let Type::Data(id) = ty else {
                    return Err(Diagnostic::new(inner.span, "? requires Result"));
                };
                let data = &self.data[id.0];
                if !data.name.starts_with("Result<") || data.variants.len() != 2 {
                    return Err(Diagnostic::new(inner.span, "? requires Result"));
                }
                let ok_tag = data
                    .variants
                    .iter()
                    .position(|name| name == "Ok")
                    .ok_or_else(|| {
                        Diagnostic::new(inner.span, "Result requires Ok and Err variants")
                    })?;
                let err_tag = data
                    .variants
                    .iter()
                    .position(|name| name == "Err")
                    .ok_or_else(|| {
                        Diagnostic::new(inner.span, "Result requires Ok and Err variants")
                    })?;
                let ok_ty = data.payloads[ok_tag]
                    .ok_or_else(|| Diagnostic::new(inner.span, "Result.Ok requires a payload"))?;
                if data.payloads[err_tag].is_none() {
                    return Err(Diagnostic::new(inner.span, "Result.Err requires a payload"));
                }
                if self.return_type != ty {
                    return Err(Diagnostic::new(
                        expr.span,
                        "? requires the function to return the same Result type",
                    ));
                }
                let success = self.block();
                let payload = self.block_param(success, ok_ty);
                let failure = self.block();
                let original = self.block_param(failure, ty);
                let mut arms = vec![
                    MatchArm {
                        target: failure,
                        binding: MatchBinding::Whole,
                    };
                    2
                ];
                arms[ok_tag] = MatchArm {
                    target: success,
                    binding: MatchBinding::Payload,
                };
                self.terminate(Terminator::Match {
                    subject: value,
                    arms,
                });
                self.current = failure;
                self.terminate(Terminator::Return(original));
                self.implicit_cleanup_returns
                    .push((self.current, expr.span));
                self.current = success;
                return Ok((payload, ok_ty));
            }
            ExprKind::Name(name) => {
                let binding = locals
                    .get(name)
                    .copied()
                    .ok_or_else(|| Diagnostic::new(expr.span, format!("unknown local {name}")))?;
                return Ok(match binding {
                    Binding::Val(id) => {
                        let ty = self.value_types[id.0];
                        if matches!(ty, Type::Data(_)) {
                            (self.emit(Operation::Move(id), ty, expr.span), ty)
                        } else {
                            (id, ty)
                        }
                    }
                    Binding::Var(place) => {
                        let ty = self.place_types[place.0];
                        let operation = if matches!(ty, Type::Data(_)) {
                            Operation::Take(place)
                        } else {
                            Operation::Load(place)
                        };
                        (self.emit(operation, ty, expr.span), ty)
                    }
                    Binding::Ref(reference, _) => (
                        self.emit(Operation::LoadRef(reference), Type::I64, expr.span),
                        Type::I64,
                    ),
                });
            }
            ExprKind::Copy(inner) => {
                if let ExprKind::Name(name) = &inner.kind {
                    let binding = locals.get(name).copied().ok_or_else(|| {
                        Diagnostic::new(inner.span, format!("unknown local {name}"))
                    })?;
                    match binding {
                        Binding::Val(value)
                            if matches!(self.value_types[value.0], Type::Data(_)) =>
                        {
                            let ty = self.value_types[value.0];
                            return Ok((self.emit(Operation::Copy(value), ty, expr.span), ty));
                        }
                        Binding::Var(place)
                            if matches!(self.place_types[place.0], Type::Data(_)) =>
                        {
                            let ty = self.place_types[place.0];
                            return Ok((self.emit(Operation::CopyPlace(place), ty, expr.span), ty));
                        }
                        _ => {}
                    }
                }
                let (value, ty) = self.expr(inner, locals)?;
                if matches!(ty, Type::Data(_)) {
                    return Ok((self.emit(Operation::Copy(value), ty, expr.span), ty));
                }
                return Ok((value, ty));
            }
            ExprKind::Neg(rhs) => {
                let (value, ty) = self.expr(rhs, locals)?;
                self.require(ty, Type::I64, rhs.span)?;
                (Operation::Neg(value), Type::I64)
            }
            ExprKind::Add(lhs, rhs) => {
                let ((a, ta), (b, tb)) = self.pair(lhs, rhs, locals)?;
                self.require(ta, Type::I64, lhs.span)?;
                self.require(tb, Type::I64, rhs.span)?;
                (Operation::Add(a, b), Type::I64)
            }
            ExprKind::Sub(lhs, rhs) => {
                let ((a, ta), (b, tb)) = self.pair(lhs, rhs, locals)?;
                self.require(ta, Type::I64, lhs.span)?;
                self.require(tb, Type::I64, rhs.span)?;
                (Operation::Sub(a, b), Type::I64)
            }
            ExprKind::Mul(lhs, rhs) => {
                let ((a, ta), (b, tb)) = self.pair(lhs, rhs, locals)?;
                self.require(ta, Type::I64, lhs.span)?;
                self.require(tb, Type::I64, rhs.span)?;
                (Operation::Mul(a, b), Type::I64)
            }
            ExprKind::Eq(lhs, rhs) => {
                let ((a, ta), (b, tb)) = self.pair(lhs, rhs, locals)?;
                if ta != tb {
                    return Err(Diagnostic::new(
                        expr.span,
                        "equality operands must have the same type",
                    ));
                }
                if let Type::Data(id) = ta
                    && (!self.data[id.0].fields.is_empty()
                        || self.data[id.0].payloads.iter().any(Option::is_some))
                {
                    return Err(Diagnostic::new(
                        expr.span,
                        "product equality and payload sum equality are not implemented",
                    ));
                }
                (Operation::Eq(a, b), Type::Bool)
            }
            ExprKind::Lt(lhs, rhs) => {
                let ((a, ta), (b, tb)) = self.pair(lhs, rhs, locals)?;
                self.require(ta, Type::I64, lhs.span)?;
                self.require(tb, Type::I64, rhs.span)?;
                (Operation::Lt(a, b), Type::Bool)
            }
            ExprKind::Call(name, args) => {
                if let Some(id) = self.data_names.get(name).copied() {
                    let fields = &self.data[id.0].fields;
                    if fields.is_empty() {
                        return Err(Diagnostic::new(
                            expr.span,
                            "sum type is not a product constructor",
                        ));
                    }
                    if args.len() != fields.len() {
                        return Err(Diagnostic::new(
                            expr.span,
                            format!("{name} expects {} fields, got {}", fields.len(), args.len()),
                        ));
                    }
                    let expected = fields.iter().map(|(_, ty)| *ty).collect::<Vec<_>>();
                    let mut values = Vec::with_capacity(args.len());
                    for (arg, ty) in args.iter().zip(expected) {
                        let CallArgument::Value(value) = arg else {
                            return Err(Diagnostic::new(
                                expr.span,
                                "product constructor requires owned fields",
                            ));
                        };
                        let (id, actual) = self.expr(value, locals)?;
                        self.require(actual, ty, value.span)?;
                        values.push(id);
                    }
                    let result =
                        self.emit(Operation::Product(id, values), Type::Data(id), expr.span);
                    return Ok((result, Type::Data(id)));
                }
                let signature = self.functions.get(name).cloned().ok_or_else(|| {
                    Diagnostic::new(expr.span, format!("unknown function {name}"))
                })?;
                let expected = signature.params;
                if args.len() != expected.len() {
                    return Err(Diagnostic::new(
                        expr.span,
                        format!(
                            "{name} expects {} arguments, got {}",
                            expected.len(),
                            args.len()
                        ),
                    ));
                }
                let mut values = Vec::with_capacity(args.len());
                for (arg, (mode, param_type)) in args.iter().zip(expected) {
                    match (arg, mode) {
                        (CallArgument::Value(arg), ParamMode::Own) => {
                            let (value, ty) = self.expr(arg, locals)?;
                            self.require(ty, param_type, arg.span)?;
                            values.push(CallArg::Value(value));
                        }
                        (
                            CallArgument::Borrow {
                                name,
                                span,
                                mutable,
                            },
                            ParamMode::Shared | ParamMode::Exclusive,
                        ) => {
                            if *mutable != (mode == ParamMode::Exclusive) {
                                return Err(Diagnostic::new(
                                    *span,
                                    "borrow mutability does not match parameter",
                                ));
                            }
                            let binding = locals.get(name).copied().ok_or_else(|| {
                                Diagnostic::new(*span, format!("unknown local {name}"))
                            })?;
                            let call_arg = match binding {
                                Binding::Var(place) => {
                                    self.require(self.place_types[place.0], Type::I64, *span)?;
                                    CallArg::Borrow(place, *mutable)
                                }
                                Binding::Val(value) if !*mutable => {
                                    self.require(self.value_types[value.0], Type::I64, *span)?;
                                    let place = self.place(Type::I64);
                                    self.emit_effect(Operation::PlaceInit(place, value), *span);
                                    CallArg::Borrow(place, false)
                                }
                                Binding::Ref(value, source_mode)
                                    if !*mutable || source_mode == ParamMode::Exclusive =>
                                {
                                    CallArg::Ref(value, *mutable)
                                }
                                _ => {
                                    return Err(Diagnostic::new(
                                        *span,
                                        "cannot mutably borrow this binding",
                                    ));
                                }
                            };
                            values.push(call_arg);
                        }
                        (CallArgument::Value(arg), _) => {
                            return Err(Diagnostic::new(arg.span, "expected explicit borrow"));
                        }
                        (CallArgument::Borrow { span, .. }, _) => {
                            return Err(Diagnostic::new(*span, "expected owned argument"));
                        }
                    }
                }
                check_call_aliases(&values)
                    .map_err(|message| Diagnostic::new(expr.span, message))?;
                (Operation::Call(signature.id, values), signature.return_type)
            }
            ExprKind::If(condition, then_body, else_body) => {
                let (cond, ty) = self.expr(condition, locals)?;
                self.require(ty, Type::Bool, condition.span)?;
                let then_block = self.block();
                let else_block = self.block();
                self.terminate(Terminator::Branch {
                    condition: cond,
                    then_block,
                    else_block,
                });
                self.current = then_block;
                let (then_value, then_type) = self
                    .body(then_body, &mut locals.clone())?
                    .expect("parser requires an if-arm tail");
                let then_end = self.current;
                self.current = else_block;
                let (else_value, else_type) = self
                    .body(else_body, &mut locals.clone())?
                    .expect("parser requires an if-arm tail");
                let else_end = self.current;
                if then_type != else_type {
                    return Err(Diagnostic::new(
                        expr.span,
                        "if branches must have the same type",
                    ));
                }
                let join = self.block();
                let result = self.block_param(join, then_type);
                self.current = then_end;
                self.terminate(Terminator::Jump(join, vec![then_value]));
                self.current = else_end;
                self.terminate(Terminator::Jump(join, vec![else_value]));
                self.current = join;
                return Ok((result, then_type));
            }
            ExprKind::With { name, value, body } => {
                return self
                    .with_body(name, expr.span, value, body, locals)?
                    .ok_or_else(|| Diagnostic::new(expr.span, "with expression requires a value"));
            }
            ExprKind::When(subject, arms) => {
                let (subject_value, subject_type) = self.expr(subject, locals)?;
                let Type::Data(data_id) = subject_type else {
                    return Err(Diagnostic::new(subject.span, "when requires a data value"));
                };
                let data = &self.data[data_id.0];
                if data.variants.is_empty() {
                    return Err(Diagnostic::new(subject.span, "when requires a sum type"));
                }
                let mut ordered = vec![None; data.variants.len()];
                for arm in arms {
                    if arm.type_name != data.name {
                        return Err(Diagnostic::new(
                            arm.span,
                            "when arm has the wrong data type",
                        ));
                    }
                    let tag = data
                        .variants
                        .iter()
                        .position(|name| name == &arm.variant)
                        .ok_or_else(|| Diagnostic::new(arm.span, "unknown when variant"))?;
                    if ordered[tag].is_some() {
                        return Err(Diagnostic::new(arm.span, "duplicate when variant"));
                    }
                    if arm.binding.is_some() != data.payloads[tag].is_some() {
                        return Err(Diagnostic::new(
                            arm.span,
                            "when payload binding does not match variant",
                        ));
                    }
                    ordered[tag] = Some(arm);
                }
                if let Some(tag) = ordered.iter().position(Option::is_none) {
                    return Err(Diagnostic::new(
                        expr.span,
                        format!(
                            "non-exhaustive when: missing {}.{}",
                            data.name, data.variants[tag]
                        ),
                    ));
                }
                let payload_types = data.payloads.clone();
                let targets = (0..ordered.len()).map(|_| self.block()).collect::<Vec<_>>();
                let payload_values = targets
                    .iter()
                    .zip(&payload_types)
                    .map(|(target, payload)| payload.map(|ty| self.block_param(*target, ty)))
                    .collect::<Vec<_>>();
                self.terminate(Terminator::Match {
                    subject: subject_value,
                    arms: targets
                        .iter()
                        .zip(&payload_types)
                        .map(|(target, payload)| MatchArm {
                            target: *target,
                            binding: if payload.is_some() {
                                MatchBinding::Payload
                            } else {
                                MatchBinding::Discard
                            },
                        })
                        .collect(),
                });
                let mut arm_results = Vec::with_capacity(ordered.len());
                for (tag, arm) in ordered.into_iter().enumerate() {
                    let arm = arm.expect("coverage checked");
                    self.current = targets[tag];
                    let mut arm_locals = locals.clone();
                    if let (Some((name, span)), Some(value)) = (&arm.binding, payload_values[tag])
                        && arm_locals
                            .insert(name.clone(), Binding::Val(value))
                            .is_some()
                    {
                        return Err(Diagnostic::new(*span, "duplicate pattern binding"));
                    }
                    let (value, ty) = self
                        .body(&arm.body, &mut arm_locals)?
                        .expect("parser requires when-arm tail");
                    arm_results.push((self.current, value, ty));
                }
                let result_type = arm_results[0].2;
                if arm_results.iter().any(|(_, _, ty)| *ty != result_type) {
                    return Err(Diagnostic::new(
                        expr.span,
                        "when arms must have the same type",
                    ));
                }
                let join = self.block();
                let result = self.block_param(join, result_type);
                for (end, value, _) in arm_results {
                    self.current = end;
                    self.terminate(Terminator::Jump(join, vec![value]));
                }
                self.current = join;
                return Ok((result, result_type));
            }
        };
        let value = self.emit(operation, ty, expr.span);
        Ok((value, ty))
    }

    fn finish(self, name: String) -> Result<(Function, CleanupSites), Diagnostic> {
        let mut blocks = Vec::with_capacity(self.blocks.len());
        for block in self.blocks {
            let Some(terminator) = block.terminator else {
                return Err(Diagnostic::new(
                    Span { start: 0, end: 0 },
                    "internal lowering left a block without a terminator",
                ));
            };
            blocks.push(Block {
                params: block.params,
                instructions: block.instructions,
                terminator,
            });
        }
        Ok((
            Function {
                name,
                blocks,
                value_types: self.value_types,
                place_types: self.place_types,
                param_modes: self.param_modes,
                return_type: self.return_type,
            },
            CleanupSites {
                returns: self.implicit_cleanup_returns,
                scopes: self.scoped_cleanup_exits,
            },
        ))
    }
}

fn register_result_application(
    application: &TypeName,
    span: Span,
    data: &mut Vec<DataDef>,
    data_names: &mut HashMap<String, DataId>,
) -> Result<DataId, Diagnostic> {
    let TypeName::Result(ok, err) = application else {
        unreachable!("only Result applications are registered here")
    };
    let name = application.key();
    if let Some(id) = data_names.get(&name) {
        return Ok(*id);
    }
    let mut payload = |ty: &TypeName| -> Result<Type, Diagnostic> {
        match ty {
            TypeName::I64 => Ok(Type::I64),
            TypeName::Bool => Ok(Type::Bool),
            TypeName::SenderI64 | TypeName::ReceiverI64 => Err(Diagnostic::new(
                span,
                "Result cannot carry channel endpoints yet",
            )),
            TypeName::Named(name) if name == "ResourceError" => Ok(Type::ResourceError),
            TypeName::Named(name) => data_names
                .get(name)
                .copied()
                .map(Type::Data)
                .ok_or_else(|| Diagnostic::new(span, format!("unknown data type {name}"))),
            TypeName::Result(_, _) => {
                register_result_application(ty, span, data, data_names).map(Type::Data)
            }
        }
    };
    let ok = payload(ok)?;
    let err = payload(err)?;
    let id = DataId(data.len());
    data_names.insert(name.clone(), id);
    data.push(DataDef {
        name,
        variants: vec!["Ok".into(), "Err".into()],
        payloads: vec![Some(ok), Some(err)],
        fields: vec![],
    });
    Ok(id)
}

pub fn lower(ast: &syntax::Program) -> Result<Program, Diagnostic> {
    if let Some(import) = ast.imports.first() {
        return Err(Diagnostic::new(
            import.span,
            "imports require a phoebe.toml project manifest",
        ));
    }
    let mut data = Vec::with_capacity(ast.data.len());
    let mut data_names = HashMap::new();
    for declaration in &ast.data {
        if matches!(
            declaration.name.as_str(),
            "I64"
                | "Bool"
                | "Result"
                | "ResourceError"
                | "Sender"
                | "Receiver"
                | "Send"
                | "Receive"
        ) || data_names.contains_key(&declaration.name)
        {
            return Err(Diagnostic::new(declaration.span, "duplicate data type"));
        }
        let mut variants = Vec::with_capacity(declaration.variants.len());
        let mut payloads = Vec::with_capacity(declaration.variants.len());
        for variant in &declaration.variants {
            if variants.contains(&variant.name) {
                return Err(Diagnostic::new(variant.span, "duplicate variant"));
            }
            let payload = match variant.payload.as_ref().map(|field| &field.ty) {
                None => None,
                Some(TypeName::I64) => Some(Type::I64),
                Some(TypeName::Bool) => Some(Type::Bool),
                Some(TypeName::SenderI64 | TypeName::ReceiverI64) => {
                    return Err(Diagnostic::new(
                        variant.span,
                        "channel endpoints cannot be stored in data yet",
                    ));
                }
                Some(TypeName::Named(name)) if name == "ResourceError" => Some(Type::ResourceError),
                Some(TypeName::Named(name)) => Some(
                    data_names
                        .get(name)
                        .copied()
                        .map(Type::Data)
                        .ok_or_else(|| {
                            Diagnostic::new(
                                variant.span,
                                format!("variant payload type `{name}` must be declared earlier"),
                            )
                        })?,
                ),
                Some(ty @ TypeName::Result(_, _)) => Some(Type::Data(register_result_application(
                    ty,
                    variant.span,
                    &mut data,
                    &mut data_names,
                )?)),
            };
            variants.push(variant.name.clone());
            payloads.push(payload);
        }
        let mut fields = Vec::with_capacity(declaration.fields.len());
        for field in &declaration.fields {
            if fields.iter().any(|(name, _)| name == &field.name) {
                return Err(Diagnostic::new(field.span, "duplicate field"));
            }
            let ty =
                match &field.ty {
                    TypeName::I64 => Type::I64,
                    TypeName::Bool => Type::Bool,
                    TypeName::SenderI64 | TypeName::ReceiverI64 => {
                        return Err(Diagnostic::new(
                            field.span,
                            "channel endpoints cannot be stored in data yet",
                        ));
                    }
                    TypeName::Named(name) if name == "ResourceError" => Type::ResourceError,
                    TypeName::Named(name) => data_names
                        .get(name)
                        .copied()
                        .map(Type::Data)
                        .ok_or_else(|| {
                            Diagnostic::new(
                                field.span,
                                format!("product field type `{name}` must be declared earlier"),
                            )
                        })?,
                    ty @ TypeName::Result(_, _) => Type::Data(register_result_application(
                        ty,
                        field.span,
                        &mut data,
                        &mut data_names,
                    )?),
                };
            fields.push((field.name.clone(), ty));
        }
        data_names.insert(declaration.name.clone(), DataId(data.len()));
        data.push(DataDef {
            name: declaration.name.clone(),
            variants,
            payloads,
            fields,
        });
    }
    for (used, name, variants, payloads) in [
        (
            ast.uses_send_outcome,
            "Send<I64>",
            vec!["Sent", "Closed"],
            vec![None, Some(Type::I64)],
        ),
        (
            ast.uses_receive_outcome,
            "Receive<I64>",
            vec!["Value", "End"],
            vec![Some(Type::I64), None],
        ),
    ] {
        if !used {
            continue;
        }
        data_names.insert(name.to_owned(), DataId(data.len()));
        data.push(DataDef {
            name: name.to_owned(),
            variants: variants.into_iter().map(str::to_owned).collect(),
            payloads,
            fields: vec![],
        });
    }
    for (application, span) in &ast.applied_results {
        register_result_application(application, *span, &mut data, &mut data_names)?;
    }
    let resolve = |name: &TypeName, span: Span| -> Result<Type, Diagnostic> {
        match name {
            TypeName::I64 => Ok(Type::I64),
            TypeName::Bool => Ok(Type::Bool),
            TypeName::SenderI64 => Ok(Type::SenderI64),
            TypeName::ReceiverI64 => Ok(Type::ReceiverI64),
            TypeName::Named(name) if name == "ResourceError" => Ok(Type::ResourceError),
            TypeName::Named(name) => data_names
                .get(name)
                .copied()
                .map(Type::Data)
                .ok_or_else(|| Diagnostic::new(span, format!("unknown data type {name}"))),
            TypeName::Result(_, _) => data_names
                .get(&name.key())
                .copied()
                .map(Type::Data)
                .ok_or_else(|| Diagnostic::new(span, "unknown Result application")),
        }
    };
    let mut functions = HashMap::new();
    for (index, function) in ast.functions.iter().enumerate() {
        if data_names.contains_key(&function.name) {
            return Err(Diagnostic::new(
                function.name_span,
                "function name conflicts with data type",
            ));
        }
        if function.params.len() > 4 {
            return Err(Diagnostic::new(
                function.name_span,
                "at most four parameters in this implementation slice",
            ));
        }
        if functions
            .insert(
                function.name.clone(),
                FunctionSignature {
                    id: FunctionId(index),
                    params: function
                        .params
                        .iter()
                        .map(|param| Ok((param.mode, resolve(&param.ty, param.span)?)))
                        .collect::<Result<Vec<_>, Diagnostic>>()?,
                    return_type: resolve(&function.return_type, function.name_span)?,
                },
            )
            .is_some()
        {
            return Err(Diagnostic::new(
                function.name_span,
                format!("duplicate function {}", function.name),
            ));
        }
    }
    let mut lowered = Vec::with_capacity(ast.functions.len());
    let mut cleanup_sites = Vec::with_capacity(ast.functions.len());
    for function in &ast.functions {
        let mut lowerer = Lowerer::new(
            &functions,
            &data,
            &data_names,
            function
                .params
                .iter()
                .map(|param| Ok((param.mode, resolve(&param.ty, param.span)?)))
                .collect::<Result<Vec<_>, Diagnostic>>()?,
            resolve(&function.return_type, function.name_span)?,
        );
        let mut locals = HashMap::new();
        for (index, param) in function.params.iter().enumerate() {
            let binding = match param.mode {
                ParamMode::Own => Binding::Val(ValueId(index)),
                mode => Binding::Ref(ValueId(index), mode),
            };
            if locals.insert(param.name.clone(), binding).is_some() {
                return Err(Diagnostic::new(
                    param.span,
                    format!("duplicate parameter {}", param.name),
                ));
            }
        }
        let (value, ty) = lowerer
            .body(&function.body, &mut locals)?
            .expect("parser requires a function tail");
        lowerer.require(
            ty,
            resolve(&function.return_type, function.name_span)?,
            function.body.tail.as_ref().expect("tail").span,
        )?;
        lowerer.terminate(Terminator::Return(value));
        let (function, cleanup) = lowerer.finish(function.name.clone())?;
        lowered.push(function);
        cleanup_sites.push(cleanup);
    }
    let mut program = Program {
        data,
        functions: lowered,
    };
    verify_structure(&program)
        .map_err(|message| Diagnostic::new(Span { start: 0, end: 0 }, message))?;
    verify_ownership(&program).map_err(|error| Diagnostic::new(error.span, error.message))?;
    // A single abstract flow pass decides lexical closes in control-flow
    // order. Provisional closes alter the abstract state, so nested aliases
    // close once. The final Core contains only the closes actually needed.
    for ((function, exits), source_function) in program
        .functions
        .iter_mut()
        .zip(&cleanup_sites)
        .zip(&ast.functions)
    {
        if exits.scopes.is_empty() {
            continue;
        }
        let returns = function
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(index, block)| {
                matches!(block.terminator, Terminator::Return(_)).then_some(BlockId(index))
            })
            .collect::<Vec<_>>();
        let Some((graph, values)) = capability_graph(function, &returns, &exits.scopes) else {
            continue;
        };
        let facts = capability_flow::verify(&graph)
            .map_err(|message| Diagnostic::new(source_function.name_span, message))?;
        let mut spans = vec![None; function.blocks.len()];
        for (block, _, span) in &exits.scopes {
            spans[block.0] = Some(*span);
        }
        for (block, slot) in facts.lexical_closes {
            let span = spans[block].expect("lexical close belongs to a with exit");
            function.blocks[block].instructions.push(Instruction {
                result: None,
                operation: Operation::CloseHandle(values[slot.0]),
                span,
            });
        }
    }
    // Treat only source `?` returns as cleanup exits. The exact checker gives
    // each such exit its live slots; then the ordinary verifier checks the
    // completed Core, including every inserted close.
    for ((function, returns), source_function) in program
        .functions
        .iter_mut()
        .zip(&cleanup_sites)
        .zip(&ast.functions)
    {
        let cleanup_blocks = returns
            .returns
            .iter()
            .map(|(block, _)| *block)
            .collect::<Vec<_>>();
        if let Some((graph, values)) = capability_graph(function, &cleanup_blocks, &[]) {
            let facts = capability_flow::verify(&graph)
                .map_err(|message| Diagnostic::new(source_function.name_span, message))?;
            for exit in facts.failures {
                let span = returns
                    .returns
                    .iter()
                    .find(|(block, _)| block.0 == exit.block)
                    .map(|(_, span)| *span)
                    .expect("failure fact belongs to an implicit return");
                for slot in exit.live {
                    function.blocks[exit.block].instructions.push(Instruction {
                        result: None,
                        operation: Operation::CloseHandle(values[slot.0]),
                        span,
                    });
                }
            }
        }
    }
    verify_capability_edges(&program).map_err(|message| {
        let span = ast
            .functions
            .iter()
            .find(|function| message.starts_with(&format!("{}:", function.name)))
            .map(|function| function.name_span)
            .unwrap_or(Span { start: 0, end: 0 });
        Diagnostic::new(span, message)
    })?;
    Ok(program)
}
