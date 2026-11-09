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
        if matches!(value.kind, ExprKind::Borrow { .. }) {
            return self.borrow_body(name, span, value, body, locals);
        }
        let (owner, ty) = self.expr(value, locals)?;
        if self.capability_layout.leaves(ty) == 0 {
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
            && self.capability_layout.leaves(ty) > 0
        {
            return Err(Diagnostic::new(
                body.tail.as_ref().map_or(span, |tail| tail.span),
                "with cannot return its endpoint or a product that owns rights; move them to another owner inside the block",
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

    fn bind_pair_local(
        &mut self,
        locals: &mut HashMap<String, Binding>,
        name: &str,
        span: Span,
        value: ValueId,
        ty: Type,
        mutable: bool,
    ) {
        let binding = if mutable {
            let place = self.place(ty);
            self.emit_effect(Operation::PlaceInit(place, value), span);
            Binding::Var(place)
        } else {
            self.owned_binding(name, value, ty, span)
        };
        locals.insert(name.to_owned(), binding);
    }

    pub(super) fn body(
        &mut self,
        body: &Body,
        locals: &mut HashMap<String, Binding>,
    ) -> Result<Option<TypedValue>, Diagnostic> {
        for statement in &body.statements {
            match statement {
                Statement::ChannelVal {
                    mutable,
                    message,
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
                    let typed_message = if *message == TypeName::I64 {
                        None
                    } else {
                        Some(data::message_type(message, self.data_names, *span)?)
                    };
                    let sender_type = typed_message.map_or(Type::SenderI64, Type::TypedSender);
                    let receiver_type =
                        typed_message.map_or(Type::ReceiverI64, Type::TypedReceiver);
                    let success = self.block();
                    let send_value = self.block_param(success, sender_type);
                    let receive_value = self.block_param(success, receiver_type);
                    let failure = self.block();
                    let error = self.block_param(failure, Type::ResourceError);
                    self.terminate(match typed_message {
                        Some(message) => Terminator::TypedChannelNew {
                            message,
                            capacity: value,
                            success,
                            failure,
                            span: *span,
                        },
                        None => Terminator::ChannelNew {
                            capacity: value,
                            success,
                            failure,
                            span: *span,
                        },
                    });
                    self.current = failure;
                    let failed = self.emit(
                        Operation::Variant(result_id, 1, Some(error)),
                        self.return_type,
                        *span,
                    );
                    self.implicit_return(failed, *span);
                    self.current = success;
                    self.bind_pair_local(
                        locals,
                        &sender.0,
                        sender.1,
                        send_value,
                        sender_type,
                        *mutable,
                    );
                    self.bind_pair_local(
                        locals,
                        &receiver.0,
                        receiver.1,
                        receive_value,
                        receiver_type,
                        *mutable,
                    );
                }
                Statement::ReplyVal {
                    mutable,
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
                    self.bind_pair_local(
                        locals,
                        &right.0,
                        right.1,
                        right_value,
                        Type::ReplyToI64,
                        *mutable,
                    );
                    self.bind_pair_local(
                        locals,
                        &receiver.0,
                        receiver.1,
                        receiver_value,
                        Type::ReplyReceiverI64,
                        *mutable,
                    );
                }
                Statement::RequestChannelVal {
                    mutable,
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
                    self.bind_pair_local(
                        locals,
                        &sender.0,
                        sender.1,
                        send_value,
                        Type::RequestSenderI64,
                        *mutable,
                    );
                    self.bind_pair_local(
                        locals,
                        &receiver.0,
                        receiver.1,
                        receive_value,
                        Type::RequestReceiverI64,
                        *mutable,
                    );
                }
                Statement::UnpackVal {
                    mutable,
                    names,
                    value,
                    span,
                } => {
                    for (index, (name, name_span)) in names.iter().enumerate() {
                        if locals.contains_key(name)
                            || names[..index].iter().any(|(prior, _)| prior == name)
                        {
                            return Err(Diagnostic::new(
                                *name_span,
                                format!("duplicate local {name}"),
                            ));
                        }
                    }
                    let (owner, ty) = self.expr(value, locals)?;
                    let (field_types, request) = match ty {
                        Type::RequestI64 => (vec![Type::I64, Type::ReplyToI64], true),
                        Type::Data(id) if !self.data[id.0].fields.is_empty() => (
                            self.data[id.0]
                                .fields
                                .iter()
                                .map(|(_, ty)| *ty)
                                .collect::<Vec<_>>(),
                            false,
                        ),
                        _ => {
                            return Err(Diagnostic::new(
                                *span,
                                "unpack requires a request or nominal product",
                            ));
                        }
                    };
                    if names.len() != field_types.len() {
                        return Err(Diagnostic::new(
                            *span,
                            format!("unpack requires {} binding name(s)", field_types.len()),
                        ));
                    }
                    let next = self.block();
                    let params = field_types
                        .into_iter()
                        .map(|ty| self.block_param(next, ty))
                        .collect::<Vec<_>>();
                    self.terminate(if request {
                        Terminator::RequestUnpack {
                            request: owner,
                            next,
                        }
                    } else {
                        Terminator::ProductUnpack {
                            product: owner,
                            next,
                        }
                    });
                    self.current = next;
                    for ((name, name_span), param) in names.iter().zip(params) {
                        let ty = self.value_types[param.0];
                        self.bind_pair_local(locals, name, *name_span, param, ty, *mutable);
                    }
                }
                Statement::Spawn { child, args, span } => {
                    let result_id = self.resource_result_id(*span)?;
                    let signature = self.functions.get(child).cloned().ok_or_else(|| {
                        Diagnostic::new(*span, format!("unknown function {child}"))
                    })?;
                    if !matches!(signature.return_type, Type::I64 | Type::Unit)
                        || signature.params.len() != args.len()
                        || signature
                            .params
                            .iter()
                            .any(|(mode, _)| *mode != ParamMode::Own)
                    {
                        return Err(Diagnostic::new(
                            *span,
                            "spawn requires an I64- or Unit-returning function with owned arguments",
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
                                _ => self.expr(expr, locals)?,
                            }
                        } else {
                            self.expr(expr, locals)?
                        };
                        self.require(ty, *expected, expr.span)?;
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
                        let source_binding = match locals.get(source).copied() {
                            Some(binding @ (Binding::Val(_) | Binding::Var(_))) => binding,
                            _ => {
                                return Err(Diagnostic::new(
                                    inner.span,
                                    "fork requires a named endpoint",
                                ));
                            }
                        };
                        let ty = match source_binding {
                            Binding::Val(value) => self.value_types[value.0],
                            Binding::Var(place) => self.place_types[place.0],
                            Binding::Ref(_, _) => unreachable!(),
                        };
                        if !matches!(ty, Type::SenderI64 | Type::ReceiverI64) {
                            return Err(Diagnostic::new(
                                inner.span,
                                "fork requires Sender<I64> or Receiver<I64>",
                            ));
                        }
                        let result_id = self.resource_result_id(expr.span)?;
                        let endpoint = match source_binding {
                            Binding::Val(value) => value,
                            Binding::Var(place) => {
                                self.emit(Operation::Take(place), ty, inner.span)
                            }
                            Binding::Ref(_, _) => unreachable!(),
                        };
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
                        if let Binding::Var(place) = source_binding {
                            self.emit_effect(Operation::Store(place, endpoint), inner.span);
                        }
                        let failed = self.emit(
                            Operation::Variant(result_id, 1, Some(error)),
                            self.return_type,
                            expr.span,
                        );
                        self.implicit_return(failed, expr.span);
                        self.current = success;
                        if let Binding::Var(place) = source_binding {
                            self.emit_effect(Operation::Store(place, endpoint), inner.span);
                        }
                        if matches!(statement, Statement::Var(..)) {
                            let place = self.place(ty);
                            self.emit_effect(Operation::PlaceInit(place, fresh), *span);
                            locals.insert(name.clone(), Binding::Var(place));
                        } else {
                            locals.insert(name.clone(), self.owned_binding(name, fresh, ty, *span));
                        }
                        continue;
                    }
                    let (value, ty) = self.expr(expr, locals)?;
                    if matches!(statement, Statement::Var(..)) {
                        let place = self.place(ty);
                        self.emit_effect(Operation::PlaceInit(place, value), *span);
                        locals.insert(name.clone(), Binding::Var(place));
                    } else {
                        locals.insert(name.clone(), self.owned_binding(name, value, ty, *span));
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
                            if self.immutable_places[place.0] {
                                return Err(Diagnostic::new(
                                    *span,
                                    format!("cannot assign to val {name}"),
                                ));
                            }
                            self.require(ty, self.place_types[place.0], expr.span)?;
                            self.emit_effect(Operation::Store(place, value), *span);
                        }
                        Binding::Ref(reference, ParamMode::Exclusive) => {
                            let expected = self.value_types[reference.0]
                                .referent()
                                .expect("validated reference parameter");
                            self.require(ty, expected, expr.span)?;
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
                Statement::ArrayAssign(name, index, span, expr) => {
                    let Some(Binding::Var(place)) = locals.get(name).copied() else {
                        return Err(Diagnostic::new(
                            *span,
                            format!("array assignment requires a var binding {name}"),
                        ));
                    };
                    let Type::ArrayI64(len) = self.place_types[place.0] else {
                        return Err(Diagnostic::new(
                            *span,
                            "indexed assignment requires a fixed I64 array",
                        ));
                    };
                    if *index >= len {
                        return Err(Diagnostic::new(
                            *span,
                            "constant array index is out of bounds",
                        ));
                    }
                    let (value, ty) = self.expr(expr, locals)?;
                    self.require(ty, Type::I64, expr.span)?;
                    self.emit_effect(Operation::ArraySetPlace(place, *index, value), *span);
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
                    let close = if let ExprKind::QualifiedCall(name, method, args) = &expr.kind
                        && args.is_empty()
                        && let Some(binding @ (Binding::Val(_) | Binding::Var(_))) =
                            locals.get(name).copied()
                    {
                        let ty = match binding {
                            Binding::Val(value) => self.value_types[value.0],
                            Binding::Var(place) => self.place_types[place.0],
                            Binding::Ref(_, _) => unreachable!(),
                        };
                        let permitted = (method == "close"
                            && self.capability_layout.leaves(ty) > 0
                            && !matches!(ty, Type::Data(_)))
                            || (method == "close_caps" && matches!(ty, Type::Data(_)));
                        permitted.then_some((binding, ty))
                    } else {
                        None
                    };
                    if let Some((binding, ty)) = close {
                        let value = match binding {
                            Binding::Val(value) => value,
                            Binding::Var(place) => self.emit(Operation::Take(place), ty, expr.span),
                            Binding::Ref(_, _) => unreachable!(),
                        };
                        self.emit_effect(cleanup::close_operation(value, ty), expr.span);
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
