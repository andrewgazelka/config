//! Tokens. Newlines are insignificant; `//` starts a comment.

use super::{Diag, Span};

#[derive(Clone, Debug, PartialEq)]
pub enum RawPart {
    Lit(String),
    /// Byte range of an interpolated expression, parsed later.
    Interp {
        lo: u32,
        hi: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Float(f64),
    Str(Vec<RawPart>),
    Punct(&'static str),
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

const PUNCT: &[&str] = &[
    "...", "..=", "??", "==", "!=", "<=", ">=", "&&", "||", "..", "{", "}", "[", "]", "(", ")", ":", ";", ",", ".",
    "=", "<", ">", "+", "-", "*", "/", "!", "|", "?",
];

pub struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    end: usize,
    file: u32,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str, file: u32) -> Self {
        Lexer { src, pos: 0, end: src.len(), file }
    }

    /// Lex `src[lo..hi]` (an interpolation) with spans in file coordinates.
    pub fn range(src: &'a str, file: u32, lo: usize, hi: usize) -> Self {
        Lexer { src, pos: lo, end: hi, file }
    }

    pub fn tokens(mut self) -> Result<Vec<Token>, Diag> {
        let mut out = Vec::new();
        loop {
            let t = self.next()?;
            let eof = t.tok == Tok::Eof;
            out.push(t);
            if eof {
                return Ok(out);
            }
        }
    }

    fn span(&self, lo: usize) -> Span {
        Span { file: self.file, lo: lo as u32, hi: self.pos as u32 }
    }

    fn err(&self, lo: usize, msg: impl Into<String>) -> Diag {
        Diag::new(Span { file: self.file, lo: lo as u32, hi: (lo + 1).min(self.end) as u32 }, msg)
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..self.end].chars().next()
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..self.end]
    }

    fn skip_trivia(&mut self) {
        loop {
            let rest = self.rest();
            let trimmed = rest.trim_start();
            self.pos += rest.len() - trimmed.len();
            if self.rest().starts_with("//") {
                self.pos += self.rest().find('\n').unwrap_or(self.rest().len());
            } else {
                return;
            }
        }
    }

    pub fn next(&mut self) -> Result<Token, Diag> {
        self.skip_trivia();
        let lo = self.pos;
        let Some(c) = self.peek() else {
            return Ok(Token { tok: Tok::Eof, span: self.span(lo) });
        };
        let rest = self.rest();
        let tok = if rest.starts_with("r\"") || rest.starts_with("r#") {
            self.pos += 1;
            self.raw_string(lo)?
        } else if c.is_alphabetic() || c == '_' {
            let len = rest
                .char_indices()
                .find(|&(_, c)| !(c.is_alphanumeric() || c == '_' || c == '-'))
                .map_or(rest.len(), |(i, _)| i);
            // A trailing `-` belongs to the next token (`a-` is never a name).
            let word = rest[..len].trim_end_matches('-');
            self.pos += word.len();
            Tok::Ident(word.to_string())
        } else if c.is_ascii_digit() {
            self.number(lo)?
        } else if c == '"' {
            if rest.starts_with("\"\"\"") {
                self.block_string(lo, false)?
            } else {
                self.pos += 1;
                Tok::Str(self.string_body(lo)?)
            }
        } else if let Some(p) = PUNCT.iter().find(|p| rest.starts_with(**p)) {
            self.pos += p.len();
            Tok::Punct(p)
        } else {
            return Err(self.err(lo, format!("unexpected character `{c}`")));
        };
        Ok(Token { tok, span: self.span(lo) })
    }

    fn number(&mut self, lo: usize) -> Result<Tok, Diag> {
        let rest = self.rest();
        let int_len = rest.find(|c: char| !c.is_ascii_digit() && c != '_').unwrap_or(rest.len());
        let mut len = int_len;
        // `1..3` is a range, `1.5` a float.
        let is_float =
            rest[int_len..].starts_with('.') && rest[int_len + 1..].starts_with(|c: char| c.is_ascii_digit());
        if is_float {
            len += 1 + rest[int_len + 1..]
                .find(|c: char| !c.is_ascii_digit() && c != '_')
                .unwrap_or(rest.len() - int_len - 1);
        }
        let text: String = rest[..len].chars().filter(|&c| c != '_').collect();
        self.pos += len;
        if is_float {
            text.parse().map(Tok::Float).map_err(|_| self.err(lo, "bad number"))
        } else {
            text.parse().map(Tok::Int).map_err(|_| self.err(lo, "integer out of range"))
        }
    }

    /// After the opening quote: escapes and `${...}`, up to the closing quote.
    fn string_body(&mut self, lo: usize) -> Result<Vec<RawPart>, Diag> {
        let mut parts = Vec::new();
        let mut lit = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.err(lo, "unterminated string"));
            };
            match c {
                '"' => {
                    self.pos += 1;
                    break;
                }
                '\n' => return Err(self.err(lo, "newline in string; use a \"\"\" block string")),
                '\\' => lit.push(self.escape()?),
                '$' if self.rest().starts_with("${") => {
                    if !lit.is_empty() {
                        parts.push(RawPart::Lit(std::mem::take(&mut lit)));
                    }
                    parts.push(self.interp()?);
                }
                c => {
                    lit.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
        if !lit.is_empty() || parts.is_empty() {
            parts.push(RawPart::Lit(lit));
        }
        Ok(parts)
    }

    fn escape(&mut self) -> Result<char, Diag> {
        let lo = self.pos;
        self.pos += 1;
        let Some(c) = self.peek() else {
            return Err(self.err(lo, "unterminated escape"));
        };
        self.pos += c.len_utf8();
        Ok(match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '0' => '\0',
            '\\' | '"' | '$' => c,
            'u' if self.rest().starts_with('{') => {
                let close = self.rest().find('}').ok_or_else(|| self.err(lo, "bad \\u{...}"))?;
                let hex = &self.rest()[1..close];
                self.pos += close + 1;
                u32::from_str_radix(hex, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| self.err(lo, "bad \\u{...}"))?
            }
            _ => return Err(self.err(lo, format!("unknown escape `\\{c}`"))),
        })
    }

    /// At `${`: find the matching `}` by lexing, so nested strings work.
    fn interp(&mut self) -> Result<RawPart, Diag> {
        let start = self.pos;
        self.pos += 2;
        let lo = self.pos;
        let mut depth = 0;
        loop {
            let t = self.next()?;
            match t.tok {
                Tok::Punct("{") => depth += 1,
                Tok::Punct("}") if depth == 0 => {
                    return Ok(RawPart::Interp { lo: lo as u32, hi: t.span.lo });
                }
                Tok::Punct("}") => depth -= 1,
                Tok::Eof => return Err(self.err(start, "unterminated `${`")),
                _ => {}
            }
        }
    }

    /// `r"..."`, `r#"..."#`, `r"""` block: no escapes, no interpolation.
    fn raw_string(&mut self, lo: usize) -> Result<Tok, Diag> {
        let hashes = self.rest().len() - self.rest().trim_start_matches('#').len();
        self.pos += hashes;
        if hashes == 0 && self.rest().starts_with("\"\"\"") {
            return self.block_string(lo, true);
        }
        if !self.rest().starts_with('"') {
            return Err(self.err(lo, "expected `\"` after `r`"));
        }
        self.pos += 1;
        let close = format!("\"{}", "#".repeat(hashes));
        let end = self.rest().find(&close).ok_or_else(|| self.err(lo, "unterminated raw string"))?;
        let text = self.rest()[..end].to_string();
        self.pos += end + close.len();
        Ok(Tok::Str(vec![RawPart::Lit(text)]))
    }

    /// `"""` newline ... newline (indent) `"""`. The closing line's indent is
    /// removed from every line; the last newline is not part of the text.
    fn block_string(&mut self, lo: usize, raw: bool) -> Result<Tok, Diag> {
        self.pos += 3;
        let rest = self.rest();
        let first_nl = rest.find('\n').ok_or_else(|| self.err(lo, "block string must start a new line"))?;
        if !rest[..first_nl].trim().is_empty() {
            return Err(self.err(lo, "text after opening \"\"\""));
        }
        let body_start = self.pos + first_nl + 1;
        // Find the closing line: optional whitespace then `"""`.
        let mut line_start = body_start;
        let (close_line, indent) = loop {
            if line_start > self.end {
                return Err(self.err(lo, "unterminated block string"));
            }
            let line_end = self.src[line_start..self.end].find('\n').map_or(self.end, |i| line_start + i);
            let line = &self.src[line_start..line_end];
            let trimmed = line.trim_start_matches([' ', '\t']);
            if trimmed.starts_with("\"\"\"") {
                break (line_start, line.len() - trimmed.len());
            }
            if line_end == self.end {
                return Err(self.err(lo, "unterminated block string"));
            }
            line_start = line_end + 1;
        };
        let mut parts = Vec::new();
        let mut lit = String::new();
        self.pos = body_start;
        let mut at_line_start = true;
        while self.pos < close_line {
            if at_line_start {
                let line = &self.src[self.pos..close_line];
                let ws = line.len() - line.trim_start_matches([' ', '\t']).len();
                let blank = line[ws..].starts_with('\n');
                if !blank && ws < indent {
                    return Err(self.err(self.pos, "line is indented less than the closing \"\"\""));
                }
                self.pos += ws.min(indent);
                at_line_start = false;
                continue;
            }
            let c = self.peek().unwrap();
            match c {
                '\n' => {
                    // The newline before the closing line is not content.
                    if self.pos + 1 < close_line {
                        lit.push('\n');
                    }
                    self.pos += 1;
                    at_line_start = true;
                }
                '\\' if !raw => lit.push(self.escape()?),
                '$' if !raw && self.rest().starts_with("${") => {
                    if !lit.is_empty() {
                        parts.push(RawPart::Lit(std::mem::take(&mut lit)));
                    }
                    parts.push(self.interp()?);
                }
                c => {
                    lit.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
        self.pos = close_line + indent + 3;
        if !lit.is_empty() || parts.is_empty() {
            parts.push(RawPart::Lit(lit));
        }
        Ok(Tok::Str(parts))
    }
}
