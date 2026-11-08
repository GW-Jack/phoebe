//! Scoped scalar source loans and their normal exit.

use super::*;

impl Lowerer<'_> {
    pub(super) fn borrow_body(
        &mut self,
        alias: &str,
        span: Span,
        value: &Expr,
        body: &Body,
        locals: &HashMap<String, Binding>,
    ) -> Result<Option<TypedValue>, Diagnostic> {
        let ExprKind::Borrow {
            name: source,
            mutable,
        } = &value.kind
        else {
            unreachable!("borrow body requires a borrow expression")
        };
        let mutable = *mutable;
        let borrow_span = value.span;
        if locals.contains_key(alias) {
            return Err(Diagnostic::new(span, format!("duplicate local {alias}")));
        }
        let binding = locals
            .get(source)
            .copied()
            .ok_or_else(|| Diagnostic::new(borrow_span, format!("unknown local {source}")))?;
        let mode = if mutable {
            ParamMode::Exclusive
        } else {
            ParamMode::Shared
        };
        let (operation, referent) = match binding {
            Binding::Var(place) => (
                Operation::BorrowBegin(place, mutable),
                self.place_types[place.0],
            ),
            Binding::Val(value) if !mutable => {
                let ty = self.value_types[value.0];
                let place = self.place(ty);
                self.emit_effect(Operation::PlaceInit(place, value), borrow_span);
                (Operation::BorrowBegin(place, false), ty)
            }
            Binding::Ref(parent, parent_mode) if self.lexical_loans.contains(&parent) => {
                if mutable && parent_mode != ParamMode::Exclusive {
                    return Err(Diagnostic::new(
                        borrow_span,
                        "exclusive reborrow requires an exclusive parent",
                    ));
                }
                let ty = self.value_types[parent.0]
                    .referent()
                    .expect("validated lexical reference");
                (Operation::BorrowReborrow(parent, mutable), ty)
            }
            _ => {
                return Err(Diagnostic::new(
                    borrow_span,
                    "this binding cannot grant the requested lexical borrow",
                ));
            }
        };
        let reference_type = Type::parameter(mode, referent).ok_or_else(|| {
            Diagnostic::new(borrow_span, "lexical borrow requires an I64 or Bool owner")
        })?;
        let reference = self.emit(operation, reference_type, borrow_span);
        self.lexical_loans.push(reference);
        let mut scoped_locals = locals.clone();
        scoped_locals.insert(alias.to_owned(), Binding::Ref(reference, mode));
        let result = self.body(body, &mut scoped_locals)?;
        self.lexical_loans.pop();
        self.emit_effect(Operation::BorrowEnd(reference), span);
        let next = self.block();
        let output = if let Some((value, ty)) = result {
            let param = self.block_param(next, ty);
            self.terminate(Terminator::Jump(next, vec![value]));
            Some((param, ty))
        } else {
            self.terminate(Terminator::Jump(next, vec![]));
            None
        };
        self.current = next;
        Ok(output)
    }
}
