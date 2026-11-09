use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::task::Poll;

pub mod path_language;

use phoebe_capability::{self as capability_flow, Action as CapAction, Target as CapTarget};
use phoebe_generic_core::CheckedModule;
pub use phoebe_layout::{DataDef, DataId, MessageType, Type};
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    I64(i64),
    Bool(bool),
    Unit,
    ArrayI64(ArrayValue),
    Data(DataId, usize, Option<Box<Value>>),
    Product(DataId, Vec<Value>),
    UnpublishedProduct(DataId, Vec<Option<Value>>),
    Ref(usize),
    RefPath(usize, Vec<usize>),
    Sender(Token),
    Receiver(Token),
    ReplyTo(ReplyToken),
    ReplyReceiver(ReplyReceiverToken),
    Request(i64, ReplyToken),
    ResourceError(ResourceError),
}

/// Interpreter storage for a fixed array. The source value has the same
/// semantics in either representation; larger arrays can share immutable
/// storage until a place is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArrayValue {
    Inline(Vec<i64>),
    Shared(runtime::SharedWords),
}

impl ArrayValue {
    pub fn from_slice(words: &[i64]) -> Result<Self, String> {
        if words.len() < 16 {
            Ok(Self::Inline(words.to_vec()))
        } else {
            runtime::SharedWords::try_new(words)
                .map(Self::Shared)
                .map_err(|error| format!("array allocation failed: {error:?}"))
        }
    }

    pub fn as_slice(&self) -> &[i64] {
        match self {
            Self::Inline(words) => words,
            Self::Shared(words) => words.as_slice(),
        }
    }

    pub fn set(&mut self, index: usize, value: i64) -> Result<(), String> {
        match self {
            Self::Inline(words) => {
                let slot = words.get_mut(index).ok_or("array index is out of bounds")?;
                *slot = value;
                Ok(())
            }
            Self::Shared(words) => words
                .try_set(index, value)
                .map(|_| ())
                .map_err(|error| format!("array update failed: {error:?}")),
        }
    }
}

type TypedValue = (ValueId, Type);

fn call_arg_place_path(arg: &CallArg) -> Option<(PlaceId, &[usize], bool)> {
    match arg {
        CallArg::Borrow(place, exclusive) => Some((*place, &[][..], *exclusive)),
        CallArg::BorrowPath(place, path, exclusive) => Some((*place, path.as_slice(), *exclusive)),
        _ => None,
    }
}

fn check_call_aliases(args: &[CallArg]) -> Result<(), String> {
    for (index, arg) in args.iter().enumerate() {
        for other in &args[..index] {
            let (overlap, exclusive) = if let (Some((a, ap, am)), Some((b, bp, bm))) =
                (call_arg_place_path(arg), call_arg_place_path(other))
            {
                (
                    a == b && path_language::PathLanguage::subtrees_overlap(ap, bp),
                    am || bm,
                )
            } else if let (CallArg::Ref(a, am), CallArg::Ref(b, bm)) = (arg, other) {
                (a == b, *am || *bm)
            } else {
                (false, false)
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
    BorrowPath(PlaceId, Vec<usize>, bool),
    Ref(ValueId, bool),
}

/// A call-scoped loan starts immediately before its call and ends when that
/// call returns or unwinds. Forwarded roots remain bounded by the caller's
/// incoming loan; they do not create a reference that can escape the task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoanRoot {
    Place(PlaceId, bool),
    Path(PlaceId, Vec<usize>, bool),
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
    ArrayI64(Vec<ValueId>),
    ArrayGet(ValueId, usize),
    ArrayLen(ValueId),
    Move(ValueId),
    Copy(ValueId),
    InferredCopy(ValueId),
    Variant(DataId, usize, Option<ValueId>),
    Product(DataId, Vec<ValueId>),
    DestBegin(PlaceId),
    DestField(PlaceId, usize, ValueId),
    DestPublish(PlaceId),
    DestTakeField(PlaceId, usize),
    Field(ValueId, usize),
    ReadField(PlaceId, usize),
    ArrayGetPlace(PlaceId, usize),
    ArraySetPlace(PlaceId, usize, ValueId),
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
    GenericCall {
        callee: usize,
        type_args: Vec<Type>,
        args: Vec<ValueId>,
    },
    BorrowBegin(PlaceId, bool),
    BorrowBeginPath(PlaceId, Vec<usize>, bool),
    BorrowReborrow(ValueId, bool),
    BorrowEnd(ValueId),
    LoadRef(ValueId),
    StoreRef(ValueId, ValueId),
    PlaceInit(PlaceId, ValueId),
    Load(PlaceId),
    Take(PlaceId),
    CopyPlace(PlaceId),
    InferredCopyPlace(PlaceId),
    Store(PlaceId, ValueId),
    CloseHandle(ValueId),
    CloseReply(ValueId),
    PackRequest {
        value: ValueId,
        right: ValueId,
    },
    CloseRequest(ValueId),
    CloseCaps(ValueId),
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
        span: Span,
    },
    TypedChannelNew {
        message: MessageType,
        capacity: ValueId,
        success: BlockId,
        failure: BlockId,
        span: Span,
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
    ProductUnpack {
        product: ValueId,
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
    TypedSend {
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
    TypedReceive {
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
                | Terminator::Spawn { span, .. }
                | Terminator::ChannelNew { span, .. }
                | Terminator::TypedChannelNew { span, .. } => visit(span),
                Terminator::Return(_)
                | Terminator::Jump(_, _)
                | Terminator::Branch { .. }
                | Terminator::Match { .. }
                | Terminator::ReplyNew { .. }
                | Terminator::RequestChannelNew { .. }
                | Terminator::RequestUnpack { .. }
                | Terminator::ProductUnpack { .. }
                | Terminator::RequestSend { .. }
                | Terminator::RequestReceive { .. }
                | Terminator::Fork { .. }
                | Terminator::ChannelSend { .. }
                | Terminator::TypedSend { .. }
                | Terminator::ChannelReceive { .. }
                | Terminator::TypedReceive { .. }
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

/// An immutable program that has passed every Core verifier. Its fields are
/// private so callers cannot invalidate the proof before code generation.
pub struct CheckedProgram(Program);

/// A concrete program and a separately verified parametric module whose
/// generic call signatures have been checked together.
pub struct CheckedLinkedProgram {
    program: Program,
    generics: CheckedModule,
}

impl CheckedLinkedProgram {
    pub fn check(program: Program, generics: CheckedModule) -> Result<Self, String> {
        phoebe_generic_core::check_acyclic(generics.as_module())?;
        verification::verify_linked(&program, &generics)?;
        Ok(Self { program, generics })
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    pub fn generics(&self) -> &CheckedModule {
        &self.generics
    }
}

impl CheckedProgram {
    pub fn check(program: Program) -> Result<Self, String> {
        verify(&program)?;
        Ok(Self(program))
    }

    pub fn as_program(&self) -> &Program {
        &self.0
    }

    /// Build the immutable descriptor catalog once for all concrete channel
    /// types in this checked program.
    pub fn message_catalog(&self) -> Result<phoebe_message::MessageCatalog, String> {
        phoebe_message::MessageCatalog::new(&self.0)
    }

    pub fn into_program(self) -> Program {
        self.0
    }

    pub fn verified(&self) -> VerifiedProgram<'_> {
        VerifiedProgram(&self.0)
    }
}

/// A non-mutable borrow carrying the same verifier guarantee. A raw Program
/// can enter this boundary only by running the verifier.
#[derive(Clone, Copy)]
pub struct VerifiedProgram<'a>(&'a Program);

impl<'a> VerifiedProgram<'a> {
    pub fn check(program: &'a Program) -> Result<Self, String> {
        verify(program)?;
        Ok(Self(program))
    }

    pub fn as_program(self) -> &'a Program {
        self.0
    }
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

impl phoebe_layout::DataDefinitions for Program {
    fn definitions(&self) -> &[DataDef] {
        &self.data
    }
}

mod cost;
pub mod generic_calls;
mod interpreter;
mod lowering;
mod scope_verification;
mod verification;

pub use lowering::{
    lower, lower_checked, lower_linked_checked, lower_linked_reusing, lower_reusing,
};
use verification::{capability_graph, recursive_call_span, verify_ownership, verify_structure};
pub use verification::{lexical_roots, operands, verify};

#[cfg(test)]
mod message_catalog_tests {
    use super::*;

    #[test]
    fn checked_core_exposes_one_catalog_for_nominal_message_descriptors() {
        let source = "data Payload { value: I64, reply: ReplyTo<I64> }\nfn main() -> I64 = 0\n";
        let checked = lower_checked(&phoebe_syntax::parse(source).unwrap()).unwrap();
        let catalog = checked.message_catalog().unwrap();
        let descriptor = catalog.descriptor(Type::Data(DataId(0))).unwrap();
        assert_eq!(descriptor.width, 2);
        assert_eq!(
            descriptor.rights,
            runtime::TypedRightPlan::ReplyTo { part: 1 }
        );
    }
}
