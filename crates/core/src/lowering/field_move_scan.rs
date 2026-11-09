//! Identify names whose product fields are projected in source. Lowering uses
//! the checked field types to decide which projections need an owned place.

use super::*;
use std::collections::HashSet;

pub(super) fn projected_fields(body: &Body) -> HashSet<(String, String)> {
    let mut fields = HashSet::new();
    visit_body(body, &mut fields);
    fields
}

fn visit_body(body: &Body, fields: &mut HashSet<(String, String)>) {
    for statement in &body.statements {
        match statement {
            Statement::Val(_, _, value)
            | Statement::Var(_, _, value)
            | Statement::Assign(_, _, value)
            | Statement::ArrayAssign(_, _, _, value)
            | Statement::Expr(value)
            | Statement::UnpackVal { value, .. } => visit_expr(value, fields),
            Statement::ChannelVal { capacity, .. }
            | Statement::RequestChannelVal { capacity, .. } => visit_expr(capacity, fields),
            Statement::Spawn { args, .. } => visit_args(args, fields),
            Statement::Scope { body, .. } => visit_body(body, fields),
            Statement::With { value, body, .. } => {
                visit_expr(value, fields);
                visit_body(body, fields);
            }
            Statement::While(condition, body) => {
                visit_expr(condition, fields);
                visit_body(body, fields);
            }
            Statement::ReplyVal { .. } | Statement::Yield(_) => {}
        }
    }
    if let Some(tail) = &body.tail {
        visit_expr(tail, fields);
    }
}

fn visit_args(args: &[CallArgument], fields: &mut HashSet<(String, String)>) {
    for arg in args {
        if let CallArgument::Value(value) = arg {
            visit_expr(value, fields);
        }
    }
}

fn visit_expr(expr: &Expr, fields: &mut HashSet<(String, String)>) {
    match &expr.kind {
        ExprKind::Member(owner, field) => {
            fields.insert((owner.clone(), field.clone()));
        }
        ExprKind::FixedArray(values) => {
            for value in values {
                visit_expr(value, fields);
            }
        }
        ExprKind::FixedIndex(value, _)
        | ExprKind::Copy(value)
        | ExprKind::Neg(value)
        | ExprKind::Not(value)
        | ExprKind::Try(value)
        | ExprKind::VariantCall(_, _, value) => visit_expr(value, fields),
        ExprKind::Add(left, right)
        | ExprKind::Sub(left, right)
        | ExprKind::Mul(left, right)
        | ExprKind::Eq(left, right)
        | ExprKind::Ne(left, right)
        | ExprKind::Lt(left, right)
        | ExprKind::Le(left, right)
        | ExprKind::Gt(left, right)
        | ExprKind::Ge(left, right)
        | ExprKind::And(left, right)
        | ExprKind::Or(left, right) => {
            visit_expr(left, fields);
            visit_expr(right, fields);
        }
        ExprKind::If(condition, then_body, else_body) => {
            visit_expr(condition, fields);
            visit_body(then_body, fields);
            visit_body(else_body, fields);
        }
        ExprKind::When(subject, arms) => {
            visit_expr(subject, fields);
            for arm in arms {
                visit_body(&arm.body, fields);
            }
        }
        ExprKind::With { value, body, .. } => {
            visit_expr(value, fields);
            visit_body(body, fields);
        }
        ExprKind::Call(_, args) | ExprKind::QualifiedCall(_, _, args) => {
            visit_args(args, fields);
        }
        ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Name(_) | ExprKind::Borrow { .. } => {}
    }
}
