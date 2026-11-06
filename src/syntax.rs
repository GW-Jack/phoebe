use crate::{Diagnostic, Span};

#[derive(Clone, Debug, PartialEq, Eq)]
enum TokenKind {
    Fn,
    Val,
    Var,
    Mut,
    While,
    If,
    Else,
    True,
    False,
    Ident(String),
    Int(i64),
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Semicolon,
    Arrow,
    Equal,
    EqualEqual,
    Less,
    Plus,
    Minus,
    Star,
    Amp,
    Eof,
}

#[derive(Clone, Debug)]
struct Token {
    kind: TokenKind,
    span: Span,
}

fn lex(source: &str) -> Result<Vec<Token>, Diagnostic> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let kind = match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' => {
                i += 1;
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                match &source[start..i] {
                    "fn" => TokenKind::Fn,
                    "val" => TokenKind::Val,
                    "var" => TokenKind::Var,
                    "mut" => TokenKind::Mut,
                    "while" => TokenKind::While,
                    "if" => TokenKind::If,
                    "else" => TokenKind::Else,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    word => TokenKind::Ident(word.to_owned()),
                }
            }
            b'0'..=b'9' => {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let value = source[start..i].parse().map_err(|_| {
                    Diagnostic::new(Span { start, end: i }, "integer literal exceeds I64")
                })?;
                TokenKind::Int(value)
            }
            b'(' => {
                i += 1;
                TokenKind::LParen
            }
            b')' => {
                i += 1;
                TokenKind::RParen
            }
            b'{' => {
                i += 1;
                TokenKind::LBrace
            }
            b'}' => {
                i += 1;
                TokenKind::RBrace
            }
            b',' => {
                i += 1;
                TokenKind::Comma
            }
            b':' => {
                i += 1;
                TokenKind::Colon
            }
            b';' => {
                i += 1;
                TokenKind::Semicolon
            }
            b'=' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::EqualEqual
            }
            b'=' => {
                i += 1;
                TokenKind::Equal
            }
            b'<' => {
                i += 1;
                TokenKind::Less
            }
            b'+' => {
                i += 1;
                TokenKind::Plus
            }
            b'*' => {
                i += 1;
                TokenKind::Star
            }
            b'&' => {
                i += 1;
                TokenKind::Amp
            }
            b'-' if bytes.get(i + 1) == Some(&b'>') => {
                i += 2;
                TokenKind::Arrow
            }
            b'-' => {
                i += 1;
                TokenKind::Minus
            }
            _ => {
                return Err(Diagnostic::new(
                    Span {
                        start,
                        end: start + 1,
                    },
                    "unrecognized character",
                ));
            }
        };
        tokens.push(Token {
            kind,
            span: Span { start, end: i },
        });
    }
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span { start: i, end: i },
    });
    Ok(tokens)
}

#[derive(Clone, Debug)]
pub struct Program {
    pub functions: Vec<Function>,
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub name_span: Span,
    pub params: Vec<Param>,
    pub return_type: ScalarType,
    pub body: Body,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarType {
    I64,
    Bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamMode {
    Own,
    Shared,
    Exclusive,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub span: Span,
    pub mode: ParamMode,
    pub ty: ScalarType,
}

#[derive(Clone, Debug)]
pub enum CallArgument {
    Value(Expr),
    Borrow {
        name: String,
        span: Span,
        mutable: bool,
    },
}

#[derive(Clone, Debug)]
pub struct Body {
    pub statements: Vec<Statement>,
    pub tail: Option<Expr>,
}

#[derive(Clone, Debug)]
pub enum Statement {
    Val(String, Span, Expr),
    Var(String, Span, Expr),
    Assign(String, Span, Expr),
    While(Expr, Body),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Int(i64),
    Bool(bool),
    Name(String),
    Neg(Box<Expr>),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Eq(Box<Expr>, Box<Expr>),
    Lt(Box<Expr>, Box<Expr>),
    If(Box<Expr>, Box<Body>, Box<Body>),
    Call(String, Vec<CallArgument>),
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn current(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn eat(&mut self, kind: &TokenKind) -> Option<Token> {
        if &self.current().kind == kind {
            let token = self.current().clone();
            self.pos += 1;
            Some(token)
        } else {
            None
        }
    }

    fn expect(&mut self, kind: &TokenKind, label: &str) -> Result<Token, Diagnostic> {
        self.eat(kind)
            .ok_or_else(|| Diagnostic::new(self.current().span, format!("expected {label}")))
    }

    fn name(&mut self) -> Result<(String, Span), Diagnostic> {
        if let TokenKind::Ident(name) = self.current().kind.clone() {
            let span = self.current().span;
            self.pos += 1;
            Ok((name, span))
        } else {
            Err(Diagnostic::new(self.current().span, "expected name"))
        }
    }

    fn function(&mut self) -> Result<Function, Diagnostic> {
        self.expect(&TokenKind::Fn, "fn")?;
        let (name, name_span) = self.name()?;
        self.expect(&TokenKind::LParen, "(")?;
        let mut params = Vec::new();
        if self.eat(&TokenKind::RParen).is_none() {
            loop {
                let (param_name, param_span) = self.name()?;
                self.expect(&TokenKind::Colon, ":")?;
                let mode = if self.eat(&TokenKind::Amp).is_some() {
                    if self.eat(&TokenKind::Mut).is_some() {
                        ParamMode::Exclusive
                    } else {
                        ParamMode::Shared
                    }
                } else {
                    ParamMode::Own
                };
                let (ty, span) = self.name()?;
                let ty = match ty.as_str() {
                    "I64" => ScalarType::I64,
                    "Bool" if mode == ParamMode::Own => ScalarType::Bool,
                    "Bool" => {
                        return Err(Diagnostic::new(span, "Bool borrows are not implemented"));
                    }
                    _ => return Err(Diagnostic::new(span, "only I64 and Bool are implemented")),
                };
                params.push(Param {
                    name: param_name,
                    span: param_span,
                    mode,
                    ty,
                });
                if self.eat(&TokenKind::Comma).is_none() {
                    self.expect(&TokenKind::RParen, ")")?;
                    break;
                }
            }
        }
        self.expect(&TokenKind::Arrow, "->")?;
        let (ret_ty, span) = self.name()?;
        let return_type = match ret_ty.as_str() {
            "I64" => ScalarType::I64,
            "Bool" => ScalarType::Bool,
            _ => return Err(Diagnostic::new(span, "only I64 and Bool are implemented")),
        };
        let body = if self.eat(&TokenKind::Equal).is_some() {
            Body {
                statements: Vec::new(),
                tail: Some(self.expr(0)?),
            }
        } else {
            self.block(true)?
        };
        Ok(Function {
            name,
            name_span,
            params,
            return_type,
            body,
        })
    }

    fn block(&mut self, require_tail: bool) -> Result<Body, Diagnostic> {
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

    fn expr(&mut self, min_bp: u8) -> Result<Expr, Diagnostic> {
        let mut lhs = match self.current().clone() {
            Token {
                kind: TokenKind::Int(value),
                span,
            } => {
                self.pos += 1;
                Expr {
                    kind: ExprKind::Int(value),
                    span,
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
                kind: TokenKind::Ident(name),
                span,
            } => {
                self.pos += 1;
                if self.eat(&TokenKind::LParen).is_some() {
                    let mut args = Vec::new();
                    let end = if let Some(close) = self.eat(&TokenKind::RParen) {
                        close.span
                    } else {
                        loop {
                            if let Some(amp) = self.eat(&TokenKind::Amp) {
                                let mutable = self.eat(&TokenKind::Mut).is_some();
                                let (name, name_span) = self.name()?;
                                args.push(CallArgument::Borrow {
                                    name,
                                    span: amp.span.join(name_span),
                                    mutable,
                                });
                            } else {
                                args.push(CallArgument::Value(self.expr(0)?));
                            }
                            if self.eat(&TokenKind::Comma).is_none() {
                                break self.expect(&TokenKind::RParen, ")")?.span;
                            }
                        }
                    };
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
                kind: TokenKind::Minus,
                span,
            } => {
                self.pos += 1;
                let rhs = self.expr(7)?;
                let end = rhs.span;
                Expr {
                    kind: ExprKind::Neg(Box::new(rhs)),
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

        loop {
            let (left_bp, right_bp) = match self.current().kind {
                TokenKind::EqualEqual | TokenKind::Less => (1, 2),
                TokenKind::Plus | TokenKind::Minus => (3, 4),
                TokenKind::Star => (5, 6),
                _ => break,
            };
            if left_bp < min_bp {
                break;
            }
            let op = self.current().kind.clone();
            self.pos += 1;
            let rhs = self.expr(right_bp)?;
            let span = lhs.span.join(rhs.span);
            let kind = match op {
                TokenKind::Plus => ExprKind::Add(Box::new(lhs), Box::new(rhs)),
                TokenKind::Minus => ExprKind::Sub(Box::new(lhs), Box::new(rhs)),
                TokenKind::Star => ExprKind::Mul(Box::new(lhs), Box::new(rhs)),
                TokenKind::EqualEqual => ExprKind::Eq(Box::new(lhs), Box::new(rhs)),
                TokenKind::Less => ExprKind::Lt(Box::new(lhs), Box::new(rhs)),
                _ => unreachable!(),
            };
            lhs = Expr { kind, span };
        }
        Ok(lhs)
    }
}

pub fn parse(source: &str) -> Result<Program, Diagnostic> {
    let mut parser = Parser {
        tokens: lex(source)?,
        pos: 0,
    };
    let mut functions = Vec::new();
    while parser.current().kind != TokenKind::Eof {
        functions.push(parser.function()?);
    }
    if functions.is_empty() {
        return Err(Diagnostic::new(
            Span { start: 0, end: 0 },
            "expected a function",
        ));
    }
    Ok(Program { functions })
}
