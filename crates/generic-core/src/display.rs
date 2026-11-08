use std::fmt;

use crate::{Function, Module, Operation, Terminator, Type};

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Param(index) => write!(f, "T{index}"),
            Self::I64 => f.write_str("I64"),
            Self::Bool => f.write_str("Bool"),
            Self::Unit => f.write_str("Unit"),
        }
    }
}

fn values(f: &mut fmt::Formatter<'_>, values: &[crate::ValueId]) -> fmt::Result {
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            f.write_str(", ")?;
        }
        write!(f, "v{}", value.0)?;
    }
    Ok(())
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fn {}<", self.name)?;
        for index in 0..self.type_params {
            if index != 0 {
                f.write_str(", ")?;
            }
            write!(f, "T{index}")?;
        }
        f.write_str(">(")?;
        for (index, ty) in self.params.iter().enumerate() {
            if index != 0 {
                f.write_str(", ")?;
            }
            write!(f, "v{index}: {ty}")?;
        }
        writeln!(f, ") -> {} {{", self.result)?;
        for (index, block) in self.blocks.iter().enumerate() {
            write!(f, "  b{index}(")?;
            for (offset, value) in block.params.iter().enumerate() {
                if offset != 0 {
                    f.write_str(", ")?;
                }
                match self.value_types.get(value.0) {
                    Some(ty) => write!(f, "v{}: {ty}", value.0)?,
                    None => write!(f, "v{}: <invalid>", value.0)?,
                }
            }
            f.write_str("):\n")?;
            for instruction in &block.instructions {
                match &instruction.operation {
                    Operation::Move(value) => {
                        writeln!(f, "    v{} = move v{}", instruction.result.0, value.0)?;
                    }
                    Operation::CopyScalar(value) => {
                        writeln!(
                            f,
                            "    v{} = copy.scalar v{}",
                            instruction.result.0, value.0
                        )?;
                    }
                    Operation::Call {
                        callee,
                        type_args,
                        args,
                    } => {
                        write!(f, "    v{} = call f{}<", instruction.result.0, callee)?;
                        for (index, ty) in type_args.iter().enumerate() {
                            if index != 0 {
                                f.write_str(", ")?;
                            }
                            write!(f, "{ty}")?;
                        }
                        f.write_str(">(")?;
                        values(f, args)?;
                        f.write_str(")\n")?;
                    }
                }
            }
            match &block.terminator {
                Terminator::Return(value) => writeln!(f, "    return v{}", value.0)?,
                Terminator::Jump { target, args } => {
                    write!(f, "    jump b{}(", target.0)?;
                    values(f, args)?;
                    f.write_str(")\n")?;
                }
                Terminator::Branch {
                    condition,
                    when_true,
                    true_args,
                    when_false,
                    false_args,
                } => {
                    write!(f, "    branch v{} ? b{}(", condition.0, when_true.0)?;
                    values(f, true_args)?;
                    write!(f, ") : b{}(", when_false.0)?;
                    values(f, false_args)?;
                    f.write_str(")\n")?;
                }
            }
        }
        f.write_str("}")
    }
}

impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, function) in self.functions.iter().enumerate() {
            if index != 0 {
                f.write_str("\n")?;
            }
            writeln!(f, "f{index} = {function}")?;
        }
        Ok(())
    }
}
