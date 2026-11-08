use super::*;

impl Parser {
    pub(super) fn type_name(&mut self, borrowed: bool) -> Result<TypeName, Diagnostic> {
        self.nested(|parser| parser.type_name_inner(borrowed))
    }

    pub(super) fn type_name_inner(&mut self, borrowed: bool) -> Result<TypeName, Diagnostic> {
        let (name, span) = self.name()?;
        if matches!(
            name.as_str(),
            "Sender"
                | "Receiver"
                | "ReplyTo"
                | "ReplyReceiver"
                | "Request"
                | "RequestSender"
                | "RequestReceiver"
        ) {
            self.expect(&TokenKind::Less, "< after channel endpoint type")?;
            let (element, element_span) = self.name()?;
            if element != "I64" {
                return Err(Diagnostic::new(
                    element_span,
                    "only I64 communication values are implemented",
                ));
            }
            let end = self.expect(&TokenKind::Greater, ">")?.span;
            if borrowed {
                return Err(Diagnostic::new(
                    span.join(end),
                    "endpoint borrows are not implemented",
                ));
            }
            return Ok(match name.as_str() {
                "Sender" => TypeName::SenderI64,
                "Receiver" => TypeName::ReceiverI64,
                "ReplyTo" => TypeName::ReplyToI64,
                "ReplyReceiver" => {
                    self.uses_reply_outcome = true;
                    TypeName::ReplyReceiverI64
                }
                "Request" => TypeName::RequestI64,
                "RequestSender" => TypeName::RequestSenderI64,
                "RequestReceiver" => TypeName::RequestReceiverI64,
                _ => unreachable!(),
            });
        }
        if matches!(name.as_str(), "Send" | "Receive" | "Complete" | "Reply") {
            if borrowed {
                return Err(Diagnostic::new(
                    span,
                    "channel outcome borrows are not implemented",
                ));
            }
            return self.channel_outcome_type_after_name(&name, span);
        }
        if matches!(name.as_str(), "Option" | "Result") {
            if self.current().kind != TokenKind::Less {
                return Err(Diagnostic::new(
                    span,
                    if name == "Result" {
                        "Result requires two type arguments"
                    } else {
                        "Option requires one type argument"
                    },
                ));
            }
            let ty = self.applied_type_after_name(&name, span)?;
            if borrowed {
                return Err(Diagnostic::new(
                    span,
                    "generic data borrows are not implemented",
                ));
            }
            return Ok(ty);
        }
        if self.eat(&TokenKind::Dot).is_some() {
            let (member, end) = self.name()?;
            if borrowed {
                return Err(Diagnostic::new(
                    span.join(end),
                    "only I64 borrows are implemented",
                ));
            }
            return Ok(TypeName::Named(format!("{name}.{member}")));
        }
        match name.as_str() {
            "I64" => Ok(TypeName::I64),
            "Bool" => Ok(TypeName::Bool),
            "Unit" if !borrowed => Ok(TypeName::Unit),
            "Unit" => Err(Diagnostic::new(span, "Unit borrows are not implemented")),
            _ if !borrowed => Ok(TypeName::Named(name)),
            _ => Err(Diagnostic::new(span, "only I64 borrows are implemented")),
        }
    }

    pub(super) fn applied_type_after_name(
        &mut self,
        name: &str,
        start: Span,
    ) -> Result<TypeName, Diagnostic> {
        self.expect(&TokenKind::Less, "< after generic data type")?;
        let value = self.type_name(false)?;
        let ty = if name == "Result" {
            self.expect(&TokenKind::Comma, ",")?;
            let err = self.type_name(false)?;
            TypeName::Result(Box::new(value), Box::new(err))
        } else {
            TypeName::Option(Box::new(value))
        };
        let end = self.expect(&TokenKind::Greater, ">")?.span;
        self.applied_types.push((ty.clone(), start.join(end)));
        Ok(ty)
    }

    pub(super) fn channel_outcome_type_after_name(
        &mut self,
        name: &str,
        _start: Span,
    ) -> Result<TypeName, Diagnostic> {
        self.expect(&TokenKind::Less, "< after channel outcome type")?;
        let (element, element_span) = self.name()?;
        if element != "I64" {
            return Err(Diagnostic::new(
                element_span,
                "only I64 channels are implemented",
            ));
        }
        self.expect(&TokenKind::Greater, ">")?;
        match name {
            "Send" => self.uses_send_outcome = true,
            "Receive" => self.uses_receive_outcome = true,
            "Complete" => self.uses_complete_outcome = true,
            "Reply" => self.uses_reply_outcome = true,
            _ => unreachable!(),
        }
        Ok(TypeName::Named(format!("{name}<I64>")))
    }
}
