//! Source expressions and typed value construction.

use super::*;

impl Lowerer<'_> {
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

    pub(super) fn expr(
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
                        (Type::ReplyToI64, "complete") => {
                            let [CallArgument::Value(message)] = args.as_slice() else {
                                return Err(Diagnostic::new(
                                    expr.span,
                                    "complete requires one owned I64 value",
                                ));
                            };
                            let (message_value, message_type) = self.expr(message, locals)?;
                            self.require(message_type, Type::I64, message.span)?;
                            let outcome = self.data_names["Complete<I64>"];
                            let delivered = self.block();
                            let closed = self.block();
                            let join = self.block();
                            let result = self.block_param(join, Type::Data(outcome));
                            self.terminate(Terminator::ReplyComplete {
                                right: endpoint,
                                value: message_value,
                                delivered,
                                closed,
                            });
                            self.current = delivered;
                            let delivered_value = self.emit(
                                Operation::Variant(outcome, 0, None),
                                Type::Data(outcome),
                                expr.span,
                            );
                            self.terminate(Terminator::Jump(join, vec![delivered_value]));
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
                        (Type::ReplyReceiverI64, "receive") => {
                            if !args.is_empty() {
                                return Err(Diagnostic::new(
                                    expr.span,
                                    "receive takes no arguments",
                                ));
                            }
                            let outcome = self.data_names["Reply<I64>"];
                            let value = self.block();
                            let item = self.block_param(value, Type::I64);
                            let end = self.block();
                            let join = self.block();
                            let result = self.block_param(join, Type::Data(outcome));
                            self.terminate(Terminator::ReplyReceive {
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
                        (Type::RequestSenderI64, "send")
                        | (Type::RequestReceiverI64, "receive") => {
                            return Err(Diagnostic::new(
                                expr.span,
                                "request traffic requires an exhaustive when with SendRequest or ReceiveRequest arms",
                            ));
                        }
                        (Type::RequestI64, "unpack") => {
                            return Err(Diagnostic::new(
                                expr.span,
                                "unpack a request with val (value, right) = request.unpack();",
                            ));
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
                self.implicit_return(original, expr.span);
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
            ExprKind::Not(rhs) => {
                let (value, ty) = self.expr(rhs, locals)?;
                self.require(ty, Type::Bool, rhs.span)?;
                (Operation::Not(value), Type::Bool)
            }
            ExprKind::And(lhs, rhs) | ExprKind::Or(lhs, rhs) => {
                let (condition, ty) = self.expr(lhs, locals)?;
                self.require(ty, Type::Bool, lhs.span)?;
                let evaluate = self.block();
                let short = self.block();
                let join = self.block();
                let result = self.block_param(join, Type::Bool);
                let is_and = matches!(expr.kind, ExprKind::And(_, _));
                self.terminate(Terminator::Branch {
                    condition,
                    then_block: if is_and { evaluate } else { short },
                    else_block: if is_and { short } else { evaluate },
                });
                self.current = short;
                let short_value = self.emit(Operation::Bool(!is_and), Type::Bool, expr.span);
                self.terminate(Terminator::Jump(join, vec![short_value]));
                self.current = evaluate;
                let (rhs_value, rhs_type) = self.expr(rhs, locals)?;
                self.require(rhs_type, Type::Bool, rhs.span)?;
                self.terminate(Terminator::Jump(join, vec![rhs_value]));
                self.current = join;
                return Ok((result, Type::Bool));
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
                if name == "request" {
                    let [CallArgument::Value(value), CallArgument::Value(right)] = args.as_slice()
                    else {
                        return Err(Diagnostic::new(
                            expr.span,
                            "request requires an I64 value and one owned ReplyTo<I64>",
                        ));
                    };
                    let (value_id, value_type) = self.expr(value, locals)?;
                    self.require(value_type, Type::I64, value.span)?;
                    let (right_id, right_type) = self.expr(right, locals)?;
                    self.require(right_type, Type::ReplyToI64, right.span)?;
                    let request = self.emit(
                        Operation::PackRequest {
                            value: value_id,
                            right: right_id,
                        },
                        Type::RequestI64,
                        expr.span,
                    );
                    return Ok((request, Type::RequestI64));
                }
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
                if let Some(result) = self.request_when(expr, subject, arms, locals)? {
                    return Ok(result);
                }
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
}
