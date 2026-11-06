use std::collections::{HashMap, VecDeque};
use std::fmt;

use crate::syntax::{self, Body, CallArgument, Expr, ExprKind, ParamMode, Statement, TypeName};
use crate::{Diagnostic, Span};

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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    I64(i64),
    Bool(bool),
    Data(DataId, usize, Option<Box<Value>>),
    Product(DataId, Vec<Value>),
    Ref(usize),
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
    Store(PlaceId, ValueId),
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

#[derive(Clone, Debug)]
pub struct DataDef {
    pub name: String,
    pub variants: Vec<String>,
    pub payloads: Vec<Option<Type>>,
    pub fields: Vec<(String, Type)>,
}

struct DraftBlock {
    params: Vec<ValueId>,
    instructions: Vec<Instruction>,
    terminator: Option<Terminator>,
}

#[derive(Clone)]
struct FunctionSignature {
    id: FunctionId,
    params: Vec<(ParamMode, Type)>,
    return_type: Type,
}

struct Lowerer<'a> {
    functions: &'a HashMap<String, FunctionSignature>,
    data: &'a [DataDef],
    data_names: &'a HashMap<String, DataId>,
    blocks: Vec<DraftBlock>,
    value_types: Vec<Type>,
    place_types: Vec<Type>,
    param_modes: Vec<ParamMode>,
    return_type: Type,
    current: BlockId,
}

impl<'a> Lowerer<'a> {
    fn new(
        functions: &'a HashMap<String, FunctionSignature>,
        data: &'a [DataDef],
        data_names: &'a HashMap<String, DataId>,
        params: Vec<(ParamMode, Type)>,
        return_type: Type,
    ) -> Self {
        let mut lowerer = Self {
            functions,
            data,
            data_names,
            blocks: vec![DraftBlock {
                params: Vec::new(),
                instructions: Vec::new(),
                terminator: None,
            }],
            value_types: Vec::new(),
            place_types: Vec::new(),
            param_modes: params.iter().map(|(mode, _)| *mode).collect(),
            return_type,
            current: BlockId(0),
        };
        for (mode, own_type) in params {
            let ty = match mode {
                ParamMode::Own => own_type,
                ParamMode::Shared => Type::RefI64,
                ParamMode::Exclusive => Type::RefMutI64,
            };
            let id = lowerer.value(ty);
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
            result: Some(result),
            operation,
            span,
        });
        result
    }

    fn emit_effect(&mut self, operation: Operation, span: Span) {
        self.blocks[self.current.0].instructions.push(Instruction {
            result: None,
            operation,
            span,
        });
    }

    fn place(&mut self, ty: Type) -> PlaceId {
        let id = PlaceId(self.place_types.len());
        self.place_types.push(ty);
        id
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
        locals: &mut HashMap<String, Binding>,
    ) -> Result<Option<TypedValue>, Diagnostic> {
        for statement in &body.statements {
            match statement {
                Statement::Val(name, span, expr) | Statement::Var(name, span, expr) => {
                    if locals.contains_key(name) {
                        return Err(Diagnostic::new(*span, format!("duplicate local {name}")));
                    }
                    let (value, ty) = self.expr(expr, locals)?;
                    if matches!(statement, Statement::Var(..)) {
                        let place = self.place(ty);
                        self.emit_effect(Operation::PlaceInit(place, value), *span);
                        locals.insert(name.clone(), Binding::Var(place));
                    } else {
                        locals.insert(name.clone(), Binding::Val(value));
                    }
                }
                Statement::Assign(name, span, expr) => {
                    let binding = locals
                        .get(name)
                        .copied()
                        .ok_or_else(|| Diagnostic::new(*span, format!("unknown local {name}")))?;
                    let (value, ty) = self.expr(expr, locals)?;
                    match binding {
                        Binding::Var(place) => {
                            self.require(ty, self.place_types[place.0], expr.span)?;
                            self.emit_effect(Operation::Store(place, value), *span);
                        }
                        Binding::Ref(reference, ParamMode::Exclusive) => {
                            self.require(ty, Type::I64, expr.span)?;
                            self.emit_effect(Operation::StoreRef(reference, value), *span);
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                *span,
                                format!("cannot assign to val {name}"),
                            ));
                        }
                    }
                }
                Statement::While(condition, loop_body) => {
                    if let Some(tail) = &loop_body.tail {
                        return Err(Diagnostic::new(
                            tail.span,
                            "while body cannot end in a value expression; add a semicolon",
                        ));
                    }
                    let head = self.block();
                    let body_block = self.block();
                    let exit = self.block();
                    self.terminate(Terminator::Jump(head, vec![]));
                    self.current = head;
                    let (cond, ty) = self.expr(condition, locals)?;
                    self.require(ty, Type::Bool, condition.span)?;
                    self.terminate(Terminator::Branch {
                        condition: cond,
                        then_block: body_block,
                        else_block: exit,
                    });
                    self.current = body_block;
                    self.body(loop_body, &mut locals.clone())?;
                    self.terminate(Terminator::Jump(head, vec![]));
                    self.current = exit;
                }
                Statement::Expr(expr) => {
                    self.expr(expr, locals)?;
                }
            }
        }
        body.tail
            .as_ref()
            .map(|expr| self.expr(expr, locals))
            .transpose()
    }

    fn pair(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
        locals: &HashMap<String, Binding>,
    ) -> Result<(TypedValue, TypedValue), Diagnostic> {
        let a = self.expr(lhs, locals)?;
        let b = self.expr(rhs, locals)?;
        Ok((a, b))
    }

    fn expr(
        &mut self,
        expr: &Expr,
        locals: &HashMap<String, Binding>,
    ) -> Result<(ValueId, Type), Diagnostic> {
        let (operation, ty) = match &expr.kind {
            ExprKind::Int(value) => (Operation::Const(*value), Type::I64),
            ExprKind::Bool(value) => (Operation::Bool(*value), Type::Bool),
            ExprKind::Member(name, variant) => {
                if let Some(binding) = locals.get(name).copied() {
                    let ty = match binding {
                        Binding::Val(value) => self.value_types[value.0],
                        Binding::Var(place) => self.place_types[place.0],
                        Binding::Ref(_, _) => Type::RefI64,
                    };
                    let Type::Data(id) = ty else {
                        return Err(Diagnostic::new(
                            expr.span,
                            "field access requires a product",
                        ));
                    };
                    let index = self.data[id.0]
                        .fields
                        .iter()
                        .position(|(field, _)| field == variant)
                        .ok_or_else(|| {
                            Diagnostic::new(expr.span, format!("unknown field {variant}"))
                        })?;
                    let field_type = self.data[id.0].fields[index].1;
                    let operation = match binding {
                        Binding::Val(value) => Operation::Field(value, index),
                        Binding::Var(place) => Operation::ReadField(place, index),
                        Binding::Ref(_, _) => unreachable!("data reference is unsupported"),
                    };
                    (operation, field_type)
                } else {
                    let id = self.data_names.get(name).copied().ok_or_else(|| {
                        Diagnostic::new(expr.span, format!("unknown data type {name}"))
                    })?;
                    let tag = self.data[id.0]
                        .variants
                        .iter()
                        .position(|candidate| candidate == variant)
                        .ok_or_else(|| {
                            Diagnostic::new(expr.span, format!("unknown variant {name}.{variant}"))
                        })?;
                    if self.data[id.0].payloads[tag].is_some() {
                        return Err(Diagnostic::new(expr.span, "variant requires a payload"));
                    }
                    (Operation::Variant(id, tag, None), Type::Data(id))
                }
            }
            ExprKind::VariantCall(name, variant, payload) => {
                let id = self.data_names.get(name).copied().ok_or_else(|| {
                    Diagnostic::new(expr.span, format!("unknown data type {name}"))
                })?;
                let tag = self.data[id.0]
                    .variants
                    .iter()
                    .position(|candidate| candidate == variant)
                    .ok_or_else(|| {
                        Diagnostic::new(expr.span, format!("unknown variant {name}.{variant}"))
                    })?;
                let expected = self.data[id.0].payloads[tag]
                    .ok_or_else(|| Diagnostic::new(expr.span, "variant has no payload"))?;
                let (value, actual) = self.expr(payload, locals)?;
                self.require(actual, expected, payload.span)?;
                (Operation::Variant(id, tag, Some(value)), Type::Data(id))
            }
            ExprKind::Try(inner) => {
                let (value, ty) = self.expr(inner, locals)?;
                let Type::Data(id) = ty else {
                    return Err(Diagnostic::new(inner.span, "? requires Result"));
                };
                let data = &self.data[id.0];
                if !data.name.starts_with("Result<") || data.variants.len() != 2 {
                    return Err(Diagnostic::new(inner.span, "? requires Result"));
                }
                let ok_tag = data
                    .variants
                    .iter()
                    .position(|name| name == "Ok")
                    .ok_or_else(|| {
                        Diagnostic::new(inner.span, "Result requires Ok and Err variants")
                    })?;
                let err_tag = data
                    .variants
                    .iter()
                    .position(|name| name == "Err")
                    .ok_or_else(|| {
                        Diagnostic::new(inner.span, "Result requires Ok and Err variants")
                    })?;
                let ok_ty = data.payloads[ok_tag]
                    .ok_or_else(|| Diagnostic::new(inner.span, "Result.Ok requires a payload"))?;
                if data.payloads[err_tag].is_none() {
                    return Err(Diagnostic::new(inner.span, "Result.Err requires a payload"));
                }
                if self.return_type != ty {
                    return Err(Diagnostic::new(
                        expr.span,
                        "? requires the function to return the same Result type",
                    ));
                }
                let success = self.block();
                let payload = self.block_param(success, ok_ty);
                let failure = self.block();
                let original = self.block_param(failure, ty);
                let mut arms = vec![
                    MatchArm {
                        target: failure,
                        binding: MatchBinding::Whole,
                    };
                    2
                ];
                arms[ok_tag] = MatchArm {
                    target: success,
                    binding: MatchBinding::Payload,
                };
                self.terminate(Terminator::Match {
                    subject: value,
                    arms,
                });
                self.current = failure;
                self.terminate(Terminator::Return(original));
                self.current = success;
                return Ok((payload, ok_ty));
            }
            ExprKind::Name(name) => {
                let binding = locals
                    .get(name)
                    .copied()
                    .ok_or_else(|| Diagnostic::new(expr.span, format!("unknown local {name}")))?;
                return Ok(match binding {
                    Binding::Val(id) => {
                        let ty = self.value_types[id.0];
                        if matches!(ty, Type::Data(_)) {
                            (self.emit(Operation::Move(id), ty, expr.span), ty)
                        } else {
                            (id, ty)
                        }
                    }
                    Binding::Var(place) => {
                        let ty = self.place_types[place.0];
                        let operation = if matches!(ty, Type::Data(_)) {
                            Operation::Take(place)
                        } else {
                            Operation::Load(place)
                        };
                        (self.emit(operation, ty, expr.span), ty)
                    }
                    Binding::Ref(reference, _) => (
                        self.emit(Operation::LoadRef(reference), Type::I64, expr.span),
                        Type::I64,
                    ),
                });
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
                if let Type::Data(id) = ta
                    && (!self.data[id.0].fields.is_empty()
                        || self.data[id.0].payloads.iter().any(Option::is_some))
                {
                    return Err(Diagnostic::new(
                        expr.span,
                        "product equality and payload sum equality are not implemented",
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
                if let Some(id) = self.data_names.get(name).copied() {
                    let fields = &self.data[id.0].fields;
                    if fields.is_empty() {
                        return Err(Diagnostic::new(
                            expr.span,
                            "sum type is not a product constructor",
                        ));
                    }
                    if args.len() != fields.len() {
                        return Err(Diagnostic::new(
                            expr.span,
                            format!("{name} expects {} fields, got {}", fields.len(), args.len()),
                        ));
                    }
                    let expected = fields.iter().map(|(_, ty)| *ty).collect::<Vec<_>>();
                    let mut values = Vec::with_capacity(args.len());
                    for (arg, ty) in args.iter().zip(expected) {
                        let CallArgument::Value(value) = arg else {
                            return Err(Diagnostic::new(
                                expr.span,
                                "product constructor requires owned fields",
                            ));
                        };
                        let (id, actual) = self.expr(value, locals)?;
                        self.require(actual, ty, value.span)?;
                        values.push(id);
                    }
                    let result =
                        self.emit(Operation::Product(id, values), Type::Data(id), expr.span);
                    return Ok((result, Type::Data(id)));
                }
                let signature = self.functions.get(name).cloned().ok_or_else(|| {
                    Diagnostic::new(expr.span, format!("unknown function {name}"))
                })?;
                let expected = signature.params;
                if args.len() != expected.len() {
                    return Err(Diagnostic::new(
                        expr.span,
                        format!(
                            "{name} expects {} arguments, got {}",
                            expected.len(),
                            args.len()
                        ),
                    ));
                }
                let mut values = Vec::with_capacity(args.len());
                for (arg, (mode, param_type)) in args.iter().zip(expected) {
                    match (arg, mode) {
                        (CallArgument::Value(arg), ParamMode::Own) => {
                            let (value, ty) = self.expr(arg, locals)?;
                            self.require(ty, param_type, arg.span)?;
                            values.push(CallArg::Value(value));
                        }
                        (
                            CallArgument::Borrow {
                                name,
                                span,
                                mutable,
                            },
                            ParamMode::Shared | ParamMode::Exclusive,
                        ) => {
                            if *mutable != (mode == ParamMode::Exclusive) {
                                return Err(Diagnostic::new(
                                    *span,
                                    "borrow mutability does not match parameter",
                                ));
                            }
                            let binding = locals.get(name).copied().ok_or_else(|| {
                                Diagnostic::new(*span, format!("unknown local {name}"))
                            })?;
                            let call_arg = match binding {
                                Binding::Var(place) => {
                                    self.require(self.place_types[place.0], Type::I64, *span)?;
                                    CallArg::Borrow(place, *mutable)
                                }
                                Binding::Val(value) if !*mutable => {
                                    self.require(self.value_types[value.0], Type::I64, *span)?;
                                    let place = self.place(Type::I64);
                                    self.emit_effect(Operation::PlaceInit(place, value), *span);
                                    CallArg::Borrow(place, false)
                                }
                                Binding::Ref(value, source_mode)
                                    if !*mutable || source_mode == ParamMode::Exclusive =>
                                {
                                    CallArg::Ref(value, *mutable)
                                }
                                _ => {
                                    return Err(Diagnostic::new(
                                        *span,
                                        "cannot mutably borrow this binding",
                                    ));
                                }
                            };
                            values.push(call_arg);
                        }
                        (CallArgument::Value(arg), _) => {
                            return Err(Diagnostic::new(arg.span, "expected explicit borrow"));
                        }
                        (CallArgument::Borrow { span, .. }, _) => {
                            return Err(Diagnostic::new(*span, "expected owned argument"));
                        }
                    }
                }
                check_call_aliases(&values)
                    .map_err(|message| Diagnostic::new(expr.span, message))?;
                (Operation::Call(signature.id, values), signature.return_type)
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
                let (then_value, then_type) = self
                    .body(then_body, &mut locals.clone())?
                    .expect("parser requires an if-arm tail");
                let then_end = self.current;
                self.current = else_block;
                let (else_value, else_type) = self
                    .body(else_body, &mut locals.clone())?
                    .expect("parser requires an if-arm tail");
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
            ExprKind::When(subject, arms) => {
                let (subject_value, subject_type) = self.expr(subject, locals)?;
                let Type::Data(data_id) = subject_type else {
                    return Err(Diagnostic::new(subject.span, "when requires a data value"));
                };
                let data = &self.data[data_id.0];
                if data.variants.is_empty() {
                    return Err(Diagnostic::new(subject.span, "when requires a sum type"));
                }
                let mut ordered = vec![None; data.variants.len()];
                for arm in arms {
                    if arm.type_name != data.name {
                        return Err(Diagnostic::new(
                            arm.span,
                            "when arm has the wrong data type",
                        ));
                    }
                    let tag = data
                        .variants
                        .iter()
                        .position(|name| name == &arm.variant)
                        .ok_or_else(|| Diagnostic::new(arm.span, "unknown when variant"))?;
                    if ordered[tag].is_some() {
                        return Err(Diagnostic::new(arm.span, "duplicate when variant"));
                    }
                    if arm.binding.is_some() != data.payloads[tag].is_some() {
                        return Err(Diagnostic::new(
                            arm.span,
                            "when payload binding does not match variant",
                        ));
                    }
                    ordered[tag] = Some(arm);
                }
                if let Some(tag) = ordered.iter().position(Option::is_none) {
                    return Err(Diagnostic::new(
                        expr.span,
                        format!(
                            "non-exhaustive when: missing {}.{}",
                            data.name, data.variants[tag]
                        ),
                    ));
                }
                let payload_types = data.payloads.clone();
                let targets = (0..ordered.len()).map(|_| self.block()).collect::<Vec<_>>();
                let payload_values = targets
                    .iter()
                    .zip(&payload_types)
                    .map(|(target, payload)| payload.map(|ty| self.block_param(*target, ty)))
                    .collect::<Vec<_>>();
                self.terminate(Terminator::Match {
                    subject: subject_value,
                    arms: targets
                        .iter()
                        .zip(&payload_types)
                        .map(|(target, payload)| MatchArm {
                            target: *target,
                            binding: if payload.is_some() {
                                MatchBinding::Payload
                            } else {
                                MatchBinding::Discard
                            },
                        })
                        .collect(),
                });
                let mut arm_results = Vec::with_capacity(ordered.len());
                for (tag, arm) in ordered.into_iter().enumerate() {
                    let arm = arm.expect("coverage checked");
                    self.current = targets[tag];
                    let mut arm_locals = locals.clone();
                    if let (Some((name, span)), Some(value)) = (&arm.binding, payload_values[tag])
                        && arm_locals
                            .insert(name.clone(), Binding::Val(value))
                            .is_some()
                    {
                        return Err(Diagnostic::new(*span, "duplicate pattern binding"));
                    }
                    let (value, ty) = self
                        .body(&arm.body, &mut arm_locals)?
                        .expect("parser requires when-arm tail");
                    arm_results.push((self.current, value, ty));
                }
                let result_type = arm_results[0].2;
                if arm_results.iter().any(|(_, _, ty)| *ty != result_type) {
                    return Err(Diagnostic::new(
                        expr.span,
                        "when arms must have the same type",
                    ));
                }
                let join = self.block();
                let result = self.block_param(join, result_type);
                for (end, value, _) in arm_results {
                    self.current = end;
                    self.terminate(Terminator::Jump(join, vec![value]));
                }
                self.current = join;
                return Ok((result, result_type));
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
            place_types: self.place_types,
            param_modes: self.param_modes,
            return_type: self.return_type,
        })
    }
}

pub fn lower(ast: &syntax::Program) -> Result<Program, Diagnostic> {
    let mut data = Vec::with_capacity(ast.data.len());
    let mut data_names = HashMap::new();
    for declaration in &ast.data {
        if matches!(declaration.name.as_str(), "I64" | "Bool" | "Result")
            || data_names.contains_key(&declaration.name)
        {
            return Err(Diagnostic::new(declaration.span, "duplicate data type"));
        }
        let mut variants = Vec::with_capacity(declaration.variants.len());
        let mut payloads = Vec::with_capacity(declaration.variants.len());
        for variant in &declaration.variants {
            if variants.contains(&variant.name) {
                return Err(Diagnostic::new(variant.span, "duplicate variant"));
            }
            let payload = match variant.payload.as_ref().map(|field| &field.ty) {
                None => None,
                Some(TypeName::I64) => Some(Type::I64),
                Some(TypeName::Bool) => Some(Type::Bool),
                Some(TypeName::Named(_) | TypeName::Result(_, _)) => {
                    return Err(Diagnostic::new(
                        variant.span,
                        "nested variant payloads are not implemented",
                    ));
                }
            };
            variants.push(variant.name.clone());
            payloads.push(payload);
        }
        let mut fields = Vec::with_capacity(declaration.fields.len());
        for field in &declaration.fields {
            if fields.iter().any(|(name, _)| name == &field.name) {
                return Err(Diagnostic::new(field.span, "duplicate field"));
            }
            let ty = match field.ty {
                TypeName::I64 => Type::I64,
                TypeName::Bool => Type::Bool,
                TypeName::Named(_) | TypeName::Result(_, _) => {
                    return Err(Diagnostic::new(
                        field.span,
                        "nested product fields are not implemented",
                    ));
                }
            };
            fields.push((field.name.clone(), ty));
        }
        data_names.insert(declaration.name.clone(), DataId(data.len()));
        data.push(DataDef {
            name: declaration.name.clone(),
            variants,
            payloads,
            fields,
        });
    }
    for (application, span) in &ast.applied_results {
        let TypeName::Result(ok, err) = application else {
            unreachable!("parser only records Result applications")
        };
        let name = application.key();
        if data_names.contains_key(&name) {
            continue;
        }
        let concrete = |ty: &TypeName| match ty {
            TypeName::I64 => Ok(Type::I64),
            TypeName::Bool => Ok(Type::Bool),
            TypeName::Named(name) => data_names
                .get(name)
                .copied()
                .map(Type::Data)
                .ok_or_else(|| Diagnostic::new(*span, format!("unknown data type {name}"))),
            TypeName::Result(_, _) => data_names
                .get(&ty.key())
                .copied()
                .map(Type::Data)
                .ok_or_else(|| Diagnostic::new(*span, "unknown Result application")),
        };
        let payloads = vec![Some(concrete(ok)?), Some(concrete(err)?)];
        data_names.insert(name.clone(), DataId(data.len()));
        data.push(DataDef {
            name,
            variants: vec!["Ok".into(), "Err".into()],
            payloads,
            fields: vec![],
        });
    }
    let resolve = |name: &TypeName, span: Span| -> Result<Type, Diagnostic> {
        match name {
            TypeName::I64 => Ok(Type::I64),
            TypeName::Bool => Ok(Type::Bool),
            TypeName::Named(name) => data_names
                .get(name)
                .copied()
                .map(Type::Data)
                .ok_or_else(|| Diagnostic::new(span, format!("unknown data type {name}"))),
            TypeName::Result(_, _) => data_names
                .get(&name.key())
                .copied()
                .map(Type::Data)
                .ok_or_else(|| Diagnostic::new(span, "unknown Result application")),
        }
    };
    let mut functions = HashMap::new();
    for (index, function) in ast.functions.iter().enumerate() {
        if data_names.contains_key(&function.name) {
            return Err(Diagnostic::new(
                function.name_span,
                "function name conflicts with data type",
            ));
        }
        if function.params.len() > 4 {
            return Err(Diagnostic::new(
                function.name_span,
                "at most four parameters in this implementation slice",
            ));
        }
        if functions
            .insert(
                function.name.clone(),
                FunctionSignature {
                    id: FunctionId(index),
                    params: function
                        .params
                        .iter()
                        .map(|param| Ok((param.mode, resolve(&param.ty, param.span)?)))
                        .collect::<Result<Vec<_>, Diagnostic>>()?,
                    return_type: resolve(&function.return_type, function.name_span)?,
                },
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
        let mut lowerer = Lowerer::new(
            &functions,
            &data,
            &data_names,
            function
                .params
                .iter()
                .map(|param| Ok((param.mode, resolve(&param.ty, param.span)?)))
                .collect::<Result<Vec<_>, Diagnostic>>()?,
            resolve(&function.return_type, function.name_span)?,
        );
        let mut locals = HashMap::new();
        for (index, param) in function.params.iter().enumerate() {
            let binding = match param.mode {
                ParamMode::Own => Binding::Val(ValueId(index)),
                mode => Binding::Ref(ValueId(index), mode),
            };
            if locals.insert(param.name.clone(), binding).is_some() {
                return Err(Diagnostic::new(
                    param.span,
                    format!("duplicate parameter {}", param.name),
                ));
            }
        }
        let (value, ty) = lowerer
            .body(&function.body, &mut locals)?
            .expect("parser requires a function tail");
        lowerer.require(
            ty,
            resolve(&function.return_type, function.name_span)?,
            function.body.tail.as_ref().expect("tail").span,
        )?;
        lowerer.terminate(Terminator::Return(value));
        lowered.push(lowerer.finish(function.name.clone())?);
    }
    let program = Program {
        data,
        functions: lowered,
    };
    verify_structure(&program)
        .map_err(|message| Diagnostic::new(Span { start: 0, end: 0 }, message))?;
    verify_ownership(&program).map_err(|error| Diagnostic::new(error.span, error.message))?;
    Ok(program)
}

fn op_type(
    program: &Program,
    function: &Function,
    operation: &Operation,
) -> Result<Option<Type>, String> {
    let ty = |id: ValueId| function.value_types[id.0];
    match operation {
        Operation::Const(_) => Ok(Some(Type::I64)),
        Operation::Bool(_) => Ok(Some(Type::Bool)),
        Operation::Move(value) => {
            if !matches!(ty(*value), Type::Data(_)) {
                return Err("move requires a nominal value".into());
            }
            Ok(Some(ty(*value)))
        }
        Operation::Variant(data_id, tag, payload) => {
            let definition = program
                .data
                .get(data_id.0)
                .ok_or_else(|| "invalid data ID".to_owned())?;
            if !definition.fields.is_empty() || *tag >= definition.variants.len() {
                return Err("invalid variant tag".into());
            }
            if definition.payloads[*tag] != payload.map(ty) {
                return Err("variant payload type mismatch".into());
            }
            Ok(Some(Type::Data(*data_id)))
        }
        Operation::Product(data_id, fields) => {
            let definition = program.data.get(data_id.0).ok_or("invalid data ID")?;
            if definition.fields.len() != fields.len() || fields.is_empty() {
                return Err("product field count mismatch".into());
            }
            for (value, (_, expected)) in fields.iter().zip(&definition.fields) {
                if ty(*value) != *expected {
                    return Err("product field type mismatch".into());
                }
            }
            Ok(Some(Type::Data(*data_id)))
        }
        Operation::Field(value, index) => {
            let Type::Data(id) = ty(*value) else {
                return Err("field access requires a product".into());
            };
            program
                .data
                .get(id.0)
                .and_then(|definition| definition.fields.get(*index))
                .map(|(_, ty)| Some(*ty))
                .ok_or_else(|| "invalid field access".into())
        }
        Operation::ReadField(place, index) => {
            let Type::Data(id) = *function
                .place_types
                .get(place.0)
                .ok_or("invalid place ID")?
            else {
                return Err("place field access requires a product".into());
            };
            program.data[id.0]
                .fields
                .get(*index)
                .map(|(_, ty)| Some(*ty))
                .ok_or_else(|| "invalid place field access".into())
        }
        Operation::Neg(a) => {
            if ty(*a) != Type::I64 {
                return Err("neg expects I64".into());
            }
            Ok(Some(Type::I64))
        }
        Operation::Add(a, b) | Operation::Sub(a, b) | Operation::Mul(a, b) => {
            if ty(*a) != Type::I64 || ty(*b) != Type::I64 {
                return Err("arithmetic expects I64".into());
            }
            Ok(Some(Type::I64))
        }
        Operation::Eq(a, b) => {
            if ty(*a) != ty(*b) || !matches!(ty(*a), Type::I64 | Type::Bool | Type::Data(_)) {
                return Err("equality type mismatch".into());
            }
            if let Type::Data(id) = ty(*a)
                && (!program.data[id.0].fields.is_empty()
                    || program.data[id.0].payloads.iter().any(Option::is_some))
            {
                return Err("product equality and payload sum equality are not implemented".into());
            }
            Ok(Some(Type::Bool))
        }
        Operation::Lt(a, b) => {
            if ty(*a) != Type::I64 || ty(*b) != Type::I64 {
                return Err("less-than expects I64".into());
            }
            Ok(Some(Type::Bool))
        }
        Operation::Call(target, args) => {
            let callee = program
                .functions
                .get(target.0)
                .ok_or_else(|| "invalid call target".to_owned())?;
            let callee_params = callee
                .blocks
                .first()
                .ok_or_else(|| "callee has no entry block".to_owned())?
                .params
                .as_slice();
            if args.len() != callee.param_modes.len()
                || args.len() != callee_params.len()
                || check_call_aliases(args).is_err()
            {
                return Err("call signature mismatch".into());
            }
            for ((arg, mode), param) in args.iter().zip(&callee.param_modes).zip(callee_params) {
                let expected = *callee
                    .value_types
                    .get(param.0)
                    .ok_or_else(|| "invalid callee parameter".to_owned())?;
                let valid = match (arg, mode) {
                    (CallArg::Value(value), ParamMode::Own) => ty(*value) == expected,
                    (CallArg::Borrow(place, false), ParamMode::Shared)
                    | (CallArg::Borrow(place, true), ParamMode::Exclusive) => {
                        function.place_types.get(place.0) == Some(&Type::I64)
                    }
                    (CallArg::Ref(value, false), ParamMode::Shared) => {
                        matches!(ty(*value), Type::RefI64 | Type::RefMutI64)
                    }
                    (CallArg::Ref(value, true), ParamMode::Exclusive) => {
                        ty(*value) == Type::RefMutI64
                    }
                    _ => false,
                };
                if !valid {
                    return Err("call signature mismatch".into());
                }
            }
            Ok(Some(callee.return_type))
        }
        Operation::LoadRef(reference) => {
            if !matches!(ty(*reference), Type::RefI64 | Type::RefMutI64) {
                return Err("load requires a reference".into());
            }
            Ok(Some(Type::I64))
        }
        Operation::StoreRef(reference, value) => {
            if ty(*reference) != Type::RefMutI64 || ty(*value) != Type::I64 {
                return Err("store requires an exclusive I64 reference".into());
            }
            Ok(None)
        }
        Operation::PlaceInit(place, value) | Operation::Store(place, value) => {
            let place_ty = function
                .place_types
                .get(place.0)
                .ok_or_else(|| "invalid place ID".to_owned())?;
            if ty(*value) != *place_ty {
                return Err("place store type mismatch".into());
            }
            Ok(None)
        }
        Operation::Load(place) => match function.place_types.get(place.0) {
            Some(Type::I64) => Ok(Some(Type::I64)),
            Some(Type::Bool) => Ok(Some(Type::Bool)),
            Some(_) => Err("nominal place requires take or field read".into()),
            None => Err("invalid place ID".into()),
        },
        Operation::Take(place) => match function.place_types.get(place.0) {
            Some(ty @ Type::Data(_)) => Ok(Some(*ty)),
            Some(_) => Err("take requires a nominal place".into()),
            None => Err("invalid place ID".into()),
        },
    }
}

fn operands(operation: &Operation) -> Vec<ValueId> {
    match operation {
        Operation::Const(_) | Operation::Bool(_) => vec![],
        Operation::Move(value) => vec![*value],
        Operation::Variant(_, _, payload) => payload.iter().copied().collect(),
        Operation::Product(_, fields) => fields.clone(),
        Operation::Field(value, _) => vec![*value],
        Operation::Neg(a)
        | Operation::PlaceInit(_, a)
        | Operation::Store(_, a)
        | Operation::LoadRef(a) => vec![*a],
        Operation::StoreRef(a, b) => vec![*a, *b],
        Operation::Load(_) | Operation::Take(_) | Operation::ReadField(_, _) => vec![],
        Operation::Add(a, b)
        | Operation::Sub(a, b)
        | Operation::Mul(a, b)
        | Operation::Eq(a, b)
        | Operation::Lt(a, b) => vec![*a, *b],
        Operation::Call(_, args) => args
            .iter()
            .filter_map(|arg| match arg {
                CallArg::Value(value) | CallArg::Ref(value, _) => Some(*value),
                CallArg::Borrow(_, _) => None,
            })
            .collect(),
    }
}

#[derive(Clone, PartialEq, Eq)]
struct OwnershipState {
    values: Vec<bool>,
    places: Vec<bool>,
}

impl OwnershipState {
    fn intersect(&mut self, other: &Self) -> bool {
        let mut changed = false;
        for (value, incoming) in self.values.iter_mut().zip(&other.values) {
            let next = *value && *incoming;
            changed |= next != *value;
            *value = next;
        }
        for (place, incoming) in self.places.iter_mut().zip(&other.places) {
            let next = *place && *incoming;
            changed |= next != *place;
            *place = next;
        }
        changed
    }
}

struct OwnershipError {
    span: Span,
    message: String,
}

fn verify_ownership(program: &Program) -> Result<(), OwnershipError> {
    for function in &program.functions {
        let mut entry = OwnershipState {
            values: vec![false; function.value_types.len()],
            places: vec![false; function.place_types.len()],
        };
        for param in &function.blocks[0].params {
            if matches!(function.value_types[param.0], Type::Data(_)) {
                entry.values[param.0] = true;
            }
        }
        let mut incoming = vec![None; function.blocks.len()];
        incoming[0] = Some(entry);
        let mut ready = VecDeque::from([0usize]);
        while let Some(block_id) = ready.pop_front() {
            let block = &function.blocks[block_id];
            let mut state = incoming[block_id].clone().expect("queued block has state");
            for param in &block.params {
                if matches!(function.value_types[param.0], Type::Data(_)) {
                    state.values[param.0] = true;
                }
            }
            let value_use = |state: &mut OwnershipState,
                             value: ValueId,
                             consume: bool,
                             span: Span| {
                if matches!(function.value_types[value.0], Type::Data(_)) {
                    if !state.values[value.0] {
                        return Err(OwnershipError {
                            span,
                            message: format!("{}: use after move of v{}", function.name, value.0),
                        });
                    }
                    if consume {
                        state.values[value.0] = false;
                    }
                }
                Ok(())
            };
            for instruction in &block.instructions {
                let span = instruction.span;
                match &instruction.operation {
                    Operation::Move(value) => value_use(&mut state, *value, true, span)?,
                    Operation::Variant(_, _, payload) => {
                        if let Some(value) = payload {
                            value_use(&mut state, *value, true, span)?;
                        }
                    }
                    Operation::Product(_, fields) => {
                        for field in fields {
                            value_use(&mut state, *field, true, span)?;
                        }
                    }
                    Operation::Call(_, args) => {
                        for arg in args {
                            if let CallArg::Value(value) = arg {
                                value_use(&mut state, *value, true, span)?;
                            }
                        }
                    }
                    Operation::PlaceInit(place, value) | Operation::Store(place, value) => {
                        value_use(&mut state, *value, true, span)?;
                        if matches!(function.place_types[place.0], Type::Data(_)) {
                            state.places[place.0] = true;
                        }
                    }
                    Operation::Take(place) => {
                        if !state.places[place.0] {
                            return Err(OwnershipError {
                                span,
                                message: format!(
                                    "{}: use after move of p{}",
                                    function.name, place.0
                                ),
                            });
                        }
                        state.places[place.0] = false;
                    }
                    Operation::ReadField(place, _) => {
                        if !state.places[place.0] {
                            return Err(OwnershipError {
                                span,
                                message: format!(
                                    "{}: use after move of p{}",
                                    function.name, place.0
                                ),
                            });
                        }
                    }
                    other => {
                        for value in operands(other) {
                            value_use(&mut state, value, false, span)?;
                        }
                    }
                }
                if let Some(result) = instruction.result
                    && matches!(function.value_types[result.0], Type::Data(_))
                {
                    state.values[result.0] = true;
                }
            }
            let span = block
                .instructions
                .last()
                .map(|item| item.span)
                .unwrap_or(Span { start: 0, end: 0 });
            let successors = match &block.terminator {
                Terminator::Return(value) => {
                    value_use(&mut state, *value, true, span)?;
                    vec![]
                }
                Terminator::Jump(target, args) => {
                    for value in args {
                        value_use(&mut state, *value, true, span)?;
                    }
                    vec![target.0]
                }
                Terminator::Branch {
                    then_block,
                    else_block,
                    ..
                } => {
                    vec![then_block.0, else_block.0]
                }
                Terminator::Match { subject, arms } => {
                    value_use(&mut state, *subject, true, span)?;
                    arms.iter().map(|arm| arm.target.0).collect()
                }
            };
            for target in successors {
                match &mut incoming[target] {
                    None => {
                        incoming[target] = Some(state.clone());
                        ready.push_back(target);
                    }
                    Some(prior) => {
                        if prior.intersect(&state) {
                            ready.push_back(target);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn verify_structure(program: &Program) -> Result<(), String> {
    if program.functions.is_empty() {
        return Err("empty Core IR program".into());
    }
    let mut data_names = std::collections::HashSet::new();
    for (index, definition) in program.data.iter().enumerate() {
        if !data_names.insert(&definition.name)
            || definition.variants.is_empty() == definition.fields.is_empty()
            || definition.payloads.len() != definition.variants.len()
        {
            return Err("invalid data declaration".into());
        }
        let mut variants = std::collections::HashSet::new();
        if definition
            .variants
            .iter()
            .any(|variant| !variants.insert(variant))
        {
            return Err("duplicate data variant".into());
        }
        if definition.payloads.iter().flatten().any(|ty| match ty {
            Type::I64 | Type::Bool => false,
            Type::Data(id) => id.0 >= index,
            Type::RefI64 | Type::RefMutI64 => true,
        }) {
            return Err("invalid variant payload type".into());
        }
        let mut fields = std::collections::HashSet::new();
        for (name, ty) in &definition.fields {
            if !fields.insert(name) || !matches!(ty, Type::I64 | Type::Bool) {
                return Err("invalid product field".into());
            }
        }
    }
    for function in &program.functions {
        for ty in function
            .value_types
            .iter()
            .chain(&function.place_types)
            .chain(std::iter::once(&function.return_type))
        {
            if let Type::Data(id) = ty
                && id.0 >= program.data.len()
            {
                return Err(format!("{}: invalid data type ID", function.name));
            }
        }
        if !matches!(function.return_type, Type::I64 | Type::Bool | Type::Data(_)) {
            return Err(format!("{}: invalid return type", function.name));
        }
        if function.blocks.is_empty() {
            return Err(format!("{}: no entry block", function.name));
        }
        if function.blocks[0].params.len() > 4 {
            return Err(format!(
                "{}: at most four parameters are supported",
                function.name
            ));
        }
        if function.param_modes.len() != function.blocks[0].params.len() {
            return Err(format!("{}: parameter mode count mismatch", function.name));
        }
        if function
            .place_types
            .iter()
            .any(|ty| !matches!(ty, Type::I64 | Type::Bool | Type::Data(_)))
        {
            return Err(format!(
                "{}: references cannot be stored in places",
                function.name
            ));
        }
        let count = function.value_types.len();
        let mut definitions = vec![None; count];
        let mut place_initializers = vec![None; function.place_types.len()];
        for (block_id, block) in function.blocks.iter().enumerate() {
            for (param_index, param) in block.params.iter().enumerate() {
                let slot = definitions
                    .get_mut(param.0)
                    .ok_or_else(|| format!("{}: invalid value ID", function.name))?;
                if slot.replace((block_id, None)).is_some() {
                    return Err(format!("{}: duplicate value definition", function.name));
                }
                if block_id == 0 {
                    let expected = match function.param_modes[param_index] {
                        ParamMode::Own => function.value_types[param.0],
                        ParamMode::Shared => Type::RefI64,
                        ParamMode::Exclusive => Type::RefMutI64,
                    };
                    if param_index != param.0
                        || function.value_types[param.0] != expected
                        || (function.param_modes[param_index] == ParamMode::Own
                            && !matches!(expected, Type::I64 | Type::Bool | Type::Data(_)))
                    {
                        return Err(format!("{}: invalid function parameter", function.name));
                    }
                } else if matches!(
                    function.value_types[param.0],
                    Type::RefI64 | Type::RefMutI64
                ) {
                    return Err(format!(
                        "{}: reference cannot be a block parameter",
                        function.name
                    ));
                }
            }
            for (index, instruction) in block.instructions.iter().enumerate() {
                if let Some(result) = instruction.result {
                    let slot = definitions
                        .get_mut(result.0)
                        .ok_or_else(|| format!("{}: invalid value ID", function.name))?;
                    if slot.replace((block_id, Some(index))).is_some() {
                        return Err(format!("{}: duplicate value definition", function.name));
                    }
                }
                if let Operation::PlaceInit(place, _) = instruction.operation {
                    let slot = place_initializers
                        .get_mut(place.0)
                        .ok_or_else(|| format!("{}: invalid place ID", function.name))?;
                    if slot.replace((block_id, index)).is_some() {
                        return Err(format!("{}: duplicate place initialization", function.name));
                    }
                }
            }
        }
        if definitions.iter().any(Option::is_none) {
            return Err(format!("{}: undefined value ID", function.name));
        }
        if place_initializers.iter().any(Option::is_none) {
            return Err(format!("{}: uninitialized place", function.name));
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
                Terminator::Match { arms, .. } => arms.iter().map(|arm| arm.target).collect(),
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
        let check_place = |place: PlaceId, block_id: usize, index: usize| -> Result<(), String> {
            let (init_block, init_index) =
                place_initializers
                    .get(place.0)
                    .and_then(|item| *item)
                    .ok_or_else(|| format!("{}: invalid place ID", function.name))?;
            if init_block == block_id {
                if init_index >= index {
                    return Err(format!(
                        "{}: place used before initialization",
                        function.name
                    ));
                }
            } else if !dominators[block_id].contains(&init_block) {
                return Err(format!(
                    "{}: non-dominating place initialization",
                    function.name
                ));
            }
            Ok(())
        };
        for (block_id, block) in function.blocks.iter().enumerate() {
            for (index, instruction) in block.instructions.iter().enumerate() {
                for operand in operands(&instruction.operation) {
                    check_use(operand, block_id, index)?;
                }
                let expected = op_type(program, function, &instruction.operation)?;
                if instruction
                    .result
                    .map(|result| function.value_types[result.0])
                    != expected
                {
                    return Err(format!(
                        "{}: instruction result type mismatch",
                        function.name
                    ));
                }
                if let Operation::Load(place)
                | Operation::Take(place)
                | Operation::ReadField(place, _)
                | Operation::Store(place, _) = instruction.operation
                {
                    check_place(place, block_id, index)?;
                }
                if let Operation::Call(_, args) = &instruction.operation {
                    for arg in args {
                        if let CallArg::Borrow(place, _) = arg {
                            check_place(*place, block_id, index)?;
                        }
                    }
                }
            }
            let end = block.instructions.len();
            match &block.terminator {
                Terminator::Return(value) => {
                    check_use(*value, block_id, end)?;
                    if function.value_types[value.0] != function.return_type {
                        return Err(format!("{}: return type mismatch", function.name));
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
                Terminator::Match { subject, arms } => {
                    check_use(*subject, block_id, end)?;
                    let Type::Data(id) = function.value_types[subject.0] else {
                        return Err(format!("{}: match requires a sum value", function.name));
                    };
                    let data = &program.data[id.0];
                    if data.variants.is_empty() || arms.len() != data.variants.len() {
                        return Err(format!("{}: match arm count mismatch", function.name));
                    }
                    let mut distinct = std::collections::HashSet::new();
                    for (tag, arm) in arms.iter().enumerate() {
                        if !distinct.insert(arm.target.0) {
                            return Err(format!("{}: duplicate match target", function.name));
                        }
                        let params = &function.blocks[arm.target.0].params;
                        let valid = match arm.binding {
                            MatchBinding::Discard => params.is_empty(),
                            MatchBinding::Payload => data.payloads[tag].is_some_and(|ty| {
                                params.len() == 1 && function.value_types[params[0].0] == ty
                            }),
                            MatchBinding::Whole => {
                                params.len() == 1
                                    && function.value_types[params[0].0] == Type::Data(id)
                            }
                        };
                        if !valid {
                            return Err(format!(
                                "{}: match payload parameter mismatch",
                                function.name
                            ));
                        }
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

pub fn verify(program: &Program) -> Result<(), String> {
    verify_structure(program)?;
    verify_ownership(program).map_err(|error| error.message)
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
        let (id, function) = self
            .function(name)
            .ok_or_else(|| format!("unknown function {name}"))?;
        if function
            .param_modes
            .iter()
            .any(|mode| *mode != ParamMode::Own)
        {
            return Err("borrowed parameters require a Phoebe caller".into());
        }
        if function.return_type != Type::I64
            || function.blocks[0]
                .params
                .iter()
                .any(|param| function.value_types[param.0] != Type::I64)
        {
            return Err("external interpreter entry requires an I64 signature".into());
        }
        let args = args
            .iter()
            .map(|value| Value::I64(*value))
            .collect::<Vec<_>>();
        Ok(as_i64(self.interpret_id(id, &args, &mut Vec::new(), 0)?))
    }

    fn interpret_id(
        &self,
        id: FunctionId,
        args: &[Value],
        stores: &mut Vec<Option<Value>>,
        depth: usize,
    ) -> Result<Value, String> {
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
        let base = stores.len();
        stores.resize(base + function.place_types.len(), None);
        for (param, arg) in function.blocks[0].params.iter().zip(args) {
            values[param.0] = Some(arg.clone());
        }
        let mut current = 0usize;
        loop {
            let block = &function.blocks[current];
            for instruction in &block.instructions {
                let get = |id: ValueId| values[id.0].clone().expect("verified value is defined");
                let value = match &instruction.operation {
                    Operation::Const(x) => Some(Value::I64(*x)),
                    Operation::Bool(x) => Some(Value::Bool(*x)),
                    Operation::Move(value) => Some(get(*value)),
                    Operation::Variant(data_id, tag, payload) => Some(Value::Data(
                        *data_id,
                        *tag,
                        payload.map(|value| Box::new(get(value))),
                    )),
                    Operation::Product(data_id, fields) => Some(Value::Product(
                        *data_id,
                        fields.iter().map(|field| get(*field)).collect(),
                    )),
                    Operation::Field(owner, index) => {
                        let Value::Product(_, fields) = get(*owner) else {
                            unreachable!("verified product")
                        };
                        Some(fields[*index].clone())
                    }
                    Operation::ReadField(place, index) => {
                        let Some(Value::Product(_, fields)) = &stores[base + place.0] else {
                            unreachable!("verified live product place")
                        };
                        Some(fields[*index].clone())
                    }
                    Operation::Neg(a) => Some(Value::I64(as_i64(get(*a)).wrapping_neg())),
                    Operation::Add(a, b) => {
                        Some(Value::I64(as_i64(get(*a)).wrapping_add(as_i64(get(*b)))))
                    }
                    Operation::Sub(a, b) => {
                        Some(Value::I64(as_i64(get(*a)).wrapping_sub(as_i64(get(*b)))))
                    }
                    Operation::Mul(a, b) => {
                        Some(Value::I64(as_i64(get(*a)).wrapping_mul(as_i64(get(*b)))))
                    }
                    Operation::Eq(a, b) => Some(Value::Bool(get(*a) == get(*b))),
                    Operation::Lt(a, b) => Some(Value::Bool(as_i64(get(*a)) < as_i64(get(*b)))),
                    Operation::Call(target, args) => {
                        let args = args
                            .iter()
                            .map(|arg| match arg {
                                CallArg::Value(value) | CallArg::Ref(value, _) => get(*value),
                                CallArg::Borrow(place, _) => Value::Ref(base + place.0),
                            })
                            .collect::<Vec<_>>();
                        Some(self.interpret_id(*target, &args, stores, depth + 1)?)
                    }
                    Operation::PlaceInit(place, value) | Operation::Store(place, value) => {
                        stores[base + place.0] = Some(get(*value));
                        None
                    }
                    Operation::Load(place) => Some(
                        stores[base + place.0]
                            .clone()
                            .expect("verified place is initialized"),
                    ),
                    Operation::Take(place) => {
                        Some(stores[base + place.0].take().expect("verified owned place"))
                    }
                    Operation::LoadRef(reference) => {
                        let Value::Ref(slot) = get(*reference) else {
                            unreachable!("verified reference")
                        };
                        Some(stores[slot].clone().expect("live borrowed place"))
                    }
                    Operation::StoreRef(reference, value) => {
                        let Value::Ref(slot) = get(*reference) else {
                            unreachable!("verified reference")
                        };
                        stores[slot] = Some(get(*value));
                        None
                    }
                };
                if let Some(result) = instruction.result {
                    values[result.0] = value;
                }
            }
            let get = |id: ValueId| values[id.0].clone().expect("verified value is defined");
            match &block.terminator {
                Terminator::Return(value) => {
                    let result = get(*value);
                    stores.truncate(base);
                    return Ok(result);
                }
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
                Terminator::Match { subject, arms } => {
                    let original = get(*subject);
                    let Value::Data(_, tag, payload) = &original else {
                        unreachable!("verified sum value")
                    };
                    let arm = arms[*tag];
                    if let Some(param) = function.blocks[arm.target.0].params.first() {
                        values[param.0] = Some(match arm.binding {
                            MatchBinding::Payload => *payload.clone().expect("verified payload"),
                            MatchBinding::Whole => original,
                            MatchBinding::Discard => unreachable!("verified match binding"),
                        });
                    }
                    current = arm.target.0;
                }
            }
        }
    }
}

fn as_i64(value: Value) -> i64 {
    match value {
        Value::I64(x) => x,
        Value::Bool(_) | Value::Data(_, _, _) | Value::Product(_, _) | Value::Ref(_) => {
            unreachable!("verified I64 operand")
        }
    }
}

fn as_bool(value: Value) -> bool {
    match value {
        Value::Bool(x) => x,
        Value::I64(_) | Value::Data(_, _, _) | Value::Product(_, _) | Value::Ref(_) => {
            unreachable!("verified Bool operand")
        }
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
                write!(f, "v{}: {:?}", param.0, function.value_types[param.0])?;
            }
            writeln!(f, ") -> {:?} {{", function.return_type)?;
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
                    if let Some(result) = instruction.result {
                        write!(
                            f,
                            "    v{}: {:?} = ",
                            result.0, function.value_types[result.0]
                        )?;
                    } else {
                        write!(f, "    ")?;
                    }
                    match &instruction.operation {
                        Operation::Const(x) => write!(f, "const {x}")?,
                        Operation::Bool(x) => write!(f, "bool {x}")?,
                        Operation::Move(value) => write!(f, "move v{}", value.0)?,
                        Operation::Variant(id, tag, payload) => write!(
                            f,
                            "variant {}.{}",
                            self.data[id.0].name, self.data[id.0].variants[*tag]
                        )
                        .and_then(|()| match payload {
                            Some(value) => write!(f, "(v{})", value.0),
                            None => Ok(()),
                        })?,
                        Operation::Product(id, fields) => {
                            write!(f, "product {}(", self.data[id.0].name)?;
                            for (index, field) in fields.iter().enumerate() {
                                if index > 0 {
                                    write!(f, ", ")?;
                                }
                                write!(f, "v{}", field.0)?;
                            }
                            write!(f, ")")?;
                        }
                        Operation::Field(owner, index) => {
                            write!(f, "field v{}, {}", owner.0, index)?
                        }
                        Operation::ReadField(place, index) => {
                            write!(f, "place.field p{}, {}", place.0, index)?
                        }
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
                                match arg {
                                    CallArg::Value(value) => write!(f, "v{}", value.0)?,
                                    CallArg::Borrow(place, false) => write!(f, "&p{}", place.0)?,
                                    CallArg::Borrow(place, true) => write!(f, "&mut p{}", place.0)?,
                                    CallArg::Ref(value, false) => write!(f, "&v{}", value.0)?,
                                    CallArg::Ref(value, true) => write!(f, "&mut v{}", value.0)?,
                                }
                            }
                            write!(f, ")")?;
                        }
                        Operation::PlaceInit(place, value) => {
                            write!(f, "place.init p{}, v{}", place.0, value.0)?
                        }
                        Operation::Load(place) => write!(f, "place.load p{}", place.0)?,
                        Operation::Take(place) => write!(f, "place.take p{}", place.0)?,
                        Operation::Store(place, value) => {
                            write!(f, "place.store p{}, v{}", place.0, value.0)?
                        }
                        Operation::LoadRef(reference) => write!(f, "ref.load v{}", reference.0)?,
                        Operation::StoreRef(reference, value) => {
                            write!(f, "ref.store v{}, v{}", reference.0, value.0)?
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
                    Terminator::Match { subject, arms } => {
                        write!(f, "    match v{}", subject.0)?;
                        for (tag, arm) in arms.iter().enumerate() {
                            write!(f, ", {}->block{}:{:?}", tag, arm.target.0, arm.binding)?;
                        }
                        writeln!(f)?;
                    }
                }
            }
            writeln!(f, "}}")?;
        }
        Ok(())
    }
}
