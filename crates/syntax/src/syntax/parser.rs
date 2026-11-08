use super::ast::*;
mod declarations;
mod expressions;
mod statements;
mod types;
use super::lexer::{Token, TokenKind, lex};
use crate::depth::{body_height, expr_height};
use crate::{Diagnostic, Span};

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
    applied_types: Vec<(TypeName, Span)>,
    uses_send_outcome: bool,
    uses_receive_outcome: bool,
    uses_complete_outcome: bool,
    uses_reply_outcome: bool,
}

const MAX_PARSE_DEPTH: usize = 48;
const MAX_AST_DEPTH: usize = 64;

impl Parser {
    fn nested<T>(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Result<T, Diagnostic>,
    ) -> Result<T, Diagnostic> {
        if self.depth == MAX_PARSE_DEPTH {
            return Err(Diagnostic::new(
                self.current().span,
                "source nesting exceeds compiler limit",
            ));
        }
        self.depth += 1;
        let result = parse(self);
        self.depth -= 1;
        result
    }

    fn current(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn eat(&mut self, kind: &TokenKind) -> Option<Token> {
        if &self.current().kind == kind {
            let token = self.current().clone();
            self.pos += 1;
            Some(token)
        } else {
            None
        }
    }

    fn expect(&mut self, kind: &TokenKind, label: &str) -> Result<Token, Diagnostic> {
        self.eat(kind)
            .ok_or_else(|| Diagnostic::new(self.current().span, format!("expected {label}")))
    }

    fn name(&mut self) -> Result<(String, Span), Diagnostic> {
        if let TokenKind::Ident(name) = self.current().kind.clone() {
            let span = self.current().span;
            self.pos += 1;
            Ok((name, span))
        } else {
            Err(Diagnostic::new(self.current().span, "expected name"))
        }
    }

    fn call_arguments(&mut self) -> Result<(Vec<CallArgument>, Span), Diagnostic> {
        let mut args = Vec::new();
        let end = if let Some(close) = self.eat(&TokenKind::RParen) {
            close.span
        } else {
            loop {
                if let Some(amp) = self.eat(&TokenKind::Amp) {
                    let mutable = self.eat(&TokenKind::Mut).is_some();
                    let (name, name_span) = self.name()?;
                    args.push(CallArgument::Borrow {
                        name,
                        span: amp.span.join(name_span),
                        mutable,
                    });
                } else {
                    args.push(CallArgument::Value(self.expr(0)?));
                }
                if self.eat(&TokenKind::Comma).is_none() {
                    break self.expect(&TokenKind::RParen, ")")?.span;
                }
            }
        };
        Ok((args, end))
    }
}

pub fn parse(source: &str) -> Result<Program, Diagnostic> {
    let mut parser = Parser {
        tokens: lex(source)?,
        pos: 0,
        depth: 0,
        applied_types: Vec::new(),
        uses_send_outcome: false,
        uses_receive_outcome: false,
        uses_complete_outcome: false,
        uses_reply_outcome: false,
    };
    let mut functions = Vec::new();
    let mut data = Vec::new();
    let mut imports = Vec::new();
    while parser.current().kind != TokenKind::Eof {
        let public = parser.eat(&TokenKind::Pub).is_some();
        if !public && parser.current().kind == TokenKind::Import {
            parser.pos += 1;
            let (name, span) = parser.name()?;
            imports.push(Import { name, span });
        } else if parser.current().kind == TokenKind::Data {
            data.push(parser.data(public)?);
        } else {
            functions.push(parser.function(public)?);
        }
    }
    if functions.is_empty() && data.is_empty() {
        return Err(Diagnostic::new(
            Span { start: 0, end: 0 },
            "expected a function",
        ));
    }
    Ok(Program {
        imports,
        data,
        functions,
        applied_types: parser.applied_types,
        uses_send_outcome: parser.uses_send_outcome,
        uses_receive_outcome: parser.uses_receive_outcome,
        uses_complete_outcome: parser.uses_complete_outcome,
        uses_reply_outcome: parser.uses_reply_outcome,
    })
}
