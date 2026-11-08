use super::*;

impl Parser {
    pub(super) fn block(&mut self, require_tail: bool) -> Result<Body, Diagnostic> {
        let body = self.nested(|parser| parser.block_inner(require_tail))?;
        if body_height(&body) > MAX_AST_DEPTH {
            return Err(Diagnostic::new(
                self.tokens[self.pos - 1].span,
                "source tree exceeds compiler limit",
            ));
        }
        Ok(body)
    }

    pub(super) fn block_inner(&mut self, require_tail: bool) -> Result<Body, Diagnostic> {
        self.expect(&TokenKind::LBrace, "{")?;
        let mut statements = Vec::new();
        loop {
            let binding = if self.eat(&TokenKind::Val).is_some() {
                Some(false)
            } else if self.eat(&TokenKind::Var).is_some() {
                Some(true)
            } else {
                None
            };
            if let Some(mutable) = binding {
                if self.eat(&TokenKind::LParen).is_some() {
                    let mut names = vec![self.name()?];
                    while self.eat(&TokenKind::Comma).is_some() {
                        names.push(self.name()?);
                    }
                    self.expect(&TokenKind::RParen, ")")?;
                    self.expect(&TokenKind::Equal, "=")?;
                    let (constructor, start) = self.name()?;
                    if self.eat(&TokenKind::Dot).is_some() {
                        let (method, end) = self.name()?;
                        if method != "unpack" {
                            return Err(Diagnostic::new(
                                end,
                                "destructuring binding requires unpack()",
                            ));
                        }
                        self.expect(&TokenKind::LParen, "(")?;
                        self.expect(&TokenKind::RParen, ")")?;
                        self.expect(&TokenKind::Semicolon, "; after binding")?;
                        statements.push(Statement::UnpackVal {
                            mutable,
                            names,
                            value: Expr {
                                kind: ExprKind::Name(constructor),
                                span: start,
                            },
                            span: start.join(end),
                        });
                        continue;
                    }
                    if names.len() != 2 {
                        return Err(Diagnostic::new(
                            start,
                            "channel and reply bindings require two names",
                        ));
                    }
                    let mut names = names.into_iter();
                    let sender = names.next().expect("checked pair binding");
                    let receiver = names.next().expect("checked pair binding");
                    if constructor != "request_channel"
                        && !matches!(constructor.as_str(), "channel" | "reply")
                    {
                        return Err(Diagnostic::new(
                            start,
                            "pair binding requires channel, reply, request_channel, or unpack",
                        ));
                    }
                    self.expect(&TokenKind::Less, "<")?;
                    let (element, element_span) = self.name()?;
                    if element != "I64" {
                        return Err(Diagnostic::new(
                            element_span,
                            "only I64 channels are implemented",
                        ));
                    }
                    self.expect(&TokenKind::Greater, ">")?;
                    self.expect(&TokenKind::LParen, "(")?;
                    if constructor == "reply" {
                        self.uses_reply_outcome = true;
                        self.expect(&TokenKind::RParen, ") after reply")?;
                        let question =
                            self.expect(&TokenKind::Question, "? after fallible reply creation")?;
                        self.expect(&TokenKind::Semicolon, "; after binding")?;
                        statements.push(Statement::ReplyVal {
                            mutable,
                            right: sender,
                            receiver,
                            span: start.join(question.span),
                        });
                        continue;
                    }
                    if self.current().kind == TokenKind::Ident("capacity".into())
                        && self
                            .tokens
                            .get(self.pos + 1)
                            .is_some_and(|next| next.kind == TokenKind::Equal)
                    {
                        self.pos += 1;
                        self.pos += 1;
                    }
                    let capacity = self.expr(0)?;
                    self.expect(&TokenKind::RParen, ")")?;
                    let question =
                        self.expect(&TokenKind::Question, "? after fallible channel creation")?;
                    self.expect(&TokenKind::Semicolon, "; after binding")?;
                    if constructor == "request_channel" {
                        statements.push(Statement::RequestChannelVal {
                            mutable,
                            sender,
                            receiver,
                            capacity,
                            span: start.join(question.span),
                        });
                    } else {
                        statements.push(Statement::ChannelVal {
                            mutable,
                            sender,
                            receiver,
                            capacity,
                            span: start.join(question.span),
                        });
                    }
                    continue;
                }
                let (name, span) = self.name()?;
                self.expect(&TokenKind::Equal, "=")?;
                let value = self.expr(0)?;
                self.expect(&TokenKind::Semicolon, "; after binding")?;
                statements.push(if mutable {
                    Statement::Var(name, span, value)
                } else {
                    Statement::Val(name, span, value)
                });
                continue;
            }
            if self.eat(&TokenKind::While).is_some() {
                self.expect(&TokenKind::LParen, "(")?;
                let condition = self.expr(0)?;
                self.expect(&TokenKind::RParen, ")")?;
                let body = self.block(false)?;
                statements.push(Statement::While(condition, body));
                continue;
            }
            if let Some(start) = self.eat(&TokenKind::Spawn) {
                let (child, _) = self.name()?;
                self.expect(&TokenKind::LParen, "(")?;
                let (args, _) = self.call_arguments()?;
                let end = self.expect(&TokenKind::Question, "? after fallible spawn")?;
                self.expect(&TokenKind::Semicolon, "; after spawn")?;
                statements.push(Statement::Spawn {
                    child,
                    args,
                    span: start.span.join(end.span),
                });
                continue;
            }
            if let Some(start) = self.eat(&TokenKind::Scope) {
                let body = self.block(false)?;
                let span = start.span.join(self.tokens[self.pos - 1].span);
                self.eat(&TokenKind::Semicolon);
                statements.push(Statement::Scope { span, body });
                continue;
            }
            if let Some(start) = self.eat(&TokenKind::With) {
                let bindings = self.with_bindings()?;
                let body = self.block(false)?;
                let span = start.span.join(self.tokens[self.pos - 1].span);
                if body.tail.is_some() {
                    let expr = nest_with_expression(bindings, body, span);
                    if self.eat(&TokenKind::Semicolon).is_some() {
                        statements.push(Statement::Expr(expr));
                    } else {
                        self.expect(&TokenKind::RBrace, "} after with expression")?;
                        return Ok(Body {
                            statements,
                            tail: Some(expr),
                        });
                    }
                } else {
                    self.eat(&TokenKind::Semicolon);
                    statements.push(nest_with_statement(bindings, body, span));
                }
                continue;
            }
            if let Some(start) = self.eat(&TokenKind::Yield) {
                self.expect(&TokenKind::Semicolon, "; after yield")?;
                statements.push(Statement::Yield(start.span));
                continue;
            }
            let assignment = matches!(self.current().kind, TokenKind::Ident(_))
                && self
                    .tokens
                    .get(self.pos + 1)
                    .is_some_and(|next| next.kind == TokenKind::Equal);
            if assignment {
                let (name, span) = self.name()?;
                self.expect(&TokenKind::Equal, "=")?;
                let value = self.expr(0)?;
                self.expect(&TokenKind::Semicolon, "; after assignment")?;
                statements.push(Statement::Assign(name, span, value));
                continue;
            }
            if self.eat(&TokenKind::RBrace).is_some() {
                if require_tail {
                    return Err(Diagnostic::new(
                        self.tokens[self.pos - 1].span,
                        "expected a final expression",
                    ));
                }
                return Ok(Body {
                    statements,
                    tail: None,
                });
            }
            let expr = self.expr(0)?;
            if self.eat(&TokenKind::Semicolon).is_some() {
                statements.push(Statement::Expr(expr));
                continue;
            }
            self.expect(&TokenKind::RBrace, "}")?;
            return Ok(Body {
                statements,
                tail: Some(expr),
            });
        }
    }
}
