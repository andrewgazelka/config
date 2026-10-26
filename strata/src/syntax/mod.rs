pub mod ast;
pub mod lexer;
pub mod parser;

use std::fmt;

/// A byte range in one source file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    pub file: u32,
    pub lo: u32,
    pub hi: u32,
}

/// One diagnostic: a message at a span, with optional related spans.
#[derive(Clone, Debug, PartialEq)]
pub struct Diag {
    pub span: Span,
    pub message: String,
    pub notes: Vec<(Span, String)>,
}

impl Diag {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Diag { span, message: message.into(), notes: Vec::new() }
    }

    pub fn note(mut self, span: Span, message: impl Into<String>) -> Self {
        self.notes.push((span, message.into()));
        self
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Maps spans to `path:line:col` and source lines.
pub trait Sources {
    fn path(&self, file: u32) -> String;
    fn text(&self, file: u32) -> Option<&str>;

    fn line_col(&self, span: Span) -> (usize, usize) {
        let Some(text) = self.text(span.file) else { return (0, 0) };
        let lo = (span.lo as usize).min(text.len());
        let before = &text[..lo];
        let line = before.matches('\n').count() + 1;
        let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
        (line, col)
    }

    fn location(&self, span: Span) -> String {
        let (line, col) = self.line_col(span);
        format!("{}:{line}:{col}", self.path(span.file))
    }

    fn render(&self, diag: &Diag) -> String {
        let mut out = format!("error: {}\n", diag.message);
        let show = |span: Span, label: &str, out: &mut String| {
            let (line, _) = self.line_col(span);
            out.push_str(&format!("  --> {}{label}\n", self.location(span)));
            if let Some(text) = self.text(span.file)
                && let Some(src) = text.lines().nth(line.wrapping_sub(1))
            {
                out.push_str(&format!("   | {}\n", src.trim_end()));
            }
        };
        show(diag.span, "", &mut out);
        for (span, note) in &diag.notes {
            show(*span, &format!(": {note}"), &mut out);
        }
        out
    }
}
