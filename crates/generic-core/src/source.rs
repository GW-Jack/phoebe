use std::collections::HashMap;

use phoebe_syntax::syntax::{self, ExprKind, ParamMode, Statement, TypeName};
use phoebe_syntax::{Diagnostic, Span};

use crate::{Block, BlockId, Function, Instruction, Operation, Terminator, Type, ValueId, verify};

fn source_type(
    name: &TypeName,
    binders: &HashMap<&str, usize>,
    span: Span,
) -> Result<Type, Diagnostic> {
    match name {
        TypeName::I64 => Ok(Type::I64),
        TypeName::Bool => Ok(Type::Bool),
        TypeName::Unit => Ok(Type::Unit),
        TypeName::Named(name) if binders.contains_key(name.as_str()) => {
            Ok(Type::Param(binders[name.as_str()]))
        }
        _ => Err(Diagnostic::new(
            span,
            "this generic Core slice accepts only scalar and bound type-parameter signatures",
        )),
    }
}

fn local(locals: &HashMap<String, ValueId>, name: &str, span: Span) -> Result<ValueId, Diagnostic> {
    locals
        .get(name)
        .copied()
        .ok_or_else(|| Diagnostic::new(span, format!("unknown local {name}")))
}

fn branch_result(
    body: &syntax::Body,
    locals: &HashMap<String, ValueId>,
    span: Span,
) -> Result<ValueId, Diagnostic> {
    if !body.statements.is_empty() {
        return Err(Diagnostic::new(
            span,
            "generic branch statements are not implemented yet",
        ));
    }
    let tail = body
        .tail
        .as_ref()
        .ok_or_else(|| Diagnostic::new(span, "generic branch needs a final value"))?;
    match &tail.kind {
        ExprKind::Name(name) => local(locals, name, tail.span),
        _ => Err(Diagnostic::new(
            tail.span,
            "generic branch result must be a named value in this slice",
        )),
    }
}

/// Lower the first source subset for a generic definition into parametric
/// Core. The result is independently verified before it is returned.
pub fn lower(source: &syntax::Function) -> Result<Function, Diagnostic> {
    if source.type_params.is_empty() {
        return Err(Diagnostic::new(
            source.name_span,
            "generic Core requires at least one type parameter",
        ));
    }
    let mut binders = HashMap::new();
    for (index, (name, span)) in source.type_params.iter().enumerate() {
        if matches!(name.as_str(), "I64" | "Bool" | "Unit" | "ResourceError") {
            return Err(Diagnostic::new(
                *span,
                format!("{name} cannot name a type parameter"),
            ));
        }
        if binders.insert(name.as_str(), index).is_some() {
            return Err(Diagnostic::new(
                *span,
                format!("duplicate type parameter {name}"),
            ));
        }
    }
    let mut params = Vec::with_capacity(source.params.len());
    let mut locals = HashMap::new();
    for (index, param) in source.params.iter().enumerate() {
        if param.mode != ParamMode::Own {
            return Err(Diagnostic::new(
                param.span,
                "generic borrows are not implemented yet",
            ));
        }
        params.push(source_type(&param.ty, &binders, param.span)?);
        if locals.insert(param.name.clone(), ValueId(index)).is_some() {
            return Err(Diagnostic::new(
                param.span,
                format!("duplicate parameter {}", param.name),
            ));
        }
    }
    let result = source_type(&source.return_type, &binders, source.name_span)?;
    let mut function = Function {
        name: source.name.clone(),
        type_params: source.type_params.len(),
        params: params.clone(),
        result,
        value_types: params,
        blocks: vec![Block {
            params: (0..source.params.len()).map(ValueId).collect(),
            instructions: Vec::new(),
            terminator: Terminator::Return(ValueId(0)),
        }],
    };
    for statement in &source.body.statements {
        let Statement::Val(name, span, expr) = statement else {
            return Err(Diagnostic::new(
                source.name_span,
                "this generic Core slice supports only val bindings",
            ));
        };
        if locals.contains_key(name) {
            return Err(Diagnostic::new(*span, format!("duplicate local {name}")));
        }
        let (operation, input) = match &expr.kind {
            ExprKind::Name(source_name) => {
                let input = local(&locals, source_name, expr.span)?;
                (Operation::Move(input), input)
            }
            ExprKind::Copy(inner) => {
                let ExprKind::Name(source_name) = &inner.kind else {
                    return Err(Diagnostic::new(
                        inner.span,
                        "generic copy needs a named scalar",
                    ));
                };
                let input = local(&locals, source_name, inner.span)?;
                if matches!(function.value_types[input.0], Type::Param(_)) {
                    return Err(Diagnostic::new(
                        expr.span,
                        "opaque type parameter cannot be copied",
                    ));
                }
                (Operation::CopyScalar(input), input)
            }
            _ => {
                return Err(Diagnostic::new(
                    expr.span,
                    "generic val initializer is not implemented yet",
                ));
            }
        };
        let value = ValueId(function.value_types.len());
        function.value_types.push(function.value_types[input.0]);
        function.blocks[0].instructions.push(Instruction {
            result: value,
            operation,
        });
        locals.insert(name.clone(), value);
    }
    let tail = source
        .body
        .tail
        .as_ref()
        .ok_or_else(|| Diagnostic::new(source.name_span, "generic body needs a final value"))?;
    function.blocks[0].terminator = match &tail.kind {
        ExprKind::Name(name) => Terminator::Return(local(&locals, name, tail.span)?),
        ExprKind::If(condition, when_true, when_false) => {
            let ExprKind::Name(condition_name) = &condition.kind else {
                return Err(Diagnostic::new(
                    condition.span,
                    "generic branch condition must be a named Bool",
                ));
            };
            let condition_value = local(&locals, condition_name, condition.span)?;
            if function.value_types[condition_value.0] != Type::Bool {
                return Err(Diagnostic::new(
                    condition.span,
                    "generic branch condition must be Bool",
                ));
            }
            let true_value = branch_result(when_true, &locals, tail.span)?;
            let false_value = branch_result(when_false, &locals, tail.span)?;
            if function.value_types[true_value.0] != result
                || function.value_types[false_value.0] != result
            {
                return Err(Diagnostic::new(
                    tail.span,
                    "generic branch result type mismatch",
                ));
            }
            let true_param = ValueId(function.value_types.len());
            function.value_types.push(result);
            let false_param = ValueId(function.value_types.len());
            function.value_types.push(result);
            function.blocks.push(Block {
                params: vec![true_param],
                instructions: Vec::new(),
                terminator: Terminator::Return(true_param),
            });
            function.blocks.push(Block {
                params: vec![false_param],
                instructions: Vec::new(),
                terminator: Terminator::Return(false_param),
            });
            Terminator::Branch {
                condition: condition_value,
                when_true: BlockId(1),
                true_args: vec![true_value],
                when_false: BlockId(2),
                false_args: vec![false_value],
            }
        }
        _ => {
            return Err(Diagnostic::new(
                tail.span,
                "generic final expression is not implemented yet",
            ));
        }
    };
    verify(&function).map_err(|message| Diagnostic::new(source.name_span, message))?;
    Ok(function)
}
