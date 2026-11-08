//! Surface sugar for a group of lexically owned communication rights.
//! Nested ordinary `with` nodes keep one Core cleanup rule for both forms.

use super::*;

impl Parser {
    pub(super) fn with_bindings(&mut self) -> Result<Vec<(String, Expr)>, Diagnostic> {
        if self.eat(&TokenKind::LParen).is_none() {
            let (name, _) = self.name()?;
            self.expect(&TokenKind::Equal, "= after with binding")?;
            return Ok(vec![(name, self.expr(0)?)]);
        }
        let mut bindings = Vec::new();
        loop {
            if bindings.len() == MAX_AST_DEPTH - 1 {
                return Err(Diagnostic::new(
                    self.current().span,
                    "source tree exceeds compiler limit",
                ));
            }
            let (name, _) = self.name()?;
            self.expect(&TokenKind::Equal, "= after with binding")?;
            bindings.push((name, self.expr(0)?));
            if self.eat(&TokenKind::Comma).is_none() {
                self.expect(&TokenKind::RParen, ") after with bindings")?;
                return Ok(bindings);
            }
        }
    }
}

pub(super) fn nest_with_expression(
    bindings: Vec<(String, Expr)>,
    mut body: Body,
    span: Span,
) -> Expr {
    for (name, value) in bindings.into_iter().rev() {
        let expr = Expr {
            kind: ExprKind::With {
                name,
                value: Box::new(value),
                body: Box::new(body),
            },
            span,
        };
        body = Body {
            statements: Vec::new(),
            tail: Some(expr),
        };
    }
    body.tail.expect("with has at least one binding")
}

pub(super) fn nest_with_statement(
    bindings: Vec<(String, Expr)>,
    mut body: Body,
    span: Span,
) -> Statement {
    for (name, value) in bindings.into_iter().rev() {
        let statement = Statement::With {
            name,
            value,
            body,
            span,
        };
        body = Body {
            statements: vec![statement],
            tail: None,
        };
    }
    body.statements
        .pop()
        .expect("with has at least one binding")
}
