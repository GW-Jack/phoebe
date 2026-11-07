pub mod syntax;
pub use syntax::*;

mod equality;
pub use equality::same_function_with_shift;

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn join(self, other: Self) -> Self {
        Self {
            start: self.start,
            end: other.end,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
}

impl Diagnostic {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }

    pub fn render(&self, source: &str) -> String {
        let start = self.span.start.min(source.len());
        let line = source[..start].bytes().filter(|b| *b == b'\n').count() + 1;
        let column = source[..start]
            .rfind('\n')
            .map_or(start + 1, |last| start - last);
        format!("{line}:{column}: {}", self.message)
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}..{}: {}",
            self.span.start, self.span.end, self.message
        )
    }
}

impl std::error::Error for Diagnostic {}
