//! Scoped scalar source loans and their normal exit.

use super::*;

impl Lowerer<'_> {
    pub(super) fn resolve_borrow_path(
        &self,
        mut ty: Type,
        fields: &[String],
        span: Span,
    ) -> Result<(Vec<usize>, Type), Diagnostic> {
        let mut path = Vec::with_capacity(fields.len());
        for field in fields {
            let Type::Data(id) = ty else {
                return Err(Diagnostic::new(
                    span,
                    "field borrow crosses a non-product value",
                ));
            };
            let (index, (_, next)) = self.data[id.0]
                .fields
                .iter()
                .enumerate()
                .find(|(_, (name, _))| name == field)
                .ok_or_else(|| Diagnostic::new(span, format!("unknown product field {field}")))?;
            path.push(index);
            ty = *next;
        }
        Ok((path, ty))
    }

    /// A shared borrow of an immutable binding can use a scalar snapshot.
    /// Only the selected closed path is projected; rights in sibling fields
    /// remain owned by the original value.
    pub(super) fn project_shared_field(
        &mut self,
        value: ValueId,
        fields: &[String],
        span: Span,
    ) -> Result<(ValueId, Type), Diagnostic> {
        let (path, _) = self.resolve_borrow_path(self.value_types[value.0], fields, span)?;
        let mut current = value;
        let mut ty = self.value_types[value.0];
        for index in path {
            let Type::Data(id) = ty else {
                unreachable!("resolved product field path")
            };
            let field_ty = self.data[id.0].fields[index].1;
            if self.capability_layout.leaves(field_ty) != 0 {
                return Err(Diagnostic::new(
                    span,
                    "shared field borrow cannot project a communication capability",
                ));
            }
            current = self.emit(Operation::Field(current, index), field_ty, span);
            ty = field_ty;
        }
        Ok((current, ty))
    }

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
            path,
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
            Binding::Var(place) => {
                let (indices, ty) =
                    self.resolve_borrow_path(self.place_types[place.0], path, borrow_span)?;
                let operation = if indices.is_empty() {
                    Operation::BorrowBegin(place, mutable)
                } else {
                    Operation::BorrowBeginPath(place, indices, mutable)
                };
                (operation, ty)
            }
            Binding::Val(value) if !mutable => {
                let (value, ty) = if path.is_empty() {
                    (value, self.value_types[value.0])
                } else {
                    self.project_shared_field(value, path, borrow_span)?
                };
                let place = self.place(ty);
                self.emit_effect(Operation::PlaceInit(place, value), borrow_span);
                (Operation::BorrowBegin(place, false), ty)
            }
            Binding::Ref(parent, parent_mode) if path.is_empty() => {
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
                    if path.is_empty() {
                        "this binding cannot grant the requested lexical borrow"
                    } else {
                        "field borrow requires a mutable local place"
                    },
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
