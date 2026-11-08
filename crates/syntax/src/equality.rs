//! Exact AST comparison for incremental reuse, allowing one module offset.

use crate::Span;
use crate::syntax::{self, Body, CallArgument, Expr, ExprKind, Statement};

fn span(a: Span, b: Span, delta: isize) -> bool {
    a.start.checked_add_signed(delta) == Some(b.start)
        && a.end.checked_add_signed(delta) == Some(b.end)
}

fn args(a: &[CallArgument], b: &[CallArgument], delta: isize) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| match (a, b) {
            (CallArgument::Value(a), CallArgument::Value(b)) => expr(a, b, delta),
            (
                CallArgument::Borrow {
                    name: an,
                    span: aspan,
                    mutable: am,
                },
                CallArgument::Borrow {
                    name: bn,
                    span: bspan,
                    mutable: bm,
                },
            ) => an == bn && am == bm && span(*aspan, *bspan, delta),
            _ => false,
        })
}

fn body(a: &Body, b: &Body, delta: isize) -> bool {
    a.statements.len() == b.statements.len()
        && a.statements
            .iter()
            .zip(&b.statements)
            .all(|(a, b)| statement(a, b, delta))
        && match (&a.tail, &b.tail) {
            (Some(a), Some(b)) => expr(a, b, delta),
            (None, None) => true,
            _ => false,
        }
}

fn statement(a: &Statement, b: &Statement, delta: isize) -> bool {
    match (a, b) {
        (Statement::Val(an, asp, ae), Statement::Val(bn, bsp, be))
        | (Statement::Var(an, asp, ae), Statement::Var(bn, bsp, be))
        | (Statement::Assign(an, asp, ae), Statement::Assign(bn, bsp, be)) => {
            an == bn && span(*asp, *bsp, delta) && expr(ae, be, delta)
        }
        (
            Statement::ChannelVal {
                sender: asend,
                receiver: arecv,
                capacity: ac,
                span: asp,
            },
            Statement::ChannelVal {
                sender: bsend,
                receiver: brecv,
                capacity: bc,
                span: bsp,
            },
        ) => {
            asend.0 == bsend.0
                && span(asend.1, bsend.1, delta)
                && arecv.0 == brecv.0
                && span(arecv.1, brecv.1, delta)
                && expr(ac, bc, delta)
                && span(*asp, *bsp, delta)
        }
        (
            Statement::RequestChannelVal {
                sender: asend,
                receiver: arecv,
                capacity: ac,
                span: asp,
            },
            Statement::RequestChannelVal {
                sender: bsend,
                receiver: brecv,
                capacity: bc,
                span: bsp,
            },
        ) => {
            asend.0 == bsend.0
                && span(asend.1, bsend.1, delta)
                && arecv.0 == brecv.0
                && span(arecv.1, brecv.1, delta)
                && expr(ac, bc, delta)
                && span(*asp, *bsp, delta)
        }
        (
            Statement::UnpackVal {
                names: an,
                value: aq,
                span: asp,
            },
            Statement::UnpackVal {
                names: bn,
                value: bq,
                span: bsp,
            },
        ) => {
            an.len() == bn.len()
                && an
                    .iter()
                    .zip(bn)
                    .all(|(a, b)| a.0 == b.0 && span(a.1, b.1, delta))
                && expr(aq, bq, delta)
                && span(*asp, *bsp, delta)
        }
        (
            Statement::ReplyVal {
                right: ar,
                receiver: av,
                span: aspan,
            },
            Statement::ReplyVal {
                right: br,
                receiver: bv,
                span: bspan,
            },
        ) => {
            ar.0 == br.0
                && span(ar.1, br.1, delta)
                && av.0 == bv.0
                && span(av.1, bv.1, delta)
                && span(*aspan, *bspan, delta)
        }
        (
            Statement::Spawn {
                child: ac,
                args: aa,
                span: asp,
            },
            Statement::Spawn {
                child: bc,
                args: ba,
                span: bsp,
            },
        ) => ac == bc && args(aa, ba, delta) && span(*asp, *bsp, delta),
        (
            Statement::Scope {
                span: asp,
                body: ab,
            },
            Statement::Scope {
                span: bsp,
                body: bb,
            },
        ) => span(*asp, *bsp, delta) && body(ab, bb, delta),
        (
            Statement::With {
                name: an,
                span: asp,
                value: av,
                body: ab,
            },
            Statement::With {
                name: bn,
                span: bsp,
                value: bv,
                body: bb,
            },
        ) => an == bn && span(*asp, *bsp, delta) && expr(av, bv, delta) && body(ab, bb, delta),
        (Statement::Yield(a), Statement::Yield(b)) => span(*a, *b, delta),
        (Statement::While(ac, ab), Statement::While(bc, bb)) => {
            expr(ac, bc, delta) && body(ab, bb, delta)
        }
        (Statement::Expr(a), Statement::Expr(b)) => expr(a, b, delta),
        _ => false,
    }
}

fn expr(a: &Expr, b: &Expr, delta: isize) -> bool {
    if !span(a.span, b.span, delta) {
        return false;
    }
    match (&a.kind, &b.kind) {
        (ExprKind::Int(a), ExprKind::Int(b)) => a == b,
        (ExprKind::Bool(a), ExprKind::Bool(b)) => a == b,
        (ExprKind::Name(a), ExprKind::Name(b)) => a == b,
        (ExprKind::Copy(a), ExprKind::Copy(b))
        | (ExprKind::Neg(a), ExprKind::Neg(b))
        | (ExprKind::Not(a), ExprKind::Not(b))
        | (ExprKind::Try(a), ExprKind::Try(b)) => expr(a, b, delta),
        (ExprKind::Add(a1, a2), ExprKind::Add(b1, b2))
        | (ExprKind::Sub(a1, a2), ExprKind::Sub(b1, b2))
        | (ExprKind::Mul(a1, a2), ExprKind::Mul(b1, b2))
        | (ExprKind::Eq(a1, a2), ExprKind::Eq(b1, b2))
        | (ExprKind::Ne(a1, a2), ExprKind::Ne(b1, b2))
        | (ExprKind::Lt(a1, a2), ExprKind::Lt(b1, b2))
        | (ExprKind::Le(a1, a2), ExprKind::Le(b1, b2))
        | (ExprKind::Gt(a1, a2), ExprKind::Gt(b1, b2))
        | (ExprKind::Ge(a1, a2), ExprKind::Ge(b1, b2))
        | (ExprKind::And(a1, a2), ExprKind::And(b1, b2))
        | (ExprKind::Or(a1, a2), ExprKind::Or(b1, b2)) => {
            expr(a1, b1, delta) && expr(a2, b2, delta)
        }
        (ExprKind::If(ac, at, ae), ExprKind::If(bc, bt, be)) => {
            expr(ac, bc, delta) && body(at, bt, delta) && body(ae, be, delta)
        }
        (ExprKind::Member(an, am), ExprKind::Member(bn, bm)) => an == bn && am == bm,
        (ExprKind::VariantCall(an, am, av), ExprKind::VariantCall(bn, bm, bv)) => {
            an == bn && am == bm && expr(av, bv, delta)
        }
        (ExprKind::QualifiedCall(an, am, aa), ExprKind::QualifiedCall(bn, bm, ba)) => {
            an == bn && am == bm && args(aa, ba, delta)
        }
        (ExprKind::Call(an, aa), ExprKind::Call(bn, ba)) => an == bn && args(aa, ba, delta),
        (ExprKind::When(asub, aa), ExprKind::When(bsub, ba)) => {
            expr(asub, bsub, delta)
                && aa.len() == ba.len()
                && aa.iter().zip(ba).all(|(a, b)| {
                    a.type_name == b.type_name
                        && a.variant == b.variant
                        && span(a.span, b.span, delta)
                        && match (&a.binding, &b.binding) {
                            (Some((an, asp)), Some((bn, bsp))) => {
                                an == bn && span(*asp, *bsp, delta)
                            }
                            (None, None) => true,
                            _ => false,
                        }
                        && body(&a.body, &b.body, delta)
                })
        }
        (
            ExprKind::With {
                name: an,
                value: av,
                body: ab,
            },
            ExprKind::With {
                name: bn,
                value: bv,
                body: bb,
            },
        ) => an == bn && expr(av, bv, delta) && body(ab, bb, delta),
        _ => false,
    }
}

pub fn same_function_with_shift(
    previous: &syntax::Function,
    current: &syntax::Function,
    delta: isize,
) -> bool {
    previous.name == current.name
        && previous.public == current.public
        && span(previous.name_span, current.name_span, delta)
        && previous.return_type == current.return_type
        && previous.params.len() == current.params.len()
        && previous.params.iter().zip(&current.params).all(|(a, b)| {
            a.name == b.name && a.mode == b.mode && a.ty == b.ty && span(a.span, b.span, delta)
        })
        && body(&previous.body, &current.body, delta)
}
