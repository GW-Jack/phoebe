use crate::{Diagnostic, Span};

#[derive(Clone, Debug, PartialEq, Eq)]
enum TokenKind {
    Fn,
    Import,
    Data,
    When,
    Val,
    Var,
    Mut,
    Copy,
    While,
    Spawn,
    With,
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
    Dot,
    Colon,
    Semicolon,
    Arrow,
    Equal,
    EqualEqual,
    Less,
    Greater,
    Plus,
    Minus,
    Star,
    Amp,
    Question,
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
                    "import" => TokenKind::Import,
                    "data" => TokenKind::Data,
                    "when" => TokenKind::When,
                    "val" => TokenKind::Val,
                    "var" => TokenKind::Var,
                    "mut" => TokenKind::Mut,
                    "copy" => TokenKind::Copy,
                    "while" => TokenKind::While,
                    "spawn" => TokenKind::Spawn,
                    "with" => TokenKind::With,
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
            b'.' => {
                i += 1;
                TokenKind::Dot
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
            b'>' => {
                i += 1;
                TokenKind::Greater
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
            b'?' => {
                i += 1;
                TokenKind::Question
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
    pub imports: Vec<Import>,
    pub data: Vec<Data>,
    pub functions: Vec<Function>,
    pub applied_results: Vec<(TypeName, Span)>,
    pub uses_send_outcome: bool,
    pub uses_receive_outcome: bool,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub name: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Data {
    pub name: String,
    pub span: Span,
    pub variants: Vec<Variant>,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug)]
pub struct Variant {
    pub name: String,
    pub span: Span,
    pub payload: Option<Field>,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub ty: TypeName,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub name_span: Span,
    pub params: Vec<Param>,
    pub return_type: TypeName,
    pub body: Body,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeName {
    I64,
    Bool,
    SenderI64,
    ReceiverI64,
    Named(String),
    Result(Box<TypeName>, Box<TypeName>),
}

impl TypeName {
    pub fn key(&self) -> String {
        match self {
            Self::I64 => "I64".into(),
            Self::Bool => "Bool".into(),
            Self::SenderI64 => "Sender<I64>".into(),
            Self::ReceiverI64 => "Receiver<I64>".into(),
            Self::Named(name) => name.clone(),
            Self::Result(ok, err) => format!("Result<{},{}>", ok.key(), err.key()),
        }
    }
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
    pub ty: TypeName,
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
    ChannelVal {
        sender: (String, Span),
        receiver: (String, Span),
        capacity: Expr,
        span: Span,
    },
    Spawn {
        child: String,
        args: Vec<CallArgument>,
        span: Span,
    },
    With {
        name: String,
        span: Span,
        value: Expr,
        body: Body,
    },
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
    Copy(Box<Expr>),
    Neg(Box<Expr>),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Eq(Box<Expr>, Box<Expr>),
    Lt(Box<Expr>, Box<Expr>),
    If(Box<Expr>, Box<Body>, Box<Body>),
    Member(String, String),
    VariantCall(String, String, Box<Expr>),
    QualifiedCall(String, String, Vec<CallArgument>),
    Try(Box<Expr>),
    When(Box<Expr>, Vec<WhenArm>),
    With {
        name: String,
        value: Box<Expr>,
        body: Box<Body>,
    },
    Call(String, Vec<CallArgument>),
}

#[derive(Clone, Debug)]
pub struct WhenArm {
    pub type_name: String,
    pub variant: String,
    pub span: Span,
    pub binding: Option<(String, Span)>,
    pub body: Body,
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    applied_results: Vec<(TypeName, Span)>,
    uses_send_outcome: bool,
    uses_receive_outcome: bool,
}

impl Parser {
    fn type_name(&mut self, borrowed: bool) -> Result<TypeName, Diagnostic> {
        let (name, span) = self.name()?;
        if matches!(name.as_str(), "Sender" | "Receiver") {
            self.expect(&TokenKind::Less, "< after channel endpoint type")?;
            let (element, element_span) = self.name()?;
            if element != "I64" {
                return Err(Diagnostic::new(
                    element_span,
                    "only I64 channels are implemented",
                ));
            }
            let end = self.expect(&TokenKind::Greater, ">")?.span;
            if borrowed {
                return Err(Diagnostic::new(
                    span.join(end),
                    "endpoint borrows are not implemented",
                ));
            }
            return Ok(if name == "Sender" {
                TypeName::SenderI64
            } else {
                TypeName::ReceiverI64
            });
        }
        if matches!(name.as_str(), "Send" | "Receive") {
            if borrowed {
                return Err(Diagnostic::new(
                    span,
                    "channel outcome borrows are not implemented",
                ));
            }
            return self.channel_outcome_type_after_name(&name, span);
        }
        if name == "Result" && self.eat(&TokenKind::Less).is_some() {
            let ok = self.type_name(false)?;
            self.expect(&TokenKind::Comma, ",")?;
            let err = self.type_name(false)?;
            let end = self.expect(&TokenKind::Greater, ">")?.span;
            if borrowed {
                return Err(Diagnostic::new(
                    span.join(end),
                    "Result borrows are not implemented",
                ));
            }
            let ty = TypeName::Result(Box::new(ok), Box::new(err));
            self.applied_results.push((ty.clone(), span.join(end)));
            return Ok(ty);
        }
        if name == "Result" {
            return Err(Diagnostic::new(span, "Result requires two type arguments"));
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
            "Bool" if !borrowed => Ok(TypeName::Bool),
            "Bool" => Err(Diagnostic::new(span, "Bool borrows are not implemented")),
            _ if !borrowed => Ok(TypeName::Named(name)),
            _ => Err(Diagnostic::new(span, "only I64 borrows are implemented")),
        }
    }

    fn result_type_after_name(&mut self, start: Span) -> Result<TypeName, Diagnostic> {
        self.expect(&TokenKind::Less, "<")?;
        let ok = self.type_name(false)?;
        self.expect(&TokenKind::Comma, ",")?;
        let err = self.type_name(false)?;
        let end = self.expect(&TokenKind::Greater, ">")?.span;
        let ty = TypeName::Result(Box::new(ok), Box::new(err));
        self.applied_results.push((ty.clone(), start.join(end)));
        Ok(ty)
    }

    fn channel_outcome_type_after_name(
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
        if name == "Send" {
            self.uses_send_outcome = true;
        } else {
            self.uses_receive_outcome = true;
        }
        Ok(TypeName::Named(format!("{name}<I64>")))
    }

    fn data(&mut self) -> Result<Data, Diagnostic> {
        self.expect(&TokenKind::Data, "data")?;
        let (name, span) = self.name()?;
        self.expect(&TokenKind::LBrace, "{")?;
        let mut variants = Vec::new();
        let mut fields = Vec::new();
        let mut product = None;
        loop {
            let (member, member_span) = self.name()?;
            let field = self.eat(&TokenKind::Colon).is_some();
            if product.is_some_and(|prior| prior != field) {
                return Err(Diagnostic::new(
                    member_span,
                    "cannot mix fields and variants",
                ));
            }
            product = Some(field);
            if field {
                let ty = self.type_name(false)?;
                fields.push(Field {
                    name: member,
                    ty,
                    span: member_span,
                });
            } else {
                let payload = if self.eat(&TokenKind::LParen).is_some() {
                    let (name, span) = self.name()?;
                    self.expect(&TokenKind::Colon, ":")?;
                    let ty = self.type_name(false)?;
                    self.expect(&TokenKind::RParen, ")")?;
                    Some(Field { name, ty, span })
                } else {
                    None
                };
                variants.push(Variant {
                    name: member,
                    span: member_span,
                    payload,
                });
            }
            if self.eat(&TokenKind::Comma).is_none() {
                self.expect(&TokenKind::RBrace, "}")?;
                break;
            }
            if self.eat(&TokenKind::RBrace).is_some() {
                break;
            }
        }
        Ok(Data {
            name,
            span,
            variants,
            fields,
        })
    }

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

    fn call_arguments(&mut self) -> Result<(Vec<CallArgument>, Span), Diagnostic> {
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
        Ok((args, end))
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
                let ty = self.type_name(mode != ParamMode::Own)?;
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
        let return_type = self.type_name(false)?;
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
                if !mutable && self.eat(&TokenKind::LParen).is_some() {
                    let sender = self.name()?;
                    self.expect(&TokenKind::Comma, ",")?;
                    let receiver = self.name()?;
                    self.expect(&TokenKind::RParen, ")")?;
                    self.expect(&TokenKind::Equal, "=")?;
                    let (constructor, start) = self.name()?;
                    if constructor != "channel" {
                        return Err(Diagnostic::new(
                            start,
                            "pair binding currently requires channel",
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
                    statements.push(Statement::ChannelVal {
                        sender,
                        receiver,
                        capacity,
                        span: start.join(question.span),
                    });
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
            if let Some(start) = self.eat(&TokenKind::With) {
                let (name, _) = self.name()?;
                self.expect(&TokenKind::Equal, "= after with binding")?;
                let value = self.expr(0)?;
                let body = self.block(false)?;
                let span = start.span.join(self.tokens[self.pos - 1].span);
                if body.tail.is_some() {
                    let expr = Expr {
                        kind: ExprKind::With {
                            name,
                            value: Box::new(value),
                            body: Box::new(body),
                        },
                        span,
                    };
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
                    statements.push(Statement::With {
                        name,
                        span,
                        value,
                        body,
                    });
                }
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
                kind: TokenKind::With,
                span,
            } => {
                self.pos += 1;
                let (name, _) = self.name()?;
                self.expect(&TokenKind::Equal, "= after with binding")?;
                let value = self.expr(0)?;
                let body = self.block(true)?;
                let end = self.tokens[self.pos - 1].span;
                Expr {
                    kind: ExprKind::With {
                        name,
                        value: Box::new(value),
                        body: Box::new(body),
                    },
                    span: span.join(end),
                }
            }
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
                    let type_name =
                        if type_name == "Result" && self.current().kind == TokenKind::Less {
                            self.result_type_after_name(start)?.key()
                        } else if matches!(type_name.as_str(), "Send" | "Receive") {
                            self.channel_outcome_type_after_name(&type_name, start)?
                                .key()
                        } else {
                            type_name
                        };
                    self.expect(&TokenKind::Dot, ".")?;
                    let (variant, end) = self.name()?;
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
                let name = if name == "Result" && self.current().kind == TokenKind::Less {
                    self.result_type_after_name(span)?.key()
                } else if matches!(name.as_str(), "Send" | "Receive") {
                    self.channel_outcome_type_after_name(&name, span)?.key()
                } else {
                    name
                };
                if self.eat(&TokenKind::Dot).is_some() {
                    let (variant, end) = self.name()?;
                    if self.eat(&TokenKind::LParen).is_some() {
                        let (args, close) = self.call_arguments()?;
                        if variant == "send" {
                            self.uses_send_outcome = true;
                        } else if variant == "receive" {
                            self.uses_receive_outcome = true;
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
            if self.current().kind == TokenKind::Question && min_bp <= 9 {
                let end = self.current().span;
                self.pos += 1;
                lhs = Expr {
                    span: lhs.span.join(end),
                    kind: ExprKind::Try(Box::new(lhs)),
                };
                continue;
            }
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
        applied_results: Vec::new(),
        uses_send_outcome: false,
        uses_receive_outcome: false,
    };
    let mut functions = Vec::new();
    let mut data = Vec::new();
    let mut imports = Vec::new();
    while parser.current().kind != TokenKind::Eof {
        if parser.current().kind == TokenKind::Import {
            parser.pos += 1;
            let (name, span) = parser.name()?;
            imports.push(Import { name, span });
        } else if parser.current().kind == TokenKind::Data {
            data.push(parser.data()?);
        } else {
            functions.push(parser.function()?);
        }
    }
    if functions.is_empty() && data.is_empty() {
        return Err(Diagnostic::new(
            Span { start: 0, end: 0 },
            "expected a function",
        ));
    }
    Ok(Program {
        imports,
        data,
        functions,
        applied_results: parser.applied_results,
        uses_send_outcome: parser.uses_send_outcome,
        uses_receive_outcome: parser.uses_receive_outcome,
    })
}
