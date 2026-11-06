use std::collections::{HashMap, VecDeque};
use std::fmt;

use crate::syntax::{self, Body, Expr, ExprKind};
use crate::{Diagnostic, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ValueId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FunctionId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    I64,
    Bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    I64(i64),
    Bool(bool),
}

type TypedValue = (ValueId, Type);

#[derive(Clone, Debug)]
pub enum Operation {
    Const(i64),
    Bool(bool),
    Neg(ValueId),
    Add(ValueId, ValueId),
    Sub(ValueId, ValueId),
    Mul(ValueId, ValueId),
    Eq(ValueId, ValueId),
    Lt(ValueId, ValueId),
    Call(FunctionId, Vec<ValueId>),
}

#[derive(Clone, Debug)]
pub struct Instruction {
    pub result: ValueId,
    pub operation: Operation,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Terminator {
    Return(ValueId),
    Jump(BlockId, Vec<ValueId>),
    Branch {
        condition: ValueId,
        then_block: BlockId,
        else_block: BlockId,
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
    pub blocks: Vec<Block>,
    pub value_types: Vec<Type>,
}

#[derive(Clone, Debug)]
pub struct Program {
    pub functions: Vec<Function>,
}

struct DraftBlock {
    params: Vec<ValueId>,
    instructions: Vec<Instruction>,
    terminator: Option<Terminator>,
}

struct Lowerer<'a> {
    functions: &'a HashMap<String, (FunctionId, usize)>,
    blocks: Vec<DraftBlock>,
    value_types: Vec<Type>,
    current: BlockId,
}

impl<'a> Lowerer<'a> {
    fn new(functions: &'a HashMap<String, (FunctionId, usize)>, param_count: usize) -> Self {
        let mut lowerer = Self {
            functions,
            blocks: vec![DraftBlock {
                params: Vec::new(),
                instructions: Vec::new(),
                terminator: None,
            }],
            value_types: Vec::new(),
            current: BlockId(0),
        };
        for _ in 0..param_count {
            let id = lowerer.value(Type::I64);
            lowerer.blocks[0].params.push(id);
        }
        lowerer
    }

    fn value(&mut self, ty: Type) -> ValueId {
        let id = ValueId(self.value_types.len());
        self.value_types.push(ty);
        id
    }

    fn block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len());
        self.blocks.push(DraftBlock {
            params: Vec::new(),
            instructions: Vec::new(),
            terminator: None,
        });
        id
    }

    fn block_param(&mut self, block: BlockId, ty: Type) -> ValueId {
        let id = self.value(ty);
        self.blocks[block.0].params.push(id);
        id
    }

    fn emit(&mut self, operation: Operation, ty: Type, span: Span) -> ValueId {
        let result = self.value(ty);
        self.blocks[self.current.0].instructions.push(Instruction {
            result,
            operation,
            span,
        });
        result
    }

    fn terminate(&mut self, terminator: Terminator) {
        assert!(self.blocks[self.current.0].terminator.is_none());
        self.blocks[self.current.0].terminator = Some(terminator);
    }

    fn require(&self, actual: Type, expected: Type, span: Span) -> Result<(), Diagnostic> {
        if actual == expected {
            Ok(())
        } else {
            Err(Diagnostic::new(
                span,
                format!("expected {expected:?}, got {actual:?}"),
            ))
        }
    }

    fn body(
        &mut self,
        body: &Body,
        locals: &mut HashMap<String, ValueId>,
    ) -> Result<(ValueId, Type), Diagnostic> {
        for (name, span, expr) in &body.lets {
            if locals.contains_key(name) {
                return Err(Diagnostic::new(*span, format!("duplicate local {name}")));
            }
            let (value, _) = self.expr(expr, locals)?;
            locals.insert(name.clone(), value);
        }
        self.expr(&body.tail, locals)
    }

    fn pair(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
        locals: &HashMap<String, ValueId>,
    ) -> Result<(TypedValue, TypedValue), Diagnostic> {
        let a = self.expr(lhs, locals)?;
        let b = self.expr(rhs, locals)?;
        Ok((a, b))
    }

    fn expr(
        &mut self,
        expr: &Expr,
        locals: &HashMap<String, ValueId>,
    ) -> Result<(ValueId, Type), Diagnostic> {
        let (operation, ty) = match &expr.kind {
            ExprKind::Int(value) => (Operation::Const(*value), Type::I64),
            ExprKind::Bool(value) => (Operation::Bool(*value), Type::Bool),
            ExprKind::Name(name) => {
                let id = locals
                    .get(name)
                    .copied()
                    .ok_or_else(|| Diagnostic::new(expr.span, format!("unknown local {name}")))?;
                return Ok((id, self.value_types[id.0]));
            }
            ExprKind::Neg(rhs) => {
                let (value, ty) = self.expr(rhs, locals)?;
                self.require(ty, Type::I64, rhs.span)?;
                (Operation::Neg(value), Type::I64)
            }
            ExprKind::Add(lhs, rhs) => {
                let ((a, ta), (b, tb)) = self.pair(lhs, rhs, locals)?;
                self.require(ta, Type::I64, lhs.span)?;
                self.require(tb, Type::I64, rhs.span)?;
                (Operation::Add(a, b), Type::I64)
            }
            ExprKind::Sub(lhs, rhs) => {
                let ((a, ta), (b, tb)) = self.pair(lhs, rhs, locals)?;
                self.require(ta, Type::I64, lhs.span)?;
                self.require(tb, Type::I64, rhs.span)?;
                (Operation::Sub(a, b), Type::I64)
            }
            ExprKind::Mul(lhs, rhs) => {
                let ((a, ta), (b, tb)) = self.pair(lhs, rhs, locals)?;
                self.require(ta, Type::I64, lhs.span)?;
                self.require(tb, Type::I64, rhs.span)?;
                (Operation::Mul(a, b), Type::I64)
            }
            ExprKind::Eq(lhs, rhs) => {
                let ((a, ta), (b, tb)) = self.pair(lhs, rhs, locals)?;
                if ta != tb {
                    return Err(Diagnostic::new(
                        expr.span,
                        "equality operands must have the same type",
                    ));
                }
                (Operation::Eq(a, b), Type::Bool)
            }
            ExprKind::Lt(lhs, rhs) => {
                let ((a, ta), (b, tb)) = self.pair(lhs, rhs, locals)?;
                self.require(ta, Type::I64, lhs.span)?;
                self.require(tb, Type::I64, rhs.span)?;
                (Operation::Lt(a, b), Type::Bool)
            }
            ExprKind::Call(name, args) => {
                let (target, expected) = self.functions.get(name).copied().ok_or_else(|| {
                    Diagnostic::new(expr.span, format!("unknown function {name}"))
                })?;
                if args.len() != expected {
                    return Err(Diagnostic::new(
                        expr.span,
                        format!("{name} expects {expected} arguments, got {}", args.len()),
                    ));
                }
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    let (value, ty) = self.expr(arg, locals)?;
                    self.require(ty, Type::I64, arg.span)?;
                    values.push(value);
                }
                (Operation::Call(target, values), Type::I64)
            }
            ExprKind::If(condition, then_body, else_body) => {
                let (cond, ty) = self.expr(condition, locals)?;
                self.require(ty, Type::Bool, condition.span)?;
                let then_block = self.block();
                let else_block = self.block();
                self.terminate(Terminator::Branch {
                    condition: cond,
                    then_block,
                    else_block,
                });
                self.current = then_block;
                let (then_value, then_type) = self.body(then_body, &mut locals.clone())?;
                let then_end = self.current;
                self.current = else_block;
                let (else_value, else_type) = self.body(else_body, &mut locals.clone())?;
                let else_end = self.current;
                if then_type != else_type {
                    return Err(Diagnostic::new(
                        expr.span,
                        "if branches must have the same type",
                    ));
                }
                let join = self.block();
                let result = self.block_param(join, then_type);
                self.current = then_end;
                self.terminate(Terminator::Jump(join, vec![then_value]));
                self.current = else_end;
                self.terminate(Terminator::Jump(join, vec![else_value]));
                self.current = join;
                return Ok((result, then_type));
            }
        };
        let value = self.emit(operation, ty, expr.span);
        Ok((value, ty))
    }

    fn finish(self, name: String) -> Result<Function, Diagnostic> {
        let mut blocks = Vec::with_capacity(self.blocks.len());
        for block in self.blocks {
            let Some(terminator) = block.terminator else {
                return Err(Diagnostic::new(
                    Span { start: 0, end: 0 },
                    "internal lowering left a block without a terminator",
                ));
            };
            blocks.push(Block {
                params: block.params,
                instructions: block.instructions,
                terminator,
            });
        }
        Ok(Function {
            name,
            blocks,
            value_types: self.value_types,
        })
    }
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
                format!("duplicate function {}", function.name),
            ));
        }
    }
    let mut lowered = Vec::with_capacity(ast.functions.len());
    for function in &ast.functions {
        let mut lowerer = Lowerer::new(&functions, function.params.len());
        let mut locals = HashMap::new();
        for (index, (name, span)) in function.params.iter().enumerate() {
            if locals.insert(name.clone(), ValueId(index)).is_some() {
                return Err(Diagnostic::new(
                    *span,
                    format!("duplicate parameter {name}"),
                ));
            }
        }
        let (value, ty) = lowerer.body(&function.body, &mut locals)?;
        lowerer.require(ty, Type::I64, function.body.tail.span)?;
        lowerer.terminate(Terminator::Return(value));
        lowered.push(lowerer.finish(function.name.clone())?);
    }
    let program = Program { functions: lowered };
    verify(&program).map_err(|message| Diagnostic::new(Span { start: 0, end: 0 }, message))?;
    Ok(program)
}

fn op_type(program: &Program, function: &Function, operation: &Operation) -> Result<Type, String> {
    let ty = |id: ValueId| function.value_types[id.0];
    match operation {
        Operation::Const(_) => Ok(Type::I64),
        Operation::Bool(_) => Ok(Type::Bool),
        Operation::Neg(a) => {
            if ty(*a) != Type::I64 {
                return Err("neg expects I64".into());
            }
            Ok(Type::I64)
        }
        Operation::Add(a, b) | Operation::Sub(a, b) | Operation::Mul(a, b) => {
            if ty(*a) != Type::I64 || ty(*b) != Type::I64 {
                return Err("arithmetic expects I64".into());
            }
            Ok(Type::I64)
        }
        Operation::Eq(a, b) => {
            if ty(*a) != ty(*b) {
                return Err("equality type mismatch".into());
            }
            Ok(Type::Bool)
        }
        Operation::Lt(a, b) => {
            if ty(*a) != Type::I64 || ty(*b) != Type::I64 {
                return Err("less-than expects I64".into());
            }
            Ok(Type::Bool)
        }
        Operation::Call(target, args) => {
            let callee = program
                .functions
                .get(target.0)
                .ok_or_else(|| "invalid call target".to_owned())?;
            if args.len() != callee.blocks[0].params.len()
                || args.iter().any(|arg| ty(*arg) != Type::I64)
            {
                return Err("call signature mismatch".into());
            }
            Ok(Type::I64)
        }
    }
}

fn operands(operation: &Operation) -> Vec<ValueId> {
    match operation {
        Operation::Const(_) | Operation::Bool(_) => vec![],
        Operation::Neg(a) => vec![*a],
        Operation::Add(a, b)
        | Operation::Sub(a, b)
        | Operation::Mul(a, b)
        | Operation::Eq(a, b)
        | Operation::Lt(a, b) => vec![*a, *b],
        Operation::Call(_, args) => args.clone(),
    }
}

pub fn verify(program: &Program) -> Result<(), String> {
    if program.functions.is_empty() {
        return Err("empty Core IR program".into());
    }
    for function in &program.functions {
        if function.blocks.is_empty() {
            return Err(format!("{}: no entry block", function.name));
        }
        if function.blocks[0].params.len() > 4 {
            return Err(format!(
                "{}: at most four parameters are supported",
                function.name
            ));
        }
        let count = function.value_types.len();
        let mut definitions = vec![None; count];
        for (block_id, block) in function.blocks.iter().enumerate() {
            for (param_index, param) in block.params.iter().enumerate() {
                let slot = definitions
                    .get_mut(param.0)
                    .ok_or_else(|| format!("{}: invalid value ID", function.name))?;
                if slot.replace((block_id, None)).is_some() {
                    return Err(format!("{}: duplicate value definition", function.name));
                }
                if block_id == 0
                    && (param_index != param.0 || function.value_types[param.0] != Type::I64)
                {
                    return Err(format!("{}: invalid function parameter", function.name));
                }
            }
            for (index, instruction) in block.instructions.iter().enumerate() {
                let slot = definitions
                    .get_mut(instruction.result.0)
                    .ok_or_else(|| format!("{}: invalid value ID", function.name))?;
                if slot.replace((block_id, Some(index))).is_some() {
                    return Err(format!("{}: duplicate value definition", function.name));
                }
            }
        }
        if definitions.iter().any(Option::is_none) {
            return Err(format!("{}: undefined value ID", function.name));
        }
        let mut successors = vec![Vec::new(); function.blocks.len()];
        let mut predecessors = vec![Vec::new(); function.blocks.len()];
        for (block_id, block) in function.blocks.iter().enumerate() {
            let targets = match &block.terminator {
                Terminator::Return(_) => vec![],
                Terminator::Jump(target, _) => vec![*target],
                Terminator::Branch {
                    then_block,
                    else_block,
                    ..
                } => vec![*then_block, *else_block],
            };
            for target in targets {
                if target.0 >= function.blocks.len() {
                    return Err(format!("{}: invalid branch target", function.name));
                }
                successors[block_id].push(target.0);
                predecessors[target.0].push(block_id);
            }
        }
        if !predecessors[0].is_empty() {
            return Err(format!("{}: entry block has a predecessor", function.name));
        }
        let mut reachable = vec![false; function.blocks.len()];
        let mut ready = VecDeque::from([0]);
        reachable[0] = true;
        while let Some(id) = ready.pop_front() {
            for target in &successors[id] {
                if !reachable[*target] {
                    reachable[*target] = true;
                    ready.push_back(*target);
                }
            }
        }
        if reachable.iter().any(|yes| !yes) {
            return Err(format!("{}: unreachable block", function.name));
        }
        let all = (0..function.blocks.len()).collect::<std::collections::HashSet<_>>();
        let mut dominators = vec![all.clone(); function.blocks.len()];
        dominators[0] = std::collections::HashSet::from([0]);
        let mut changed = true;
        while changed {
            changed = false;
            for id in 1..function.blocks.len() {
                let mut next = all.clone();
                for predecessor in &predecessors[id] {
                    next.retain(|candidate| dominators[*predecessor].contains(candidate));
                }
                next.insert(id);
                if next != dominators[id] {
                    dominators[id] = next;
                    changed = true;
                }
            }
        }
        let check_use =
            |value: ValueId, block_id: usize, instruction_index: usize| -> Result<(), String> {
                let (def_block, def_index) = definitions
                    .get(value.0)
                    .and_then(|item| *item)
                    .ok_or_else(|| format!("{}: undefined operand", function.name))?;
                if def_block == block_id {
                    if def_index.is_some_and(|index| index >= instruction_index) {
                        return Err(format!("{}: use before definition", function.name));
                    }
                } else if !dominators[block_id].contains(&def_block) {
                    return Err(format!("{}: non-dominating value use", function.name));
                }
                Ok(())
            };
        for (block_id, block) in function.blocks.iter().enumerate() {
            for (index, instruction) in block.instructions.iter().enumerate() {
                for operand in operands(&instruction.operation) {
                    check_use(operand, block_id, index)?;
                }
                let expected = op_type(program, function, &instruction.operation)?;
                if function.value_types[instruction.result.0] != expected {
                    return Err(format!(
                        "{}: instruction result type mismatch",
                        function.name
                    ));
                }
            }
            let end = block.instructions.len();
            match &block.terminator {
                Terminator::Return(value) => {
                    check_use(*value, block_id, end)?;
                    if function.value_types[value.0] != Type::I64 {
                        return Err(format!("{}: return expects I64", function.name));
                    }
                }
                Terminator::Jump(target, args) => {
                    let params = &function.blocks[target.0].params;
                    if args.len() != params.len() {
                        return Err(format!("{}: block argument count mismatch", function.name));
                    }
                    for (arg, param) in args.iter().zip(params) {
                        check_use(*arg, block_id, end)?;
                        if function.value_types[arg.0] != function.value_types[param.0] {
                            return Err(format!("{}: block argument type mismatch", function.name));
                        }
                    }
                }
                Terminator::Branch {
                    condition,
                    then_block,
                    else_block,
                } => {
                    check_use(*condition, block_id, end)?;
                    if function.value_types[condition.0] != Type::Bool {
                        return Err(format!("{}: branch requires Bool", function.name));
                    }
                    if !function.blocks[then_block.0].params.is_empty()
                        || !function.blocks[else_block.0].params.is_empty()
                    {
                        return Err(format!(
                            "{}: conditional target cannot have parameters",
                            function.name
                        ));
                    }
                }
            }
        }
    }
    // Guarded task stacks are not implemented. Reject recursive calls for now.
    let mut incoming = vec![0usize; program.functions.len()];
    let mut edges = vec![Vec::new(); program.functions.len()];
    for (caller, function) in program.functions.iter().enumerate() {
        for block in &function.blocks {
            for instruction in &block.instructions {
                if let Operation::Call(target, _) = &instruction.operation {
                    edges[caller].push(target.0);
                    incoming[target.0] += 1;
                }
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
            .ok_or_else(|| format!("unknown function {name}"))?;
        self.interpret_id(id, args, 0)
    }

    fn interpret_id(&self, id: FunctionId, args: &[i64], depth: usize) -> Result<i64, String> {
        if depth >= 1024 {
            return Err("interpreter call depth exceeded 1024".into());
        }
        let function = &self.functions[id.0];
        if args.len() != function.blocks[0].params.len() {
            return Err(format!(
                "{} expects {} arguments, got {}",
                function.name,
                function.blocks[0].params.len(),
                args.len()
            ));
        }
        let mut values = vec![None; function.value_types.len()];
        for (param, arg) in function.blocks[0].params.iter().zip(args) {
            values[param.0] = Some(Value::I64(*arg));
        }
        let mut current = 0usize;
        loop {
            let block = &function.blocks[current];
            for instruction in &block.instructions {
                let get = |id: ValueId| values[id.0].expect("verified value is defined");
                let value = match &instruction.operation {
                    Operation::Const(x) => Value::I64(*x),
                    Operation::Bool(x) => Value::Bool(*x),
                    Operation::Neg(a) => Value::I64(as_i64(get(*a)).wrapping_neg()),
                    Operation::Add(a, b) => {
                        Value::I64(as_i64(get(*a)).wrapping_add(as_i64(get(*b))))
                    }
                    Operation::Sub(a, b) => {
                        Value::I64(as_i64(get(*a)).wrapping_sub(as_i64(get(*b))))
                    }
                    Operation::Mul(a, b) => {
                        Value::I64(as_i64(get(*a)).wrapping_mul(as_i64(get(*b))))
                    }
                    Operation::Eq(a, b) => Value::Bool(get(*a) == get(*b)),
                    Operation::Lt(a, b) => Value::Bool(as_i64(get(*a)) < as_i64(get(*b))),
                    Operation::Call(target, args) => {
                        let args = args.iter().map(|arg| as_i64(get(*arg))).collect::<Vec<_>>();
                        Value::I64(self.interpret_id(*target, &args, depth + 1)?)
                    }
                };
                values[instruction.result.0] = Some(value);
            }
            let get = |id: ValueId| values[id.0].expect("verified value is defined");
            match &block.terminator {
                Terminator::Return(value) => return Ok(as_i64(get(*value))),
                Terminator::Jump(target, args) => {
                    let incoming = args.iter().map(|arg| get(*arg)).collect::<Vec<_>>();
                    for (param, value) in function.blocks[target.0].params.iter().zip(incoming) {
                        values[param.0] = Some(value);
                    }
                    current = target.0;
                }
                Terminator::Branch {
                    condition,
                    then_block,
                    else_block,
                } => {
                    current = if as_bool(get(*condition)) {
                        then_block.0
                    } else {
                        else_block.0
                    };
                }
            }
        }
    }
}

fn as_i64(value: Value) -> i64 {
    match value {
        Value::I64(x) => x,
        Value::Bool(_) => unreachable!("verified I64 operand"),
    }
}

fn as_bool(value: Value) -> bool {
    match value {
        Value::Bool(x) => x,
        Value::I64(_) => unreachable!("verified Bool operand"),
    }
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for function in &self.functions {
            write!(f, "fn {}(", function.name)?;
            for (index, param) in function.blocks[0].params.iter().enumerate() {
                if index > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "v{}: I64", param.0)?;
            }
            writeln!(f, ") -> I64 {{")?;
            for (block_id, block) in function.blocks.iter().enumerate() {
                write!(f, "  block{block_id}(")?;
                for (index, param) in block.params.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "v{}: {:?}", param.0, function.value_types[param.0])?;
                }
                writeln!(f, "):")?;
                for instruction in &block.instructions {
                    write!(
                        f,
                        "    v{}: {:?} = ",
                        instruction.result.0, function.value_types[instruction.result.0]
                    )?;
                    match &instruction.operation {
                        Operation::Const(x) => write!(f, "const {x}")?,
                        Operation::Bool(x) => write!(f, "bool {x}")?,
                        Operation::Neg(a) => write!(f, "neg v{}", a.0)?,
                        Operation::Add(a, b) => write!(f, "add v{}, v{}", a.0, b.0)?,
                        Operation::Sub(a, b) => write!(f, "sub v{}, v{}", a.0, b.0)?,
                        Operation::Mul(a, b) => write!(f, "mul v{}, v{}", a.0, b.0)?,
                        Operation::Eq(a, b) => write!(f, "eq v{}, v{}", a.0, b.0)?,
                        Operation::Lt(a, b) => write!(f, "lt v{}, v{}", a.0, b.0)?,
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
                match &block.terminator {
                    Terminator::Return(value) => writeln!(f, "    return v{}", value.0)?,
                    Terminator::Jump(target, args) => {
                        write!(f, "    jump block{}(", target.0)?;
                        for (index, arg) in args.iter().enumerate() {
                            if index > 0 {
                                write!(f, ", ")?;
                            }
                            write!(f, "v{}", arg.0)?;
                        }
                        writeln!(f, ")")?;
                    }
                    Terminator::Branch {
                        condition,
                        then_block,
                        else_block,
                    } => {
                        writeln!(
                            f,
                            "    branch v{}, block{}, block{}",
                            condition.0, then_block.0, else_block.0
                        )?;
                    }
                }
            }
            writeln!(f, "}}")?;
        }
        Ok(())
    }
}
