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
            ExprKind::FixedArray(elements) => {
                let mut values = Vec::with_capacity(elements.len());
                for element in elements {
                    let (value, ty) = self.expr(element, locals)?;
                    self.require(ty, Type::I64, element.span)?;
                    values.push(value);
                }
                (Operation::ArrayI64(values), Type::ArrayI64(elements.len()))
            }
            ExprKind::FixedIndex(array, index) => {
                let (value, ty) = if let ExprKind::Name(name) = &array.kind {
                    match locals.get(name).copied() {
                        Some(Binding::Val(value)) => (value, self.value_types[value.0]),
                        Some(Binding::Var(place)) => {
                            let Type::ArrayI64(len) = self.place_types[place.0] else {
                                return Err(Diagnostic::new(
                                    array.span,
                                    "indexing requires a fixed I64 array",
                                ));
                            };
                            if *index >= len {
                                return Err(Diagnostic::new(
                                    expr.span,
                                    "constant array index is out of bounds",
                                ));
                            }
                            return Ok((
                                self.emit(
                                    Operation::ArrayGetPlace(place, *index),
                                    Type::I64,
                                    expr.span,
                                ),
                                Type::I64,
                            ));
                        }
                        _ => self.expr(array, locals)?,
                    }
                } else {
                    self.expr(array, locals)?
                };
                let Type::ArrayI64(len) = ty else {
                    return Err(Diagnostic::new(
                        array.span,
                        "indexing requires a fixed I64 array",
                    ));
                };
                if *index >= len {
                    return Err(Diagnostic::new(
                        expr.span,
                        "constant array index is out of bounds",
                    ));
                }
                (Operation::ArrayGet(value, *index), Type::I64)
            }
            ExprKind::Member(name, variant) => {
                if let Some(binding) = locals.get(name).copied() {
                    let ty = match binding {
                        Binding::Val(value) => self.value_types[value.0],
                        Binding::Var(place) => self.place_types[place.0],
                        Binding::Ref(value, _) => self.value_types[value.0],
                    };
                    if let Type::ArrayI64(len) = ty
                        && variant == "len"
                    {
                        return Ok((
                            self.emit(Operation::Const(len as i64), Type::I64, expr.span),
                            Type::I64,
                        ));
                    }
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
                    if self.capability_layout.leaves(field_type) > 0 {
                        return Err(Diagnostic::new(
                            expr.span,
                            "capability field requires whole-product unpack",
                        ));
                    }
                    let operation = match binding {
                        Binding::Val(value) => Operation::Field(value, index),
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
                if let Some(result) = self.communication_call(expr, name, member, args, locals)? {
                    return Ok(result);
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
                let err_ty = data.payloads[err_tag]
                    .ok_or_else(|| Diagnostic::new(inner.span, "Result.Err requires a payload"))?;
                let Type::Data(return_id) = self.return_type else {
                    return Err(Diagnostic::new(
                        expr.span,
                        "? requires the function to return Result with the same error type",
                    ));
                };
                let return_data = &self.data[return_id.0];
                let return_error_tag = return_data
                    .variants
                    .iter()
                    .position(|name| name == "Err")
                    .ok_or_else(|| {
                    Diagnostic::new(
                        expr.span,
                        "? requires the function to return Result with the same error type",
                    )
                })?;
                if !return_data.name.starts_with("Result<")
                    || return_data.variants.len() != 2
                    || return_data.payloads[return_error_tag] != Some(err_ty)
                {
                    return Err(Diagnostic::new(
                        expr.span,
                        "? requires the function to return Result with the same error type",
                    ));
                }
                let success = self.block();
                let payload = self.block_param(success, ok_ty);
                let failure = self.block();
                let passthrough = self.return_type == ty;
                let error = self.block_param(failure, if passthrough { ty } else { err_ty });
                let mut arms = vec![
                    MatchArm {
                        target: failure,
                        binding: if passthrough {
                            MatchBinding::Whole
                        } else {
                            MatchBinding::Payload
                        },
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
                let failed = if passthrough {
                    error
                } else {
                    self.emit(
                        Operation::Variant(return_id, return_error_tag, Some(error)),
                        self.return_type,
                        expr.span,
                    )
                };
                self.implicit_return(failed, expr.span);
                self.current = success;
                return Ok((payload, ok_ty));
            }
            ExprKind::Name(name) => {
                if name == "Unit" && !locals.contains_key(name) {
                    return Ok((
                        self.emit(Operation::Unit, Type::Unit, expr.span),
                        Type::Unit,
                    ));
                }
                let binding = locals
                    .get(name)
                    .copied()
                    .ok_or_else(|| Diagnostic::new(expr.span, format!("unknown local {name}")))?;
                return Ok(match binding {
                    Binding::Val(id) => {
                        let ty = self.value_types[id.0];
                        if matches!(ty, Type::Data(_) | Type::ArrayI64(_)) {
                            let operation = if self.capability_layout.leaves(ty) == 0 {
                                Operation::InferredCopy(id)
                            } else {
                                Operation::Move(id)
                            };
                            (self.emit(operation, ty, expr.span), ty)
                        } else {
                            (id, ty)
                        }
                    }
                    Binding::Var(place) => {
                        let ty = self.place_types[place.0];
                        let operation = if matches!(ty, Type::Data(_) | Type::ArrayI64(_))
                            || self.capability_layout.leaves(ty) > 0
                        {
                            if self.capability_layout.leaves(ty) == 0 {
                                Operation::InferredCopyPlace(place)
                            } else {
                                Operation::Take(place)
                            }
                        } else {
                            Operation::Load(place)
                        };
                        (self.emit(operation, ty, expr.span), ty)
                    }
                    Binding::Ref(reference, _) => {
                        let ty = self.value_types[reference.0]
                            .referent()
                            .expect("validated reference parameter");
                        (self.emit(Operation::LoadRef(reference), ty, expr.span), ty)
                    }
                });
            }
            ExprKind::Borrow { .. } => {
                return Err(Diagnostic::new(
                    expr.span,
                    "borrow expression requires a with binding",
                ));
            }
            ExprKind::Copy(inner) => {
                if let ExprKind::Name(name) = &inner.kind {
                    let binding = locals.get(name).copied().ok_or_else(|| {
                        Diagnostic::new(inner.span, format!("unknown local {name}"))
                    })?;
                    match binding {
                        Binding::Val(value)
                            if matches!(
                                self.value_types[value.0],
                                Type::Data(_) | Type::ArrayI64(_)
                            ) =>
                        {
                            let ty = self.value_types[value.0];
                            if self.capability_layout.leaves(ty) > 0 {
                                return Err(Diagnostic::new(
                                    expr.span,
                                    "copy cannot duplicate communication capabilities",
                                ));
                            }
                            return Ok((self.emit(Operation::Copy(value), ty, expr.span), ty));
                        }
                        Binding::Var(place)
                            if matches!(
                                self.place_types[place.0],
                                Type::Data(_) | Type::ArrayI64(_)
                            ) =>
                        {
                            let ty = self.place_types[place.0];
                            if self.capability_layout.leaves(ty) > 0 {
                                return Err(Diagnostic::new(
                                    expr.span,
                                    "copy cannot duplicate communication capabilities",
                                ));
                            }
                            return Ok((self.emit(Operation::CopyPlace(place), ty, expr.span), ty));
                        }
                        _ => {}
                    }
                }
                let (value, ty) = self.expr(inner, locals)?;
                if matches!(ty, Type::Data(_) | Type::ArrayI64(_)) {
                    if self.capability_layout.leaves(ty) > 0 {
                        return Err(Diagnostic::new(
                            expr.span,
                            "copy cannot duplicate communication capabilities",
                        ));
                    }
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
            ExprKind::Eq(lhs, rhs) | ExprKind::Ne(lhs, rhs) => {
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
                (
                    if matches!(expr.kind, ExprKind::Eq(_, _)) {
                        Operation::Eq(a, b)
                    } else {
                        Operation::Ne(a, b)
                    },
                    Type::Bool,
                )
            }
            ExprKind::Lt(lhs, rhs)
            | ExprKind::Le(lhs, rhs)
            | ExprKind::Gt(lhs, rhs)
            | ExprKind::Ge(lhs, rhs) => {
                let ((a, ta), (b, tb)) = self.pair(lhs, rhs, locals)?;
                self.require(ta, Type::I64, lhs.span)?;
                self.require(tb, Type::I64, rhs.span)?;
                (
                    match expr.kind {
                        ExprKind::Lt(_, _) => Operation::Lt(a, b),
                        ExprKind::Le(_, _) => Operation::Le(a, b),
                        ExprKind::Gt(_, _) => Operation::Gt(a, b),
                        ExprKind::Ge(_, _) => Operation::Ge(a, b),
                        _ => unreachable!(),
                    },
                    Type::Bool,
                )
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
                if let Some(generics) = self.generics
                    && generics
                        .as_module()
                        .functions
                        .iter()
                        .any(|function| function.name == *name)
                {
                    let mut values = Vec::with_capacity(args.len());
                    let mut types = Vec::with_capacity(args.len());
                    for arg in args {
                        let CallArgument::Value(arg) = arg else {
                            return Err(Diagnostic::new(
                                expr.span,
                                "generic call requires owned arguments",
                            ));
                        };
                        let (value, ty) = self.expr(arg, locals)?;
                        values.push(value);
                        types.push(ty);
                    }
                    let resolved = crate::generic_calls::infer(generics, name, &types)
                        .map_err(|message| Diagnostic::new(expr.span, message))?;
                    return Ok((
                        self.emit(
                            Operation::GenericCall {
                                callee: resolved.callee,
                                type_args: resolved.type_args,
                                args: values,
                            },
                            resolved.result,
                            expr.span,
                        ),
                        resolved.result,
                    ));
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
                                    self.require(self.place_types[place.0], param_type, *span)?;
                                    CallArg::Borrow(place, *mutable)
                                }
                                Binding::Val(value) if !*mutable => {
                                    self.require(self.value_types[value.0], param_type, *span)?;
                                    let place = self.place(param_type);
                                    self.emit_effect(Operation::PlaceInit(place, value), *span);
                                    CallArg::Borrow(place, false)
                                }
                                Binding::Ref(value, source_mode)
                                    if !*mutable || source_mode == ParamMode::Exclusive =>
                                {
                                    let actual = self.value_types[value.0]
                                        .referent()
                                        .expect("validated reference parameter");
                                    self.require(actual, param_type, *span)?;
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
