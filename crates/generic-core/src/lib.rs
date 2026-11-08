//! Parametric, ownership-checked Core for shape-shared generic definitions.
//!
//! This IR slice admits opaque type parameters, scalar values, moves,
//! explicit block arguments, and owned generic-to-generic calls. It has no
//! type-specific operations or dictionary calls yet. Every type parameter is
//! treated as affine, even when a later concrete substitution has no
//! communication capabilities.

mod display;
mod source;
mod verification;

pub use source::{lower, lower_program, lower_program_checked};
pub use verification::{check_acyclic, verify, verify_module};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ValueId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Param(usize),
    I64,
    Bool,
    Unit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub type_params: usize,
    pub params: Vec<Type>,
    pub result: Type,
    pub value_types: Vec<Type>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    pub functions: Vec<Function>,
}

/// Immutable evidence that every parametric body and call has passed the
/// module verifier. Concrete call sites may use its signatures without
/// checking those bodies again.
#[derive(Clone, Debug)]
pub struct CheckedModule(Module);

impl CheckedModule {
    pub fn check(module: Module) -> Result<Self, String> {
        verify_module(&module)?;
        Ok(Self(module))
    }

    pub fn as_module(&self) -> &Module {
        &self.0
    }

    pub fn into_module(self) -> Module {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub params: Vec<ValueId>,
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub result: ValueId,
    pub operation: Operation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Move(ValueId),
    CopyScalar(ValueId),
    Call {
        callee: usize,
        type_args: Vec<Type>,
        args: Vec<ValueId>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Terminator {
    Return(ValueId),
    Jump {
        target: BlockId,
        args: Vec<ValueId>,
    },
    Branch {
        condition: ValueId,
        when_true: BlockId,
        true_args: Vec<ValueId>,
        when_false: BlockId,
        false_args: Vec<ValueId>,
    },
}

#[cfg(test)]
mod tests;
