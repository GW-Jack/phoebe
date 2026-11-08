use super::*;

impl Parser {
    pub(super) fn data(&mut self, public: bool) -> Result<Data, Diagnostic> {
        self.expect(&TokenKind::Data, "data")?;
        let (name, span) = self.name()?;
        self.expect(&TokenKind::LBrace, "{")?;
        let mut variants = Vec::new();
        let mut fields = Vec::new();
        let mut product = None;
        loop {
            let (member, member_span) = self.name()?;
            let field = self.eat(&TokenKind::Colon).is_some();
            if product.is_some_and(|prior| prior != field) {
                return Err(Diagnostic::new(
                    member_span,
                    "cannot mix fields and variants",
                ));
            }
            product = Some(field);
            if field {
                let ty = self.type_name(false)?;
                fields.push(Field {
                    name: member,
                    ty,
                    span: member_span,
                });
            } else {
                let payload = if self.eat(&TokenKind::LParen).is_some() {
                    let (name, span) = self.name()?;
                    self.expect(&TokenKind::Colon, ":")?;
                    let ty = self.type_name(false)?;
                    self.expect(&TokenKind::RParen, ")")?;
                    Some(Field { name, ty, span })
                } else {
                    None
                };
                variants.push(Variant {
                    name: member,
                    span: member_span,
                    payload,
                });
            }
            if self.eat(&TokenKind::Comma).is_none() {
                self.expect(&TokenKind::RBrace, "}")?;
                break;
            }
            if self.eat(&TokenKind::RBrace).is_some() {
                break;
            }
        }
        Ok(Data {
            name,
            public,
            span,
            variants,
            fields,
        })
    }

    pub(super) fn function(&mut self, public: bool) -> Result<Function, Diagnostic> {
        self.expect(&TokenKind::Fn, "fn")?;
        let (name, name_span) = self.name()?;
        self.expect(&TokenKind::LParen, "(")?;
        let mut params = Vec::new();
        if self.eat(&TokenKind::RParen).is_none() {
            loop {
                let (param_name, param_span) = self.name()?;
                self.expect(&TokenKind::Colon, ":")?;
                let mode = if self.eat(&TokenKind::Amp).is_some() {
                    if self.eat(&TokenKind::Mut).is_some() {
                        ParamMode::Exclusive
                    } else {
                        ParamMode::Shared
                    }
                } else {
                    ParamMode::Own
                };
                let ty = self.type_name(mode != ParamMode::Own)?;
                params.push(Param {
                    name: param_name,
                    span: param_span,
                    mode,
                    ty,
                });
                if self.eat(&TokenKind::Comma).is_none() {
                    self.expect(&TokenKind::RParen, ")")?;
                    break;
                }
            }
        }
        self.expect(&TokenKind::Arrow, "->")?;
        let return_type = self.type_name(false)?;
        let body = if self.eat(&TokenKind::Equal).is_some() {
            Body {
                statements: Vec::new(),
                tail: Some(self.expr(0)?),
            }
        } else {
            self.block(true)?
        };
        Ok(Function {
            name,
            public,
            name_span,
            params,
            return_type,
            body,
        })
    }
}
