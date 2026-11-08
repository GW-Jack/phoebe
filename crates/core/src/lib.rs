use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::task::Poll;

use phoebe_capability::{self as capability_flow, Action as CapAction, Target as CapTarget};
use phoebe_runtime::{
    self as runtime, Context, ReplyReceiverToken, ReplyToken, ResourceError, Scheduler,
    ScopeLimits, Task, Token,
};
use phoebe_syntax::syntax::{
    self, Body, CallArgument, Expr, ExprKind, ParamMode, Statement, TypeName,
};
use phoebe_syntax::{Diagnostic, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ValueId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FunctionId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlaceId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegionId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TaskScopeId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DataId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    I64,
    Bool,
    Unit,
    Data(DataId),
    RefI64,
    RefMutI64,
    SenderI64,
    ReceiverI64,
    ReplyToI64,
    ReplyReceiverI64,
    RequestI64,
    RequestSenderI64,
    RequestReceiverI64,
    ResourceError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    I64(i64),
    Bool(bool),
    Unit,
    Data(DataId, usize, Option<Box<Value>>),
    Product(DataId, Vec<Value>),
    Ref(usize),
    Sender(Token),
    Receiver(Token),
    ReplyTo(ReplyToken),
    ReplyReceiver(ReplyReceiverToken),
    Request(i64, ReplyToken),
    ResourceError(ResourceError),
}

type TypedValue = (ValueId, Type);

fn check_call_aliases(args: &[CallArg]) -> Result<(), String> {
    for (index, arg) in args.iter().enumerate() {
        for other in &args[..index] {
            let (overlap, exclusive) = match (arg, other) {
                (CallArg::Borrow(a, am), CallArg::Borrow(b, bm)) => (a == b, *am || *bm),
                (CallArg::Ref(a, am), CallArg::Ref(b, bm)) => (a == b, *am || *bm),
                _ => (false, false),
            };
            if overlap && exclusive {
                return Err("overlapping call borrows include an exclusive borrow".into());
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Binding {
    Val(ValueId),
    Var(PlaceId),
    Ref(ValueId, ParamMode),
}

#[derive(Clone, Debug)]
pub enum CallArg {
    Value(ValueId),
    Borrow(PlaceId, bool),
    Ref(ValueId, bool),
}

/// A call-scoped loan starts immediately before its call and ends when that
/// call returns or unwinds. Forwarded roots remain bounded by the caller's
/// incoming loan; they do not create a reference that can escape the task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoanRoot {
    Place(PlaceId, bool),
    Forwarded(ValueId, bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoanRegion {
    pub id: RegionId,
    pub call_block: BlockId,
    pub call_instruction: usize,
    pub roots: Vec<LoanRoot>,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Const(i64),
    Bool(bool),
    Unit,
    Move(ValueId),
    Copy(ValueId),
    Variant(DataId, usize, Option<ValueId>),
    Product(DataId, Vec<ValueId>),
    Field(ValueId, usize),
    ReadField(PlaceId, usize),
    Neg(ValueId),
    Not(ValueId),
    Add(ValueId, ValueId),
    Sub(ValueId, ValueId),
    Mul(ValueId, ValueId),
    Eq(ValueId, ValueId),
    Ne(ValueId, ValueId),
    Lt(ValueId, ValueId),
    Le(ValueId, ValueId),
    Gt(ValueId, ValueId),
    Ge(ValueId, ValueId),
    Call(FunctionId, Vec<CallArg>),
    LoadRef(ValueId),
    StoreRef(ValueId, ValueId),
    PlaceInit(PlaceId, ValueId),
    Load(PlaceId),
    Take(PlaceId),
    CopyPlace(PlaceId),
    Store(PlaceId, ValueId),
    CloseHandle(ValueId),
    CloseReply(ValueId),
    PackRequest { value: ValueId, right: ValueId },
    CloseRequest(ValueId),
    ScopeEnter(TaskScopeId),
}

#[derive(Clone, Debug)]
pub struct Instruction {
    pub result: Option<ValueId>,
    pub operation: Operation,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchBinding {
    Discard,
    Payload,
    Whole,
}

#[derive(Clone, Copy, Debug)]
pub struct MatchArm {
    pub target: BlockId,
    pub binding: MatchBinding,
}

#[derive(Clone, Debug)]
pub enum Terminator {
    Return(ValueId),
    Yield {
        resume: BlockId,
        span: Span,
        automatic: bool,
    },
    ScopeJoin {
        scope: TaskScopeId,
        resume: BlockId,
        span: Span,
    },
    Jump(BlockId, Vec<ValueId>),
    Branch {
        condition: ValueId,
        then_block: BlockId,
        else_block: BlockId,
    },
    Match {
        subject: ValueId,
        arms: Vec<MatchArm>,
    },
    ChannelNew {
        capacity: ValueId,
        success: BlockId,
        failure: BlockId,
    },
    ReplyNew {
        success: BlockId,
        failure: BlockId,
    },
    RequestChannelNew {
        capacity: ValueId,
        success: BlockId,
        failure: BlockId,
    },
    RequestUnpack {
        request: ValueId,
        next: BlockId,
    },
    RequestSend {
        sender: ValueId,
        request: ValueId,
        sent: BlockId,
        closed: BlockId,
    },
    RequestReceive {
        receiver: ValueId,
        value: BlockId,
        end: BlockId,
    },
    Fork {
        source: ValueId,
        success: BlockId,
        failure: BlockId,
    },
    ChannelSend {
        sender: ValueId,
        message: ValueId,
        sent: BlockId,
        closed: BlockId,
    },
    ChannelReceive {
        receiver: ValueId,
        value: BlockId,
        end: BlockId,
    },
    ReplyComplete {
        right: ValueId,
        value: ValueId,
        delivered: BlockId,
        closed: BlockId,
    },
    ReplyReceive {
        receiver: ValueId,
        value: BlockId,
        end: BlockId,
    },
    Spawn {
        child: FunctionId,
        args: Vec<ValueId>,
        success: BlockId,
        failure: BlockId,
        span: Span,
    },
}

#[derive(Clone, Debug)]
pub struct Block {
    pub params: Vec<ValueId>,
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub param_modes: Vec<ParamMode>,
    pub return_type: Type,
    pub blocks: Vec<Block>,
    pub value_types: Vec<Type>,
    pub place_types: Vec<Type>,
    pub loan_regions: Vec<LoanRegion>,
}

impl Function {
    /// Visit every source span stored in a checked function. Incremental
    /// rebasing and native cache keys must use the same complete traversal.
    pub fn for_each_source_span_mut(&mut self, mut visit: impl FnMut(&mut Span)) {
        for block in &mut self.blocks {
            for instruction in &mut block.instructions {
                visit(&mut instruction.span);
            }
            match &mut block.terminator {
                Terminator::Yield { span, .. }
                | Terminator::ScopeJoin { span, .. }
                | Terminator::Spawn { span, .. } => visit(span),
                Terminator::Return(_)
                | Terminator::Jump(_, _)
                | Terminator::Branch { .. }
                | Terminator::Match { .. }
                | Terminator::ChannelNew { .. }
                | Terminator::ReplyNew { .. }
                | Terminator::RequestChannelNew { .. }
                | Terminator::RequestUnpack { .. }
                | Terminator::RequestSend { .. }
                | Terminator::RequestReceive { .. }
                | Terminator::Fork { .. }
                | Terminator::ChannelSend { .. }
                | Terminator::ChannelReceive { .. }
                | Terminator::ReplyComplete { .. }
                | Terminator::ReplyReceive { .. } => {}
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Program {
    pub data: Vec<DataDef>,
    pub functions: Vec<Function>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CostEvent {
    pub function: String,
    pub block: usize,
    pub instruction: usize,
    pub span: Span,
    pub kind: &'static str,
    pub type_name: String,
    pub reason: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataDef {
    pub name: String,
    pub variants: Vec<String>,
    pub payloads: Vec<Option<Type>>,
    pub fields: Vec<(String, Type)>,
}

mod interpreter;
mod lowering;
mod scope_verification;
mod verification;

pub use lowering::{lower, lower_reusing};
use verification::{
    capability_graph, recursive_call_span, verify_capability_edges, verify_ownership,
    verify_structure,
};
pub use verification::{operands, verify};
