use crate::{Diagnostic, Span};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TokenKind {
    Fn,
    Pub,
    Import,
    Data,
    When,
    Val,
    Var,
    Mut,
    Copy,
    While,
    Spawn,
    Scope,
    With,
    Yield,
    If,
    Else,
    True,
    False,
    Ident(String),
    Int(u64),
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Dot,
    Colon,
    Semicolon,
    Arrow,
    Equal,
    EqualEqual,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Plus,
    Minus,
    Star,
    Amp,
    AndAnd,
    OrOr,
    Bang,
    Question,
    Eof,
}

#[derive(Clone, Debug)]
pub(super) struct Token {
    pub(super) kind: TokenKind,
    pub(super) span: Span,
}

pub(super) fn lex(source: &str) -> Result<Vec<Token>, Diagnostic> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let kind = match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' => {
                i += 1;
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                match &source[start..i] {
                    "fn" => TokenKind::Fn,
                    "pub" => TokenKind::Pub,
                    "import" => TokenKind::Import,
                    "data" => TokenKind::Data,
                    "when" => TokenKind::When,
                    "val" => TokenKind::Val,
                    "var" => TokenKind::Var,
                    "mut" => TokenKind::Mut,
                    "copy" => TokenKind::Copy,
                    "while" => TokenKind::While,
                    "spawn" => TokenKind::Spawn,
                    "scope" => TokenKind::Scope,
                    "with" => TokenKind::With,
                    "yield" => TokenKind::Yield,
                    "if" => TokenKind::If,
                    "else" => TokenKind::Else,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    word => TokenKind::Ident(word.to_owned()),
                }
            }
            b'0'..=b'9' => {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let value = source[start..i].parse().map_err(|_| {
                    Diagnostic::new(Span { start, end: i }, "integer literal exceeds I64")
                })?;
                TokenKind::Int(value)
            }
            b'(' => {
                i += 1;
                TokenKind::LParen
            }
            b')' => {
                i += 1;
                TokenKind::RParen
            }
            b'[' => {
                i += 1;
                TokenKind::LBracket
            }
            b']' => {
                i += 1;
                TokenKind::RBracket
            }
            b'{' => {
                i += 1;
                TokenKind::LBrace
            }
            b'}' => {
                i += 1;
                TokenKind::RBrace
            }
            b',' => {
                i += 1;
                TokenKind::Comma
            }
            b'.' => {
                i += 1;
                TokenKind::Dot
            }
            b':' => {
                i += 1;
                TokenKind::Colon
            }
            b';' => {
                i += 1;
                TokenKind::Semicolon
            }
            b'=' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::EqualEqual
            }
            b'=' => {
                i += 1;
                TokenKind::Equal
            }
            b'!' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::BangEqual
            }
            b'<' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::LessEqual
            }
            b'<' => {
                i += 1;
                TokenKind::Less
            }
            b'>' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::GreaterEqual
            }
            b'>' => {
                i += 1;
                TokenKind::Greater
            }
            b'+' => {
                i += 1;
                TokenKind::Plus
            }
            b'*' => {
                i += 1;
                TokenKind::Star
            }
            b'&' if bytes.get(i + 1) == Some(&b'&') => {
                i += 2;
                TokenKind::AndAnd
            }
            b'&' => {
                i += 1;
                TokenKind::Amp
            }
            b'|' if bytes.get(i + 1) == Some(&b'|') => {
                i += 2;
                TokenKind::OrOr
            }
            b'!' => {
                i += 1;
                TokenKind::Bang
            }
            b'?' => {
                i += 1;
                TokenKind::Question
            }
            b'-' if bytes.get(i + 1) == Some(&b'>') => {
                i += 2;
                TokenKind::Arrow
            }
            b'-' => {
                i += 1;
                TokenKind::Minus
            }
            _ => {
                let end = start + source[start..].chars().next().unwrap().len_utf8();
                return Err(Diagnostic::new(
                    Span { start, end },
                    "unrecognized character",
                ));
            }
        };
        tokens.push(Token {
            kind,
            span: Span { start, end: i },
        });
    }
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span { start: i, end: i },
    });
    Ok(tokens)
}
