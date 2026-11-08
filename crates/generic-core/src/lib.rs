//! Parametric, ownership-checked Core for shape-shared generic definitions.
//!
//! This first IR slice admits opaque type parameters, scalar values, moves,
//! and explicit block arguments. It has no type-specific operations or
//! dictionary calls yet. Every type parameter is treated as affine, even
//! when a later concrete substitution has no communication capabilities.

mod verification;

pub use verification::verify;

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
