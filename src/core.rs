use std::collections::{HashMap, VecDeque};
use std::fmt;

use crate::syntax::{self, Expr, ExprKind};
use crate::{Diagnostic, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ValueId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FunctionId(pub usize);

#[derive(Clone, Debug)]
pub enum Operation {
    Const(i64),
    Neg(ValueId),
    Add(ValueId, ValueId),
    Sub(ValueId, ValueId),
    Mul(ValueId, ValueId),
    Call(FunctionId, Vec<ValueId>),
}

#[derive(Clone, Debug)]
pub struct Instruction {
    pub result: ValueId,
    pub operation: Operation,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub params: Vec<ValueId>,
    pub instructions: Vec<Instruction>,
    pub return_value: ValueId,
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub block: Block,
}

#[derive(Clone, Debug)]
pub struct Program {
    pub functions: Vec<Function>,
}

fn emit(
    instructions: &mut Vec<Instruction>,
    param_count: usize,
    operation: Operation,
    span: Span,
) -> ValueId {
    let result = ValueId(param_count + instructions.len());
    instructions.push(Instruction {
        result,
        operation,
        span,
    });
    result
}

fn lower_expr(
    expr: &Expr,
    locals: &HashMap<String, ValueId>,
    functions: &HashMap<String, (FunctionId, usize)>,
    instructions: &mut Vec<Instruction>,
    param_count: usize,
) -> Result<ValueId, Diagnostic> {
    let operation = match &expr.kind {
        ExprKind::Int(value) => Operation::Const(*value),
        ExprKind::Name(name) => {
            return locals
                .get(name)
                .copied()
                .ok_or_else(|| Diagnostic::new(expr.span, format!("unknown local `{name}`")));
        }
        ExprKind::Neg(rhs) => Operation::Neg(lower_expr(
            rhs,
            locals,
            functions,
            instructions,
            param_count,
        )?),
        ExprKind::Add(lhs, rhs) => Operation::Add(
            lower_expr(lhs, locals, functions, instructions, param_count)?,
            lower_expr(rhs, locals, functions, instructions, param_count)?,
        ),
        ExprKind::Sub(lhs, rhs) => Operation::Sub(
            lower_expr(lhs, locals, functions, instructions, param_count)?,
            lower_expr(rhs, locals, functions, instructions, param_count)?,
        ),
        ExprKind::Mul(lhs, rhs) => Operation::Mul(
            lower_expr(lhs, locals, functions, instructions, param_count)?,
            lower_expr(rhs, locals, functions, instructions, param_count)?,
        ),
        ExprKind::Call(name, args) => {
            let (target, expected) = functions
                .get(name)
                .copied()
                .ok_or_else(|| Diagnostic::new(expr.span, format!("unknown function `{name}`")))?;
            if args.len() != expected {
                return Err(Diagnostic::new(
                    expr.span,
                    format!("`{name}` expects {expected} arguments, got {}", args.len()),
                ));
            }
            let values = args
                .iter()
                .map(|arg| lower_expr(arg, locals, functions, instructions, param_count))
                .collect::<Result<Vec<_>, _>>()?;
            Operation::Call(target, values)
        }
    };
    Ok(emit(instructions, param_count, operation, expr.span))
}

pub fn lower(ast: &syntax::Program) -> Result<Program, Diagnostic> {
    let mut functions = HashMap::new();
    for (index, function) in ast.functions.iter().enumerate() {
        if function.params.len() > 4 {
            return Err(Diagnostic::new(
                function.name_span,
                "at most four parameters in this implementation slice",
            ));
        }
        if functions
            .insert(
                function.name.clone(),
                (FunctionId(index), function.params.len()),
            )
            .is_some()
        {
            return Err(Diagnostic::new(
                function.name_span,
                format!("duplicate function `{}`", function.name),
            ));
        }
    }
    let mut lowered = Vec::with_capacity(ast.functions.len());
    for function in &ast.functions {
        let mut locals = HashMap::new();
        for (index, (name, span)) in function.params.iter().enumerate() {
            if locals.insert(name.clone(), ValueId(index)).is_some() {
                return Err(Diagnostic::new(
                    *span,
                    format!("duplicate parameter `{name}`"),
                ));
            }
        }
        let mut instructions = Vec::new();
        let param_count = function.params.len();
        for (name, span, expr) in &function.body.lets {
            if locals.contains_key(name) {
                return Err(Diagnostic::new(*span, format!("duplicate local `{name}`")));
            }
            let value = lower_expr(expr, &locals, &functions, &mut instructions, param_count)?;
            locals.insert(name.clone(), value);
        }
        let return_value = lower_expr(
            &function.body.tail,
            &locals,
            &functions,
            &mut instructions,
            param_count,
        )?;
        lowered.push(Function {
            name: function.name.clone(),
            block: Block {
                params: (0..param_count).map(ValueId).collect(),
                instructions,
                return_value,
            },
        });
    }
    let program = Program { functions: lowered };
    verify(&program).map_err(|message| Diagnostic::new(Span { start: 0, end: 0 }, message))?;
    Ok(program)
}

pub fn verify(program: &Program) -> Result<(), String> {
    if program.functions.is_empty() {
        return Err("empty Core IR program".into());
    }
    for function in &program.functions {
        let block = &function.block;
        if block.params.len() > 4 {
            return Err(format!(
                "{}: at most four parameters are supported",
                function.name
            ));
        }
        for (index, param) in block.params.iter().enumerate() {
            if param.0 != index {
                return Err(format!("{}: noncanonical block parameter", function.name));
            }
        }
        for (index, instruction) in block.instructions.iter().enumerate() {
            let available = block.params.len() + index;
            if instruction.result.0 != available {
                return Err(format!("{}: noncanonical value ID", function.name));
            }
            let operands: &[ValueId] = match &instruction.operation {
                Operation::Const(_) => &[],
                Operation::Neg(a) => std::slice::from_ref(a),
                Operation::Add(a, b) | Operation::Sub(a, b) | Operation::Mul(a, b) => {
                    if a.0 >= available || b.0 >= available {
                        return Err(format!("{}: use before definition", function.name));
                    }
                    continue;
                }
                Operation::Call(target, args) => {
                    let Some(callee) = program.functions.get(target.0) else {
                        return Err(format!("{}: invalid call target", function.name));
                    };
                    if args.len() != callee.block.params.len() {
                        return Err(format!("{}: call arity mismatch", function.name));
                    }
                    args
                }
            };
            if operands.iter().any(|value| value.0 >= available) {
                return Err(format!("{}: use before definition", function.name));
            }
        }
        let value_count = block.params.len() + block.instructions.len();
        if block.return_value.0 >= value_count {
            return Err(format!("{}: undefined return value", function.name));
        }
    }
    // Until Phoebe has guarded task stacks, reject recursion rather than
    // allowing native code to exhaust the host thread's stack.
    let mut incoming = vec![0usize; program.functions.len()];
    let mut edges = vec![Vec::new(); program.functions.len()];
    for (caller, function) in program.functions.iter().enumerate() {
        for instruction in &function.block.instructions {
            if let Operation::Call(target, _) = &instruction.operation {
                edges[caller].push(target.0);
                incoming[target.0] += 1;
            }
        }
    }
    let mut ready = incoming
        .iter()
        .enumerate()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| id)
        .collect::<VecDeque<_>>();
    let mut visited = 0;
    while let Some(id) = ready.pop_front() {
        visited += 1;
        for target in &edges[id] {
            incoming[*target] -= 1;
            if incoming[*target] == 0 {
                ready.push_back(*target);
            }
        }
    }
    if visited != program.functions.len() {
        return Err("recursive call cycle is not yet supported".into());
    }
    Ok(())
}

impl Program {
    pub fn function(&self, name: &str) -> Option<(FunctionId, &Function)> {
        self.functions
            .iter()
            .enumerate()
            .find(|(_, function)| function.name == name)
            .map(|(index, function)| (FunctionId(index), function))
    }

    pub fn interpret(&self, name: &str, args: &[i64]) -> Result<i64, String> {
        verify(self)?;
        let (id, _) = self
            .function(name)
            .ok_or_else(|| format!("unknown function `{name}`"))?;
        self.interpret_id(id, args, 0)
    }

    fn interpret_id(&self, id: FunctionId, args: &[i64], depth: usize) -> Result<i64, String> {
        if depth >= 1024 {
            return Err("interpreter call depth exceeded 1024".into());
        }
        let function = &self.functions[id.0];
        if args.len() != function.block.params.len() {
            return Err(format!(
                "`{}` expects {} arguments, got {}",
                function.name,
                function.block.params.len(),
                args.len()
            ));
        }
        let mut values = Vec::with_capacity(args.len() + function.block.instructions.len());
        values.extend_from_slice(args);
        for instruction in &function.block.instructions {
            let value = match &instruction.operation {
                Operation::Const(value) => *value,
                Operation::Neg(a) => values[a.0].wrapping_neg(),
                Operation::Add(a, b) => values[a.0].wrapping_add(values[b.0]),
                Operation::Sub(a, b) => values[a.0].wrapping_sub(values[b.0]),
                Operation::Mul(a, b) => values[a.0].wrapping_mul(values[b.0]),
                Operation::Call(target, args) => {
                    let args = args.iter().map(|value| values[value.0]).collect::<Vec<_>>();
                    self.interpret_id(*target, &args, depth + 1)?
                }
            };
            values.push(value);
        }
        Ok(values[function.block.return_value.0])
    }
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for function in &self.functions {
            write!(f, "fn {}(", function.name)?;
            for (index, param) in function.block.params.iter().enumerate() {
                if index > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "v{}: I64", param.0)?;
            }
            writeln!(f, ") -> I64 {{")?;
            writeln!(f, "  block0:")?;
            for instruction in &function.block.instructions {
                write!(f, "    v{} = ", instruction.result.0)?;
                match &instruction.operation {
                    Operation::Const(value) => write!(f, "const {value}")?,
                    Operation::Neg(a) => write!(f, "neg v{}", a.0)?,
                    Operation::Add(a, b) => write!(f, "add v{}, v{}", a.0, b.0)?,
                    Operation::Sub(a, b) => write!(f, "sub v{}, v{}", a.0, b.0)?,
                    Operation::Mul(a, b) => write!(f, "mul v{}, v{}", a.0, b.0)?,
                    Operation::Call(target, args) => {
                        write!(f, "call {}(", self.functions[target.0].name)?;
                        for (index, arg) in args.iter().enumerate() {
                            if index > 0 {
                                write!(f, ", ")?;
                            }
                            write!(f, "v{}", arg.0)?;
                        }
                        write!(f, ")")?;
                    }
                }
                writeln!(f, "  @{}..{}", instruction.span.start, instruction.span.end)?;
            }
            writeln!(f, "    return v{}", function.block.return_value.0)?;
            writeln!(f, "}}")?;
        }
        Ok(())
    }
}
