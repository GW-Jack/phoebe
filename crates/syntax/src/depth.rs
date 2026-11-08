//! Bound the tree passed to recursive Core lowering without charging for
//! unrelated statements or functions.

use crate::syntax::{Body, CallArgument, Expr, ExprKind, Statement};

fn argument_height(argument: &CallArgument) -> usize {
    match argument {
        CallArgument::Value(value) => expr_height(value),
        CallArgument::Borrow { .. } => 1,
    }
}

fn arguments_height(arguments: &[CallArgument]) -> usize {
    arguments.iter().map(argument_height).max().unwrap_or(0)
}

pub(crate) fn expr_height(expr: &Expr) -> usize {
    1 + match &expr.kind {
        ExprKind::Int(_)
        | ExprKind::Bool(_)
        | ExprKind::Name(_)
        | ExprKind::Borrow { .. }
        | ExprKind::Member(_, _) => 0,
        ExprKind::Copy(value)
        | ExprKind::Neg(value)
        | ExprKind::Not(value)
        | ExprKind::Try(value) => expr_height(value),
        ExprKind::Add(a, b)
        | ExprKind::Sub(a, b)
        | ExprKind::Mul(a, b)
        | ExprKind::Eq(a, b)
        | ExprKind::Ne(a, b)
        | ExprKind::Lt(a, b)
        | ExprKind::Le(a, b)
        | ExprKind::Gt(a, b)
        | ExprKind::Ge(a, b)
        | ExprKind::And(a, b)
        | ExprKind::Or(a, b) => expr_height(a).max(expr_height(b)),
        ExprKind::VariantCall(_, _, value) => expr_height(value),
        ExprKind::Call(_, arguments) | ExprKind::QualifiedCall(_, _, arguments) => {
            arguments_height(arguments)
        }
        ExprKind::If(condition, yes, no) => expr_height(condition)
            .max(body_height(yes))
            .max(body_height(no)),
        ExprKind::When(subject, arms) => expr_height(subject).max(
            arms.iter()
                .map(|arm| body_height(&arm.body))
                .max()
                .unwrap_or(0),
        ),
        ExprKind::With { value, body, .. } => expr_height(value).max(body_height(body)),
    }
}

pub(crate) fn body_height(body: &Body) -> usize {
    let statements = body
        .statements
        .iter()
        .map(|statement| match statement {
            Statement::Val(_, _, value)
            | Statement::Var(_, _, value)
            | Statement::Assign(_, _, value)
            | Statement::Expr(value) => expr_height(value),
            Statement::ChannelVal { capacity, .. }
            | Statement::RequestChannelVal { capacity, .. } => expr_height(capacity),
            Statement::UnpackVal { value, .. } => expr_height(value),
            Statement::ReplyVal { .. } => 0,
            Statement::Spawn { args, .. } => arguments_height(args),
            Statement::Scope { body, .. } => body_height(body),
            Statement::With { value, body, .. } => expr_height(value).max(body_height(body)),
            Statement::While(condition, body) => expr_height(condition).max(body_height(body)),
            Statement::Yield(_) => 0,
        })
        .max()
        .unwrap_or(0);
    1 + statements.max(body.tail.as_ref().map(expr_height).unwrap_or(0))
}
