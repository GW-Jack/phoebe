//! Source statements and lexical cleanup sites.

use super::*;

impl Lowerer<'_> {
    pub(super) fn with_body(
        &mut self,
        name: &str,
        span: Span,
        value: &Expr,
        body: &Body,
        locals: &HashMap<String, Binding>,
    ) -> Result<Option<TypedValue>, Diagnostic> {
        let (owner, ty) = self.expr(value, locals)?;
        if !matches!(
            ty,
            Type::SenderI64
                | Type::ReceiverI64
                | Type::ReplyToI64
                | Type::ReplyReceiverI64
                | Type::RequestI64
                | Type::RequestSenderI64
                | Type::RequestReceiverI64
        ) {
            return Err(Diagnostic::new(
                value.span,
                "with requires a communication capability",
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
            && matches!(
                ty,
                Type::SenderI64
                    | Type::ReceiverI64
                    | Type::ReplyToI64
                    | Type::ReplyReceiverI64
                    | Type::RequestI64
                    | Type::RequestSenderI64
                    | Type::RequestReceiverI64
            )
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

    pub(super) fn body(
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
                    self.implicit_return(failed, *span);
                    self.current = success;
                    locals.insert(sender.0.clone(), Binding::Val(send_value));
                    locals.insert(receiver.0.clone(), Binding::Val(receive_value));
                }
                Statement::ReplyVal {
                    right,
                    receiver,
                    span,
                } => {
                    for (name, name_span) in [right, receiver] {
                        if locals.contains_key(name) || right.0 == receiver.0 {
                            return Err(Diagnostic::new(
                                *name_span,
                                format!("duplicate local {name}"),
                            ));
                        }
                    }
                    let result_id = self.resource_result_id(*span)?;
                    let success = self.block();
                    let right_value = self.block_param(success, Type::ReplyToI64);
                    let receiver_value = self.block_param(success, Type::ReplyReceiverI64);
                    let failure = self.block();
                    let error = self.block_param(failure, Type::ResourceError);
                    self.terminate(Terminator::ReplyNew { success, failure });
                    self.current = failure;
                    let failed = self.emit(
                        Operation::Variant(result_id, 1, Some(error)),
                        self.return_type,
                        *span,
                    );
                    self.implicit_return(failed, *span);
                    self.current = success;
                    locals.insert(right.0.clone(), Binding::Val(right_value));
                    locals.insert(receiver.0.clone(), Binding::Val(receiver_value));
                }
                Statement::RequestChannelVal {
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
                    let result_id = self.resource_result_id(*span)?;
                    let (value, ty) = self.expr(capacity, locals)?;
                    self.require(ty, Type::I64, capacity.span)?;
                    let success = self.block();
                    let send_value = self.block_param(success, Type::RequestSenderI64);
                    let receive_value = self.block_param(success, Type::RequestReceiverI64);
                    let failure = self.block();
                    let error = self.block_param(failure, Type::ResourceError);
                    self.terminate(Terminator::RequestChannelNew {
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
                    self.implicit_return(failed, *span);
                    self.current = success;
                    locals.insert(sender.0.clone(), Binding::Val(send_value));
                    locals.insert(receiver.0.clone(), Binding::Val(receive_value));
                }
                Statement::RequestUnpackVal {
                    value,
                    right,
                    request,
                    span,
                } => {
                    for (name, name_span) in [value, right] {
                        if locals.contains_key(name) || value.0 == right.0 {
                            return Err(Diagnostic::new(
                                *name_span,
                                format!("duplicate local {name}"),
                            ));
                        }
                    }
                    let (request, ty) = self.expr(request, locals)?;
                    self.require(ty, Type::RequestI64, *span)?;
                    let next = self.block();
                    let value_param = self.block_param(next, Type::I64);
                    let right_param = self.block_param(next, Type::ReplyToI64);
                    self.terminate(Terminator::RequestUnpack { request, next });
                    self.current = next;
                    locals.insert(value.0.clone(), Binding::Val(value_param));
                    locals.insert(right.0.clone(), Binding::Val(right_param));
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
                                        Type::Data(_)
                                            | Type::SenderI64
                                            | Type::ReceiverI64
                                            | Type::ReplyToI64
                                            | Type::ReplyReceiverI64
                                            | Type::RequestI64
                                            | Type::RequestSenderI64
                                            | Type::RequestReceiverI64
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
                        if matches!(
                            ty,
                            Type::Data(_)
                                | Type::SenderI64
                                | Type::ReceiverI64
                                | Type::ReplyToI64
                                | Type::ReplyReceiverI64
                                | Type::RequestI64
                                | Type::RequestSenderI64
                                | Type::RequestReceiverI64
                        ) && !matches!(expr.kind, ExprKind::Name(_))
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
                        span: *span,
                    });
                    self.current = failure;
                    let failed = self.emit(
                        Operation::Variant(result_id, 1, Some(error)),
                        self.return_type,
                        *span,
                    );
                    self.implicit_return(failed, *span);
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
                Statement::Scope { span, body } => {
                    if let Some(tail) = &body.tail {
                        return Err(Diagnostic::new(
                            tail.span,
                            "scope body cannot end in a value expression; add a semicolon",
                        ));
                    }
                    if !spawn_scan::body_can_spawn(body, self.spawn_effects) {
                        self.body(body, &mut locals.clone())?;
                        continue;
                    }
                    let scope = TaskScopeId(self.next_task_scope);
                    self.next_task_scope += 1;
                    self.emit_effect(Operation::ScopeEnter(scope), *span);
                    self.task_scope_stack.push((scope, *span));
                    self.body(body, &mut locals.clone())?;
                    self.task_scope_stack.pop();
                    let resume = self.block();
                    self.terminate(Terminator::ScopeJoin {
                        scope,
                        resume,
                        span: *span,
                    });
                    self.current = resume;
                }
                Statement::Yield(span) => {
                    let resume = self.block();
                    self.terminate(Terminator::Yield {
                        resume,
                        span: *span,
                        automatic: false,
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
                        self.implicit_return(failed, expr.span);
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
                    // A task running a pure CPU loop must eventually give the
                    // scheduler a chance to observe cancellation and run its
                    // siblings. Keep the budget local to this loop so nested
                    // loops do not reset each other's progress.
                    const BACKEDGE_BUDGET: i64 = 1024;
                    let budget = if self.automatic_checkpoints {
                        let budget = self.place(Type::I64);
                        let initial =
                            self.emit(Operation::Const(BACKEDGE_BUDGET), Type::I64, condition.span);
                        self.emit_effect(Operation::PlaceInit(budget, initial), condition.span);
                        Some(budget)
                    } else {
                        None
                    };
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
                    if let Some(budget) = budget {
                        let remaining =
                            self.emit(Operation::Load(budget), Type::I64, condition.span);
                        let one = self.emit(Operation::Const(1), Type::I64, condition.span);
                        let next =
                            self.emit(Operation::Sub(remaining, one), Type::I64, condition.span);
                        self.emit_effect(Operation::Store(budget, next), condition.span);
                        let zero = self.emit(Operation::Const(0), Type::I64, condition.span);
                        let due = self.emit(Operation::Eq(next, zero), Type::Bool, condition.span);
                        let checkpoint = self.block();
                        self.terminate(Terminator::Branch {
                            condition: due,
                            then_block: checkpoint,
                            else_block: head,
                        });
                        self.current = checkpoint;
                        let reset =
                            self.emit(Operation::Const(BACKEDGE_BUDGET), Type::I64, condition.span);
                        self.emit_effect(Operation::Store(budget, reset), condition.span);
                        self.terminate(Terminator::Yield {
                            resume: head,
                            span: condition.span,
                            automatic: true,
                        });
                    } else {
                        self.terminate(Terminator::Jump(head, vec![]));
                    }
                    self.current = exit;
                }
                Statement::Expr(expr) => {
                    if let ExprKind::QualifiedCall(name, method, args) = &expr.kind
                        && method == "close"
                        && args.is_empty()
                        && let Some(Binding::Val(value)) = locals.get(name).copied()
                        && matches!(
                            self.value_types[value.0],
                            Type::SenderI64
                                | Type::ReceiverI64
                                | Type::RequestSenderI64
                                | Type::RequestReceiverI64
                        )
                    {
                        self.emit_effect(Operation::CloseHandle(value), expr.span);
                    } else if let ExprKind::QualifiedCall(name, method, args) = &expr.kind
                        && method == "close"
                        && args.is_empty()
                        && let Some(Binding::Val(value)) = locals.get(name).copied()
                        && matches!(
                            self.value_types[value.0],
                            Type::ReplyToI64 | Type::ReplyReceiverI64
                        )
                    {
                        self.emit_effect(Operation::CloseReply(value), expr.span);
                    } else if let ExprKind::QualifiedCall(name, method, args) = &expr.kind
                        && method == "close"
                        && args.is_empty()
                        && let Some(Binding::Val(value)) = locals.get(name).copied()
                        && self.value_types[value.0] == Type::RequestI64
                    {
                        self.emit_effect(Operation::CloseRequest(value), expr.span);
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
}
