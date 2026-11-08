//! Lower communication methods while preserving owned rights in mutable places.

use super::*;

impl Lowerer<'_> {
    pub(super) fn communication_call(
        &mut self,
        expr: &Expr,
        name: &str,
        member: &str,
        args: &[CallArgument],
        locals: &HashMap<String, Binding>,
    ) -> Result<Option<TypedValue>, Diagnostic> {
        if let Some(binding @ (Binding::Val(_) | Binding::Var(_))) = locals.get(name).copied() {
            let endpoint_type = match binding {
                Binding::Val(value) => self.value_types[value.0],
                Binding::Var(place) => self.place_types[place.0],
                Binding::Ref(_, _) => unreachable!(),
            };
            let known_method = matches!(
                (endpoint_type, member),
                (Type::SenderI64, "send")
                    | (Type::ReceiverI64, "receive")
                    | (Type::ReplyToI64, "complete")
                    | (Type::ReplyReceiverI64, "receive")
                    | (Type::RequestSenderI64, "send")
                    | (Type::RequestReceiverI64, "receive")
                    | (Type::RequestI64, "unpack")
            );
            if known_method {
                let retained_place = match binding {
                    Binding::Var(place) => Some(place),
                    Binding::Val(_) | Binding::Ref(_, _) => None,
                };
                let endpoint = match binding {
                    Binding::Val(value) => value,
                    Binding::Var(place) => {
                        self.emit(Operation::Take(place), endpoint_type, expr.span)
                    }
                    Binding::Ref(_, _) => unreachable!(),
                };
                match (endpoint_type, member) {
                    (Type::SenderI64, "send") => {
                        let [CallArgument::Value(message)] = args else {
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
                        if let Some(place) = retained_place {
                            self.emit_effect(Operation::Store(place, endpoint), expr.span);
                        }
                        let sent_value = self.emit(
                            Operation::Variant(outcome, 0, None),
                            Type::Data(outcome),
                            expr.span,
                        );
                        self.terminate(Terminator::Jump(join, vec![sent_value]));
                        self.current = closed;
                        if let Some(place) = retained_place {
                            self.emit_effect(Operation::Store(place, endpoint), expr.span);
                        }
                        let closed_value = self.emit(
                            Operation::Variant(outcome, 1, Some(message_value)),
                            Type::Data(outcome),
                            expr.span,
                        );
                        self.terminate(Terminator::Jump(join, vec![closed_value]));
                        self.current = join;
                        return Ok(Some((result, Type::Data(outcome))));
                    }
                    (Type::ReceiverI64, "receive") => {
                        if !args.is_empty() {
                            return Err(Diagnostic::new(expr.span, "receive takes no arguments"));
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
                        if let Some(place) = retained_place {
                            self.emit_effect(Operation::Store(place, endpoint), expr.span);
                        }
                        let some_value = self.emit(
                            Operation::Variant(outcome, 0, Some(item)),
                            Type::Data(outcome),
                            expr.span,
                        );
                        self.terminate(Terminator::Jump(join, vec![some_value]));
                        self.current = end;
                        if let Some(place) = retained_place {
                            self.emit_effect(Operation::Store(place, endpoint), expr.span);
                        }
                        let end_value = self.emit(
                            Operation::Variant(outcome, 1, None),
                            Type::Data(outcome),
                            expr.span,
                        );
                        self.terminate(Terminator::Jump(join, vec![end_value]));
                        self.current = join;
                        return Ok(Some((result, Type::Data(outcome))));
                    }
                    (Type::ReplyToI64, "complete") => {
                        let [CallArgument::Value(message)] = args else {
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
                        return Ok(Some((result, Type::Data(outcome))));
                    }
                    (Type::ReplyReceiverI64, "receive") => {
                        if !args.is_empty() {
                            return Err(Diagnostic::new(expr.span, "receive takes no arguments"));
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
                        return Ok(Some((result, Type::Data(outcome))));
                    }
                    (Type::RequestSenderI64, "send") | (Type::RequestReceiverI64, "receive") => {
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
        }
        Ok(None)
    }
}
