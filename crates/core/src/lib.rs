use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::task::Poll;

use phoebe_capability::{self as capability_flow, Action as CapAction, Target as CapTarget};
use phoebe_runtime::{
    self as runtime, Context, ResourceError, Scheduler, ScopeLimits, Task, Token,
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
pub struct DataId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    I64,
    Bool,
    Data(DataId),
    RefI64,
    RefMutI64,
    SenderI64,
    ReceiverI64,
    ResourceError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    I64(i64),
    Bool(bool),
    Data(DataId, usize, Option<Box<Value>>),
    Product(DataId, Vec<Value>),
    Ref(usize),
    Sender(Token),
    Receiver(Token),
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

#[derive(Clone, Debug)]
pub enum Operation {
    Const(i64),
    Bool(bool),
    Move(ValueId),
    Copy(ValueId),
    Variant(DataId, usize, Option<ValueId>),
    Product(DataId, Vec<ValueId>),
    Field(ValueId, usize),
    ReadField(PlaceId, usize),
    Neg(ValueId),
    Add(ValueId, ValueId),
    Sub(ValueId, ValueId),
    Mul(ValueId, ValueId),
    Eq(ValueId, ValueId),
    Lt(ValueId, ValueId),
    Call(FunctionId, Vec<CallArg>),
    LoadRef(ValueId),
    StoreRef(ValueId, ValueId),
    PlaceInit(PlaceId, ValueId),
    Load(PlaceId),
    Take(PlaceId),
    CopyPlace(PlaceId),
    Store(PlaceId, ValueId),
    CloseHandle(ValueId),
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
    Spawn {
        child: FunctionId,
        args: Vec<ValueId>,
        success: BlockId,
        failure: BlockId,
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
mod verification;

pub use lowering::{lower, lower_reusing};
use verification::{capability_graph, verify_capability_edges, verify_ownership, verify_structure};
pub use verification::{operands, verify};
