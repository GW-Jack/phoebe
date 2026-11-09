use super::*;

impl Parser {
    pub(super) fn expr(&mut self, min_bp: u8) -> Result<Expr, Diagnostic> {
        let expr = self.nested(|parser| parser.expr_inner(min_bp))?;
        if expr_height(&expr) > MAX_AST_DEPTH {
            return Err(Diagnostic::new(
                self.tokens[self.pos - 1].span,
                "source tree exceeds compiler limit",
            ));
        }
        Ok(expr)
    }

    pub(super) fn expr_inner(&mut self, min_bp: u8) -> Result<Expr, Diagnostic> {
        let mut lhs = match self.current().clone() {
            Token {
                kind: TokenKind::Amp,
                span,
            } => {
                self.pos += 1;
                let mutable = self.eat(&TokenKind::Mut).is_some();
                let (name, path, end) = self.borrow_path()?;
                Expr {
                    kind: ExprKind::Borrow {
                        name,
                        path,
                        mutable,
                    },
                    span: span.join(end),
                }
            }
            Token {
                kind: TokenKind::With,
                span,
            } => {
                self.pos += 1;
                let bindings = self.with_bindings()?;
                let body = self.block(true)?;
                let end = self.tokens[self.pos - 1].span;
                nest_with_expression(bindings, body, span.join(end))
            }
            Token {
                kind: TokenKind::Int(value),
                span,
            } => {
                let value = i64::try_from(value)
                    .map_err(|_| Diagnostic::new(span, "integer literal exceeds I64"))?;
                self.pos += 1;
                Expr {
                    kind: ExprKind::Int(value),
                    span,
                }
            }
            Token {
                kind: TokenKind::LBracket,
                span,
            } => {
                self.pos += 1;
                let mut elements = Vec::new();
                if self.current().kind != TokenKind::RBracket {
                    loop {
                        if elements.len() == 256 {
                            return Err(Diagnostic::new(
                                self.current().span,
                                "fixed I64 array exceeds 256 elements",
                            ));
                        }
                        elements.push(self.expr(0)?);
                        if self.eat(&TokenKind::Comma).is_none() {
                            break;
                        }
                        if self.current().kind == TokenKind::RBracket {
                            break;
                        }
                    }
                }
                let end = self.expect(&TokenKind::RBracket, "]")?.span;
                Expr {
                    kind: ExprKind::FixedArray(elements),
                    span: span.join(end),
                }
            }
            Token {
                kind: TokenKind::True,
                span,
            } => {
                self.pos += 1;
                Expr {
                    kind: ExprKind::Bool(true),
                    span,
                }
            }
            Token {
                kind: TokenKind::False,
                span,
            } => {
                self.pos += 1;
                Expr {
                    kind: ExprKind::Bool(false),
                    span,
                }
            }
            Token {
                kind: TokenKind::If,
                span,
            } => {
                self.pos += 1;
                self.expect(&TokenKind::LParen, "(")?;
                let condition = self.expr(0)?;
                self.expect(&TokenKind::RParen, ")")?;
                let then_body = self.block(true)?;
                self.expect(&TokenKind::Else, "else")?;
                let else_body = self.block(true)?;
                let end = self.tokens[self.pos - 1].span;
                Expr {
                    kind: ExprKind::If(
                        Box::new(condition),
                        Box::new(then_body),
                        Box::new(else_body),
                    ),
                    span: span.join(end),
                }
            }
            Token {
                kind: TokenKind::When,
                span,
            } => {
                self.pos += 1;
                self.expect(&TokenKind::LParen, "(")?;
                let subject = self.expr(0)?;
                self.expect(&TokenKind::RParen, ")")?;
                self.expect(&TokenKind::LBrace, "{")?;
                let mut arms = Vec::new();
                loop {
                    let (type_name, start) = self.name()?;
                    let mut type_name = if matches!(type_name.as_str(), "Option" | "Result")
                        && self.current().kind == TokenKind::Less
                    {
                        self.applied_type_after_name(&type_name, start)?.key()
                    } else if matches!(
                        type_name.as_str(),
                        "Send" | "Receive" | "Complete" | "Reply"
                    ) {
                        self.channel_outcome_type_after_name(&type_name, start)?
                            .key()
                    } else {
                        type_name
                    };
                    self.expect(&TokenKind::Dot, ".")?;
                    let (first, first_end) = self.name()?;
                    let (variant, end) = if self.eat(&TokenKind::Dot).is_some() {
                        type_name = format!("{type_name}.{first}");
                        self.name()?
                    } else {
                        (first, first_end)
                    };
                    let binding = if self.eat(&TokenKind::LParen).is_some() {
                        let name = self.name()?;
                        self.expect(&TokenKind::RParen, ")")?;
                        Some(name)
                    } else {
                        None
                    };
                    self.expect(&TokenKind::Arrow, "->")?;
                    let body = self.block(true)?;
                    arms.push(WhenArm {
                        type_name,
                        variant,
                        span: start.join(end),
                        binding,
                        body,
                    });
                    if self.eat(&TokenKind::Comma).is_none() {
                        let end = self.expect(&TokenKind::RBrace, "}")?.span;
                        break Expr {
                            kind: ExprKind::When(Box::new(subject), arms),
                            span: span.join(end),
                        };
                    }
                    if let Some(end) = self.eat(&TokenKind::RBrace) {
                        break Expr {
                            kind: ExprKind::When(Box::new(subject), arms),
                            span: span.join(end.span),
                        };
                    }
                }
            }
            Token {
                kind: TokenKind::Ident(name),
                span,
            } => {
                self.pos += 1;
                let name = if matches!(name.as_str(), "Option" | "Result")
                    && self.current().kind == TokenKind::Less
                {
                    self.applied_type_after_name(&name, span)?.key()
                } else if matches!(name.as_str(), "Send" | "Receive" | "Complete" | "Reply") {
                    self.channel_outcome_type_after_name(&name, span)?.key()
                } else {
                    name
                };
                if self.eat(&TokenKind::Dot).is_some() {
                    let (first, first_end) = self.name()?;
                    let (name, variant, end) = if self.eat(&TokenKind::Dot).is_some() {
                        let (variant, end) = self.name()?;
                        (format!("{name}.{first}"), variant, end)
                    } else {
                        (name, first, first_end)
                    };
                    if self.eat(&TokenKind::LParen).is_some() {
                        let (args, close) = self.call_arguments()?;
                        if variant == "send" {
                            self.uses_send_outcome = true;
                        } else if variant == "receive" {
                            self.uses_receive_outcome = true;
                        } else if variant == "complete" {
                            self.uses_complete_outcome = true;
                        }
                        Expr {
                            kind: ExprKind::QualifiedCall(name, variant, args),
                            span: span.join(close),
                        }
                    } else {
                        Expr {
                            kind: ExprKind::Member(name, variant),
                            span: span.join(end),
                        }
                    }
                } else if self.eat(&TokenKind::LParen).is_some() {
                    let (args, end) = self.call_arguments()?;
                    Expr {
                        kind: ExprKind::Call(name, args),
                        span: span.join(end),
                    }
                } else {
                    Expr {
                        kind: ExprKind::Name(name),
                        span,
                    }
                }
            }
            Token {
                kind: TokenKind::Copy,
                span,
            } => {
                self.pos += 1;
                self.expect(&TokenKind::LParen, "(")?;
                let value = self.expr(0)?;
                let end = self.expect(&TokenKind::RParen, ")")?.span;
                Expr {
                    kind: ExprKind::Copy(Box::new(value)),
                    span: span.join(end),
                }
            }
            Token {
                kind: TokenKind::Minus,
                span,
            } => {
                self.pos += 1;
                if let Token {
                    kind: TokenKind::Int(value),
                    span: magnitude,
                } = self.current().clone()
                    && value > i64::MAX as u64
                {
                    if value != (i64::MAX as u64) + 1 {
                        return Err(Diagnostic::new(
                            span.join(magnitude),
                            "integer literal exceeds I64",
                        ));
                    }
                    self.pos += 1;
                    Expr {
                        kind: ExprKind::Int(i64::MIN),
                        span: span.join(magnitude),
                    }
                } else {
                    let rhs = self.expr(11)?;
                    let end = rhs.span;
                    Expr {
                        kind: ExprKind::Neg(Box::new(rhs)),
                        span: span.join(end),
                    }
                }
            }
            Token {
                kind: TokenKind::Bang,
                span,
            } => {
                self.pos += 1;
                let rhs = self.expr(11)?;
                let end = rhs.span;
                Expr {
                    kind: ExprKind::Not(Box::new(rhs)),
                    span: span.join(end),
                }
            }
            Token {
                kind: TokenKind::LParen,
                ..
            } => {
                self.pos += 1;
                let expr = self.expr(0)?;
                self.expect(&TokenKind::RParen, ")")?;
                expr
            }
            _ => return Err(Diagnostic::new(self.current().span, "expected expression")),
        };

        let mut chain = 0;
        loop {
            if self.current().kind == TokenKind::LBracket && min_bp <= 12 {
                if chain == MAX_PARSE_DEPTH {
                    return Err(Diagnostic::new(
                        self.current().span,
                        "expression chain exceeds compiler limit",
                    ));
                }
                chain += 1;
                self.pos += 1;
                let Token {
                    kind: TokenKind::Int(index),
                    span,
                } = self.current().clone()
                else {
                    return Err(Diagnostic::new(
                        self.current().span,
                        "fixed array index must be a constant nonnegative integer",
                    ));
                };
                let index = usize::try_from(index)
                    .map_err(|_| Diagnostic::new(span, "fixed array index exceeds target size"))?;
                self.pos += 1;
                let end = self.expect(&TokenKind::RBracket, "]")?.span;
                lhs = Expr {
                    span: lhs.span.join(end),
                    kind: ExprKind::FixedIndex(Box::new(lhs), index),
                };
                continue;
            }
            if self.current().kind == TokenKind::Question && min_bp <= 13 {
                if chain == MAX_PARSE_DEPTH {
                    return Err(Diagnostic::new(
                        self.current().span,
                        "expression chain exceeds compiler limit",
                    ));
                }
                chain += 1;
                let end = self.current().span;
                self.pos += 1;
                lhs = Expr {
                    span: lhs.span.join(end),
                    kind: ExprKind::Try(Box::new(lhs)),
                };
                continue;
            }
            let (left_bp, right_bp) = match self.current().kind {
                TokenKind::OrOr => (1, 2),
                TokenKind::AndAnd => (3, 4),
                TokenKind::EqualEqual
                | TokenKind::BangEqual
                | TokenKind::Less
                | TokenKind::LessEqual
                | TokenKind::Greater
                | TokenKind::GreaterEqual => (5, 6),
                TokenKind::Plus | TokenKind::Minus => (7, 8),
                TokenKind::Star => (9, 10),
                _ => break,
            };
            if left_bp < min_bp {
                break;
            }
            if chain == MAX_PARSE_DEPTH {
                return Err(Diagnostic::new(
                    self.current().span,
                    "expression chain exceeds compiler limit",
                ));
            }
            chain += 1;
            let op = self.current().kind.clone();
            self.pos += 1;
            let rhs = self.expr(right_bp)?;
            let span = lhs.span.join(rhs.span);
            let kind = match op {
                TokenKind::Plus => ExprKind::Add(Box::new(lhs), Box::new(rhs)),
                TokenKind::Minus => ExprKind::Sub(Box::new(lhs), Box::new(rhs)),
                TokenKind::Star => ExprKind::Mul(Box::new(lhs), Box::new(rhs)),
                TokenKind::EqualEqual => ExprKind::Eq(Box::new(lhs), Box::new(rhs)),
                TokenKind::BangEqual => ExprKind::Ne(Box::new(lhs), Box::new(rhs)),
                TokenKind::Less => ExprKind::Lt(Box::new(lhs), Box::new(rhs)),
                TokenKind::LessEqual => ExprKind::Le(Box::new(lhs), Box::new(rhs)),
                TokenKind::Greater => ExprKind::Gt(Box::new(lhs), Box::new(rhs)),
                TokenKind::GreaterEqual => ExprKind::Ge(Box::new(lhs), Box::new(rhs)),
                TokenKind::AndAnd => ExprKind::And(Box::new(lhs), Box::new(rhs)),
                TokenKind::OrOr => ExprKind::Or(Box::new(lhs), Box::new(rhs)),
                _ => unreachable!(),
            };
            lhs = Expr { kind, span };
        }
        Ok(lhs)
    }
}
