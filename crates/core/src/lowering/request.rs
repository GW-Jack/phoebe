use super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn request_when(
        &mut self,
        expr: &Expr,
        subject: &Expr,
        arms: &[syntax::WhenArm],
        locals: &HashMap<String, Binding>,
    ) -> Result<Option<TypedValue>, Diagnostic> {
        let ExprKind::QualifiedCall(name, method, args) = &subject.kind else {
            return Ok(None);
        };
        let Some(Binding::Val(endpoint)) = locals.get(name).copied() else {
            return Ok(None);
        };
        let endpoint_type = self.value_types[endpoint.0];
        let (outcome, success_name, terminal_name, payload_type) =
            match (endpoint_type, method.as_str()) {
                (Type::RequestSenderI64, "send") => {
                    ("SendRequest", "Sent", "Closed", Type::RequestI64)
                }
                (Type::RequestReceiverI64, "receive") => {
                    ("ReceiveRequest", "Value", "End", Type::RequestI64)
                }
                _ => return Ok(None),
            };
        let mut success_arm = None;
        let mut terminal_arm = None;
        for arm in arms {
            if arm.type_name != outcome {
                return Err(Diagnostic::new(
                    arm.span,
                    format!("when arm must use {outcome}"),
                ));
            }
            let target = match arm.variant.as_str() {
                name if name == success_name => &mut success_arm,
                name if name == terminal_name => &mut terminal_arm,
                _ => return Err(Diagnostic::new(arm.span, "unknown request outcome variant")),
            };
            if target.replace(arm).is_some() {
                return Err(Diagnostic::new(
                    arm.span,
                    "duplicate request outcome variant",
                ));
            }
        }
        let success_arm = success_arm.ok_or_else(|| {
            Diagnostic::new(
                expr.span,
                format!("non-exhaustive when: missing {outcome}.{success_name}"),
            )
        })?;
        let terminal_arm = terminal_arm.ok_or_else(|| {
            Diagnostic::new(
                expr.span,
                format!("non-exhaustive when: missing {outcome}.{terminal_name}"),
            )
        })?;
        let bound_arm = if endpoint_type == Type::RequestSenderI64 {
            terminal_arm
        } else {
            success_arm
        };
        let plain_arm = if endpoint_type == Type::RequestSenderI64 {
            success_arm
        } else {
            terminal_arm
        };
        if bound_arm.binding.is_none() || plain_arm.binding.is_some() {
            return Err(Diagnostic::new(
                expr.span,
                "request outcome payload binding does not match variant",
            ));
        }
        let (success, terminal, payload) = if endpoint_type == Type::RequestSenderI64 {
            let [CallArgument::Value(request)] = args.as_slice() else {
                return Err(Diagnostic::new(
                    subject.span,
                    "request send requires one owned Request<I64>",
                ));
            };
            let (request_id, ty) = self.expr(request, locals)?;
            self.require(ty, Type::RequestI64, request.span)?;
            let sent = self.block();
            let closed = self.block();
            let rejected = self.block_param(closed, payload_type);
            self.terminate(Terminator::RequestSend {
                sender: endpoint,
                request: request_id,
                sent,
                closed,
            });
            (sent, closed, rejected)
        } else {
            if !args.is_empty() {
                return Err(Diagnostic::new(
                    subject.span,
                    "request receive takes no arguments",
                ));
            }
            let value = self.block();
            let item = self.block_param(value, payload_type);
            let end = self.block();
            self.terminate(Terminator::RequestReceive {
                receiver: endpoint,
                value,
                end,
            });
            (value, end, item)
        };
        let mut results = Vec::with_capacity(2);
        for (arm, block, bound) in [
            (
                success_arm,
                success,
                endpoint_type == Type::RequestReceiverI64,
            ),
            (
                terminal_arm,
                terminal,
                endpoint_type == Type::RequestSenderI64,
            ),
        ] {
            self.current = block;
            let mut arm_locals = locals.clone();
            if bound {
                let (name, span) = arm.binding.as_ref().expect("validated payload binding");
                if arm_locals
                    .insert(name.clone(), Binding::Val(payload))
                    .is_some()
                {
                    return Err(Diagnostic::new(*span, "duplicate pattern binding"));
                }
            }
            let (value, ty) = self
                .body(&arm.body, &mut arm_locals)?
                .expect("parser requires when-arm tail");
            results.push((self.current, value, ty));
        }
        if results[0].2 != results[1].2 {
            return Err(Diagnostic::new(
                expr.span,
                "when arms must have the same type",
            ));
        }
        let ty = results[0].2;
        let join = self.block();
        let result = self.block_param(join, ty);
        for (end, value, _) in results {
            self.current = end;
            self.terminate(Terminator::Jump(join, vec![value]));
        }
        self.current = join;
        Ok(Some((result, ty)))
    }
}
