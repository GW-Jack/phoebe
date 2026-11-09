use crate::Span;

#[derive(Clone, Debug)]
pub struct Program {
    pub imports: Vec<Import>,
    pub data: Vec<Data>,
    pub functions: Vec<Function>,
    pub applied_types: Vec<(TypeName, Span)>,
    pub uses_send_outcome: bool,
    pub uses_receive_outcome: bool,
    pub uses_complete_outcome: bool,
    pub uses_reply_outcome: bool,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub name: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Data {
    pub name: String,
    pub public: bool,
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
    pub public: bool,
    pub name_span: Span,
    pub type_params: Vec<(String, Span)>,
    pub params: Vec<Param>,
    pub return_type: TypeName,
    pub body: Body,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeName {
    I64,
    ArrayI64(usize),
    Bool,
    Unit,
    SenderI64,
    ReceiverI64,
    ReplyToI64,
    ReplyReceiverI64,
    RequestI64,
    RequestSenderI64,
    RequestReceiverI64,
    TypedSender(Box<TypeName>),
    TypedReceiver(Box<TypeName>),
    Send(Box<TypeName>),
    Receive(Box<TypeName>),
    Named(String),
    Option(Box<TypeName>),
    Result(Box<TypeName>, Box<TypeName>),
}

impl TypeName {
    pub fn key(&self) -> String {
        match self {
            Self::I64 => "I64".into(),
            Self::ArrayI64(len) => format!("[I64; {len}]"),
            Self::Bool => "Bool".into(),
            Self::Unit => "Unit".into(),
            Self::SenderI64 => "Sender<I64>".into(),
            Self::ReceiverI64 => "Receiver<I64>".into(),
            Self::ReplyToI64 => "ReplyTo<I64>".into(),
            Self::ReplyReceiverI64 => "ReplyReceiver<I64>".into(),
            Self::RequestI64 => "Request<I64>".into(),
            Self::RequestSenderI64 => "RequestSender<I64>".into(),
            Self::RequestReceiverI64 => "RequestReceiver<I64>".into(),
            Self::TypedSender(value) => format!("Sender<{}>", value.key()),
            Self::TypedReceiver(value) => format!("Receiver<{}>", value.key()),
            Self::Send(value) => format!("Send<{}>", value.key()),
            Self::Receive(value) => format!("Receive<{}>", value.key()),
            Self::Named(name) => name.clone(),
            Self::Option(value) => format!("Option<{}>", value.key()),
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
        mutable: bool,
        message: TypeName,
        sender: (String, Span),
        receiver: (String, Span),
        capacity: Expr,
        span: Span,
    },
    ReplyVal {
        mutable: bool,
        right: (String, Span),
        receiver: (String, Span),
        span: Span,
    },
    RequestChannelVal {
        mutable: bool,
        sender: (String, Span),
        receiver: (String, Span),
        capacity: Expr,
        span: Span,
    },
    UnpackVal {
        mutable: bool,
        names: Vec<(String, Span)>,
        value: Expr,
        span: Span,
    },
    Spawn {
        child: String,
        args: Vec<CallArgument>,
        span: Span,
    },
    Scope {
        span: Span,
        body: Body,
    },
    With {
        name: String,
        span: Span,
        value: Expr,
        body: Body,
    },
    Yield(Span),
    Var(String, Span, Expr),
    Assign(String, Span, Expr),
    ArrayAssign(String, usize, Span, Expr),
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
    FixedArray(Vec<Expr>),
    FixedIndex(Box<Expr>, usize),
    Name(String),
    Borrow {
        name: String,
        mutable: bool,
    },
    Copy(Box<Expr>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Eq(Box<Expr>, Box<Expr>),
    Ne(Box<Expr>, Box<Expr>),
    Lt(Box<Expr>, Box<Expr>),
    Le(Box<Expr>, Box<Expr>),
    Gt(Box<Expr>, Box<Expr>),
    Ge(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
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
