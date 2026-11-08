//! Lower communication methods while preserving owned rights in mutable places.

use super::*;

fn message_type_name(ty: Type, data: &[DataDef]) -> &str {
    match ty {
        Type::I64 => "I64",
        Type::Bool => "Bool",
        Type::Unit => "Unit",
        Type::Data(id) => &data[id.0].name,
        Type::RequestI64 => "Request<I64>",
        Type::ReplyToI64 => "ReplyTo<I64>",
        Type::ResourceError => "ResourceError",
        _ => unreachable!("typed channel message"),
    }
}

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
                    | (Type::TypedSender(_), "send")
                    | (Type::ReceiverI64, "receive")
                    | (Type::TypedReceiver(_), "receive")
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
                    (Type::SenderI64 | Type::TypedSender(_), "send") => {
                        let [CallArgument::Value(message)] = args else {
                            return Err(Diagnostic::new(
                                expr.span,
                                "send requires one owned message value",
                            ));
                        };
                        let (message_value, message_type) = self.expr(message, locals)?;
                        let expected = match endpoint_type {
                            Type::SenderI64 => Type::I64,
                            Type::TypedSender(message) => message.value_type(),
                            _ => unreachable!(),
                        };
                        self.require(message_type, expected, message.span)?;
                        let outcome_name =
                            format!("Send<{}>", message_type_name(expected, self.data));
                        let outcome = self.data_names[&outcome_name];
                        let sent = self.block();
                        let closed = self.block();
                        let returned = if matches!(endpoint_type, Type::TypedSender(_)) {
                            self.block_param(closed, expected)
                        } else {
                            message_value
                        };
                        let join = self.block();
                        let result = self.block_param(join, Type::Data(outcome));
                        self.terminate(match endpoint_type {
                            Type::SenderI64 => Terminator::ChannelSend {
                                sender: endpoint,
                                message: message_value,
                                sent,
                                closed,
                            },
                            Type::TypedSender(_) => Terminator::TypedSend {
                                sender: endpoint,
                                message: message_value,
                                sent,
                                closed,
                            },
                            _ => unreachable!(),
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
                            Operation::Variant(outcome, 1, Some(returned)),
                            Type::Data(outcome),
                            expr.span,
                        );
                        self.terminate(Terminator::Jump(join, vec![closed_value]));
                        self.current = join;
                        return Ok(Some((result, Type::Data(outcome))));
                    }
                    (Type::ReceiverI64 | Type::TypedReceiver(_), "receive") => {
                        if !args.is_empty() {
                            return Err(Diagnostic::new(expr.span, "receive takes no arguments"));
                        }
                        let expected = match endpoint_type {
                            Type::ReceiverI64 => Type::I64,
                            Type::TypedReceiver(message) => message.value_type(),
                            _ => unreachable!(),
                        };
                        let outcome_name =
                            format!("Receive<{}>", message_type_name(expected, self.data));
                        let outcome = self.data_names[&outcome_name];
                        let value = self.block();
                        let item = self.block_param(value, expected);
                        let end = self.block();
                        let join = self.block();
                        let result = self.block_param(join, Type::Data(outcome));
                        self.terminate(match endpoint_type {
                            Type::ReceiverI64 => Terminator::ChannelReceive {
                                receiver: endpoint,
                                value,
                                end,
                            },
                            Type::TypedReceiver(_) => Terminator::TypedReceive {
                                receiver: endpoint,
                                value,
                                end,
                            },
                            _ => unreachable!(),
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
