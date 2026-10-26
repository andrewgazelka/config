//! Recursive descent over the token stream.

use super::ast::*;
use super::lexer::{Lexer, RawPart, Tok, Token};
use super::{Diag, Span};
use crate::merge::Level;

pub fn parse(src: &str, file: u32) -> Result<Vec<Stmt>, Diag> {
    let tokens = Lexer::new(src, file).tokens()?;
    let mut p = Parser { src, file, tokens, pos: 0 };
    let mut items = Vec::new();
    while !p.at_eof() {
        items.push(p.stmt(true)?);
        p.separators();
    }
    Ok(items)
}

struct Parser<'a> {
    src: &'a str,
    file: u32,
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.tokens[(self.pos + n).min(self.tokens.len() - 1)].tok
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn prev_hi(&self) -> u32 {
        self.tokens[self.pos.saturating_sub(1)].span.hi
    }

    fn since(&self, lo: Span) -> Span {
        Span { file: self.file, lo: lo.lo, hi: self.prev_hi().max(lo.lo) }
    }

    fn bump(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn at_eof(&self) -> bool {
        *self.peek() == Tok::Eof
    }

    fn is(&self, p: &str) -> bool {
        matches!(self.peek(), Tok::Punct(q) if *q == p)
    }

    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Tok::Ident(i) if i == w)
    }

    fn eat(&mut self, p: &str) -> bool {
        if self.is(p) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, p: &str) -> Result<Span, Diag> {
        if self.is(p) { Ok(self.bump().span) } else { Err(self.unexpected(&format!("`{p}`"))) }
    }

    fn unexpected(&self, wanted: &str) -> Diag {
        let found = match self.peek() {
            Tok::Ident(i) => format!("`{i}`"),
            Tok::Int(i) => format!("`{i}`"),
            Tok::Float(f) => format!("`{f}`"),
            Tok::Str(_) => "a string".into(),
            Tok::Punct(p) => format!("`{p}`"),
            Tok::Eof => "end of file".into(),
        };
        Diag::new(self.span(), format!("expected {wanted}, found {found}"))
    }

    fn separators(&mut self) {
        while self.eat(";") || self.eat(",") {}
    }

    fn ident(&mut self) -> Result<Ident, Diag> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                let span = self.bump().span;
                Ok(Ident { name: name.into(), span })
            }
            _ => Err(self.unexpected("a name")),
        }
    }

    fn name(&mut self) -> Result<Name, Diag> {
        match self.peek().clone() {
            Tok::Ident(_) => Ok(Name::Word(self.ident()?)),
            Tok::Str(parts) => {
                let span = self.bump().span;
                Ok(Name::Str(self.str_parts(parts)?, span))
            }
            _ => Err(self.unexpected("a name")),
        }
    }

    fn str_parts(&self, raw: Vec<RawPart>) -> Result<Vec<StrPart>, Diag> {
        raw.into_iter()
            .map(|p| match p {
                RawPart::Lit(s) => Ok(StrPart::Lit(s)),
                RawPart::Interp { lo, hi } => {
                    let tokens = Lexer::range(self.src, self.file, lo as usize, hi as usize).tokens()?;
                    let mut sub = Parser { src: self.src, file: self.file, tokens, pos: 0 };
                    let e = sub.expr()?;
                    if !sub.at_eof() {
                        return Err(sub.unexpected("`}`"));
                    }
                    Ok(StrPart::Interp(e))
                }
            })
            .collect()
    }

    fn block<T>(&mut self, mut item: impl FnMut(&mut Self) -> Result<T, Diag>) -> Result<Vec<T>, Diag> {
        self.expect("{")?;
        let mut items = Vec::new();
        self.separators();
        while !self.is("}") {
            if self.at_eof() {
                return Err(self.unexpected("`}`"));
            }
            items.push(item(self)?);
            self.separators();
        }
        self.bump();
        Ok(items)
    }

    fn pattern(&mut self) -> Result<Pattern, Diag> {
        let first = self.ident()?;
        let second = if self.eat(",") { Some(self.ident()?) } else { None };
        Ok(Pattern { first, second })
    }

    fn dotted(&mut self) -> Result<Vec<Ident>, Diag> {
        let mut path = vec![self.ident()?];
        while self.is(".") {
            self.bump();
            path.push(self.ident()?);
        }
        Ok(path)
    }

    fn bases(&mut self) -> Result<Vec<Vec<Ident>>, Diag> {
        let mut bases = vec![self.dotted()?];
        while self.eat(",") {
            bases.push(self.dotted()?);
        }
        Ok(bases)
    }

    fn stmt(&mut self, top: bool) -> Result<Stmt, Diag> {
        let lo = self.span();
        let keyword = match self.peek() {
            Tok::Ident(w) => Some(w.clone()),
            _ => None,
        };
        let next_is_colon = matches!(self.peek_at(1), Tok::Punct(":"));
        match keyword.as_deref() {
            Some("for") if !next_is_colon => {
                self.bump();
                let pattern = self.pattern()?;
                self.expect_word("in")?;
                let iter = self.expr()?;
                let body = self.block(|p| p.stmt(false))?;
                return Ok(Stmt::For { pattern, iter, body, span: self.since(lo) });
            }
            Some("if") if !next_is_colon => {
                self.bump();
                let cond = self.expr()?;
                let then = self.block(|p| p.stmt(false))?;
                let otherwise = if self.is_word("else") {
                    self.bump();
                    if self.is_word("if") { vec![self.stmt(false)?] } else { self.block(|p| p.stmt(false))? }
                } else {
                    vec![]
                };
                return Ok(Stmt::If { cond, then, otherwise, span: self.since(lo) });
            }
            Some("const") if !next_is_colon => {
                self.bump();
                let name = self.ident()?;
                self.expect("=")?;
                let value = self.expr()?;
                return Ok(Stmt::Const { name, value, span: self.since(lo) });
            }
            Some(kw @ ("prefab" | "component" | "type" | "facts" | "rule")) if !next_is_colon => {
                if !top {
                    return Err(Diag::new(lo, format!("`{kw}` is only allowed at the top level of a file")));
                }
                self.bump();
                return self.declaration(kw, lo);
            }
            _ => {}
        }
        // Braces open a scope, a colon sets one value: `name { ... }` is an
        // entity, `name: value` a component, and `name: base, base { ... }`
        // an entity built from prefabs. `name: { ... }` would be a second way
        // to write a nested object, so it is refused.
        if matches!(self.peek(), Tok::Ident(_) | Tok::Str(_)) && next_is_colon {
            let name = self.name()?;
            self.bump();
            let priority = self.is_word("default") || self.is_word("override");
            if self.is("{") || (priority && matches!(self.peek_at(1), Tok::Punct("{"))) {
                let span = name.span();
                let shown = &self.src[span.lo as usize..span.hi as usize];
                return Err(Diag::new(span, format!("write `{shown} {{ ... }}`: a nested object is a scope")));
            }
            if self.prefab_list_ahead() {
                let bases = self.bases()?;
                let body = self.block(|p| p.stmt(false))?;
                return Ok(Stmt::Entity { path: vec![name], bases, body, span: self.since(lo) });
            }
            let value = self.expr()?;
            return Ok(Stmt::Component { name, value: Some(value), span: self.since(lo) });
        }
        let first = self.name()?;
        if let Name::Word(_) = &first
            && !self.is(".")
            && !self.is("{")
            && !self.is(":")
        {
            return Ok(Stmt::Component { name: first, value: None, span: self.since(lo) });
        }
        let mut path = vec![first];
        while self.eat(".") {
            path.push(self.name()?);
        }
        let bases = if self.eat(":") { self.bases()? } else { vec![] };
        let body = self.block(|p| p.stmt(false))?;
        Ok(Stmt::Entity { path, bases, body, span: self.since(lo) })
    }

    /// After `name:`, whether `base, base {` follows (an entity built from
    /// prefabs) rather than a component value. A value never has that shape
    /// except `default {..}` / `override {..}` and `if c {..}`, which are
    /// keywords, not prefab names.
    fn prefab_list_ahead(&self) -> bool {
        let mut i = 0;
        loop {
            match self.peek_at(i) {
                Tok::Ident(w) if !matches!(w.as_str(), "default" | "override" | "if" | "null" | "true" | "false") => {}
                _ => return false,
            }
            i += 1;
            match self.peek_at(i) {
                Tok::Punct("." | ",") => i += 1,
                Tok::Punct("{") => return true,
                _ => return false,
            }
        }
    }

    fn expect_word(&mut self, w: &str) -> Result<(), Diag> {
        if self.is_word(w) {
            self.bump();
            Ok(())
        } else {
            Err(self.unexpected(&format!("`{w}`")))
        }
    }

    fn string_lit(&mut self) -> Result<String, Diag> {
        match self.peek().clone() {
            Tok::Str(parts) => match parts.as_slice() {
                [RawPart::Lit(s)] => {
                    self.bump();
                    Ok(s.clone())
                }
                _ => Err(Diag::new(self.span(), "expected a plain string here")),
            },
            _ => Err(self.unexpected("a string")),
        }
    }

    fn declaration(&mut self, kw: &str, lo: Span) -> Result<Stmt, Diag> {
        match kw {
            "prefab" => {
                let name = self.ident()?;
                let bases = if self.eat(":") { self.bases()? } else { vec![] };
                let body = self.block(|p| p.stmt(false))?;
                Ok(Stmt::Prefab { name, bases, body, span: self.since(lo) })
            }
            "component" => {
                let name = self.ident()?;
                self.expect(":")?;
                let ty = self.ty()?;
                Ok(Stmt::ComponentDecl { name, ty, span: self.since(lo) })
            }
            "type" => {
                let name = self.ident()?;
                self.expect("=")?;
                let ty = self.ty()?;
                Ok(Stmt::TypeDecl { name, ty, span: self.since(lo) })
            }
            "facts" => {
                let name = self.ident()?;
                self.expect(":")?;
                let component = self.ident()?;
                self.expect_word("from")?;
                let source = if self.is_word("nix") {
                    self.bump();
                    let expr = self.string_lit()?;
                    self.expect_word("lock")?;
                    let lock = self.string_lit()?;
                    FactSource::Nix { expr, lock }
                } else {
                    FactSource::Json(self.string_lit()?)
                };
                Ok(Stmt::Facts { name, component, source, span: self.since(lo) })
            }
            "rule" => {
                let name = self.ident()?;
                let body = self.block(|p| p.stmt(false))?;
                Ok(Stmt::Rule { name, body, span: self.since(lo) })
            }
            _ => unreachable!(),
        }
    }

    fn ty(&mut self) -> Result<TypeExpr, Diag> {
        let mut terms = vec![self.ty_term()?];
        while self.eat("|") {
            terms.push(self.ty_term()?);
        }
        Ok(if terms.len() == 1 { terms.pop().unwrap() } else { TypeExpr::Union(terms) })
    }

    fn ty_term(&mut self) -> Result<TypeExpr, Diag> {
        let lo = self.span();
        match self.peek().clone() {
            Tok::Str(_) => Ok(TypeExpr::Literal(self.string_lit()?)),
            Tok::Punct("(") => {
                self.bump();
                let t = self.ty()?;
                self.expect(")")?;
                Ok(t)
            }
            Tok::Punct("{") => {
                self.bump();
                let mut fields = Vec::new();
                let mut rest = None;
                self.separators();
                while !self.eat("}") {
                    if self.eat("..") {
                        // `..` admits any other key; `..: T` admits others of type T.
                        rest = Some(Box::new(if self.eat(":") {
                            self.ty()?
                        } else {
                            TypeExpr::Named(Ident { name: "any".into(), span: self.since(lo) })
                        }));
                    } else {
                        let key = match self.name()? {
                            Name::Word(i) => i.name.to_string(),
                            Name::Str(parts, span) => match parts.as_slice() {
                                [StrPart::Lit(s)] => s.clone(),
                                _ => return Err(Diag::new(span, "type field names are plain")),
                            },
                        };
                        let optional = self.eat("?");
                        self.expect(":")?;
                        fields.push((key, self.ty()?, optional));
                    }
                    self.separators();
                }
                Ok(TypeExpr::Record { fields, rest })
            }
            Tok::Ident(w)
                if matches!(w.as_str(), "list" | "set" | "map") && matches!(self.peek_at(1), Tok::Punct("(")) =>
            {
                self.bump();
                self.bump();
                let inner = Box::new(self.ty()?);
                self.expect(")")?;
                Ok(match w.as_str() {
                    "list" => TypeExpr::List(inner),
                    "set" => TypeExpr::Set(inner),
                    _ => TypeExpr::Map(inner),
                })
            }
            Tok::Ident(_) => Ok(TypeExpr::Named(self.ident()?)),
            _ => Err(self.unexpected("a type")),
        }
    }

    pub fn expr(&mut self) -> Result<Expr, Diag> {
        let lo = self.span();
        let level = match self.peek() {
            Tok::Ident(w) if w == "default" => Some(Level::Default),
            Tok::Ident(w) if w == "override" => Some(Level::Override),
            _ => None,
        };
        if let Some(level) = level
            && !matches!(self.peek_at(1), Tok::Punct(":" | "." | "[" | ")" | "," | "}" | "]" | ";"))
        {
            self.bump();
            let inner = self.expr()?;
            return Ok(Expr { kind: ExprKind::Priority(level, Box::new(inner)), span: self.since(lo) });
        }
        self.binary(0)
    }

    fn binary(&mut self, min: u8) -> Result<Expr, Diag> {
        let lo = self.span();
        let mut lhs = self.unary()?;
        while let Tok::Punct(p) = self.peek() {
            let (prec, op) = match *p {
                "??" => (1, BinOp::Else),
                "||" => (2, BinOp::Or),
                "&&" => (3, BinOp::And),
                "==" => (4, BinOp::Eq),
                "!=" => (4, BinOp::Ne),
                "<" => (4, BinOp::Lt),
                "<=" => (4, BinOp::Le),
                ">" => (4, BinOp::Gt),
                ">=" => (4, BinOp::Ge),
                ".." | "..=" => (5, BinOp::Add),
                "+" => (6, BinOp::Add),
                "-" => (6, BinOp::Sub),
                "*" => (7, BinOp::Mul),
                "/" => (7, BinOp::Div),
                _ => break,
            };
            if prec < min {
                break;
            }
            let tok = self.bump();
            let rhs = self.binary(prec + 1)?;
            let kind = match tok.tok {
                Tok::Punct("..") => ExprKind::Range(Box::new(lhs), Box::new(rhs), false),
                Tok::Punct("..=") => ExprKind::Range(Box::new(lhs), Box::new(rhs), true),
                _ => ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
            };
            lhs = Expr { kind, span: self.since(lo) };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, Diag> {
        let lo = self.span();
        let op = if self.eat("-") {
            Some(UnOp::Neg)
        } else if self.eat("!") {
            Some(UnOp::Not)
        } else {
            None
        };
        match op {
            Some(op) => {
                let inner = self.unary()?;
                Ok(Expr { kind: ExprKind::Unary(op, Box::new(inner)), span: self.since(lo) })
            }
            None => self.postfix(),
        }
    }

    fn postfix(&mut self) -> Result<Expr, Diag> {
        let lo = self.span();
        let mut e = self.primary()?;
        loop {
            if self.is(".") {
                self.bump();
                if self.eat("*") {
                    e = Expr { kind: ExprKind::Children(Box::new(e)), span: self.since(lo) };
                    continue;
                }
                let name = self.name()?;
                e = match e.kind {
                    ExprKind::Path(mut segs) => {
                        segs.push(name);
                        Expr { kind: ExprKind::Path(segs), span: self.since(lo) }
                    }
                    kind => Expr {
                        kind: ExprKind::Field(Box::new(Expr { kind, span: e.span }), name),
                        span: self.since(lo),
                    },
                };
            } else if self.is("[")
                && matches!(self.peek_at(1), Tok::Ident(_))
                && matches!(self.peek_at(2), Tok::Punct("]"))
            {
                self.bump();
                let comp = self.ident()?;
                self.bump();
                e = Expr { kind: ExprKind::Component(Box::new(e), comp), span: self.since(lo) };
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, Diag> {
        let lo = self.span();
        let kind = match self.peek().clone() {
            Tok::Int(i) => {
                self.bump();
                ExprKind::Int(i)
            }
            Tok::Float(f) => {
                self.bump();
                ExprKind::Float(f)
            }
            Tok::Str(parts) => {
                self.bump();
                ExprKind::Str(self.str_parts(parts)?)
            }
            Tok::Punct("(") => {
                self.bump();
                let e = self.expr()?;
                self.expect(")")?;
                return Ok(e);
            }
            Tok::Punct("{") => ExprKind::Record(self.block(|p| p.field())?),
            Tok::Punct("[") => {
                self.bump();
                let mut elems = Vec::new();
                self.separators();
                while !self.eat("]") {
                    if self.at_eof() {
                        return Err(self.unexpected("`]`"));
                    }
                    elems.push(self.elem()?);
                    self.separators();
                }
                ExprKind::List(elems)
            }
            Tok::Ident(w) => match w.as_str() {
                "null" => {
                    self.bump();
                    ExprKind::Null
                }
                "true" | "false" => {
                    self.bump();
                    ExprKind::Bool(w == "true")
                }
                "if" => {
                    self.bump();
                    let cond = self.expr()?;
                    self.expect("{")?;
                    let then = self.expr()?;
                    self.expect("}")?;
                    self.expect_word("else")?;
                    let otherwise = if self.is_word("if") {
                        self.primary()?
                    } else {
                        self.expect("{")?;
                        let e = self.expr()?;
                        self.expect("}")?;
                        e
                    };
                    ExprKind::If(Box::new(cond), Box::new(then), Box::new(otherwise))
                }
                _ if matches!(self.peek_at(1), Tok::Punct("(")) => {
                    let name = self.ident()?;
                    self.bump();
                    let mut args = Vec::new();
                    self.separators();
                    while !self.eat(")") {
                        args.push(self.expr()?);
                        self.separators();
                    }
                    ExprKind::Call(name, args)
                }
                _ => ExprKind::Path(vec![Name::Word(self.ident()?)]),
            },
            _ => return Err(self.unexpected("an expression")),
        };
        Ok(Expr { kind, span: self.since(lo) })
    }

    fn keyword_here(&self, w: &str) -> bool {
        self.is_word(w) && !matches!(self.peek_at(1), Tok::Punct(":"))
    }

    fn field(&mut self) -> Result<Field, Diag> {
        if self.eat("...") {
            return Ok(Field::Spread(self.expr()?));
        }
        if self.keyword_here("for") {
            self.bump();
            let pattern = self.pattern()?;
            self.expect_word("in")?;
            let iter = self.expr()?;
            let body = self.block(|p| p.field())?;
            return Ok(Field::For { pattern, iter, body });
        }
        if self.keyword_here("if") {
            self.bump();
            let cond = self.expr()?;
            let then = self.block(|p| p.field())?;
            let otherwise = if self.is_word("else") {
                self.bump();
                if self.is_word("if") { vec![self.field()?] } else { self.block(|p| p.field())? }
            } else {
                vec![]
            };
            return Ok(Field::If { cond, then, otherwise });
        }
        let key = self.name()?;
        self.expect(":")?;
        Ok(Field::Pair { key, value: self.expr()? })
    }

    fn elem(&mut self) -> Result<Elem, Diag> {
        if self.eat("...") {
            return Ok(Elem::Spread(self.expr()?));
        }
        if self.is_word("for") {
            self.bump();
            let pattern = self.pattern()?;
            self.expect_word("in")?;
            let iter = self.expr()?;
            let body = self.block(|p| p.elem())?;
            return Ok(Elem::For { pattern, iter, body });
        }
        if self.is_word("if") {
            self.bump();
            let cond = self.expr()?;
            let then = self.block(|p| p.elem())?;
            let otherwise = if self.is_word("else") {
                self.bump();
                if self.is_word("if") { vec![self.elem()?] } else { self.block(|p| p.elem())? }
            } else {
                vec![]
            };
            return Ok(Elem::If { cond, then, otherwise });
        }
        Ok(Elem::Item(self.expr()?))
    }
}

/// Parse an address like `zed[tab_size]` or `files."~/.x"[file]`.
pub fn parse_address(src: &str) -> Result<Expr, Diag> {
    let tokens = Lexer::new(src, u32::MAX).tokens()?;
    let mut p = Parser { src, file: u32::MAX, tokens, pos: 0 };
    let e = p.expr()?;
    if !p.at_eof() {
        return Err(p.unexpected("end of address"));
    }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_component_prefab_and_tag() {
        let src = r#"
            zed : typography { font { size: default 12 }; "unstable.x": 1; keymap { editor { "cmd--" { unbind } } } }
            prefab typography { font { family: "Mono" } }
        "#;
        let items = parse(src, 0).unwrap();
        let Stmt::Entity { bases, body, .. } = &items[0] else { panic!() };
        assert_eq!(bases[0][0].name.as_ref(), "typography");
        assert!(matches!(body[0], Stmt::Entity { .. }));
        assert!(matches!(body[1], Stmt::Component { name: Name::Str(..), .. }));
        let Stmt::Entity { body: keymap, .. } = &body[2] else { panic!() };
        let Stmt::Entity { body: editor, .. } = &keymap[0] else { panic!() };
        let Stmt::Entity { body: key, .. } = &editor[0] else { panic!() };
        assert!(matches!(key[0], Stmt::Component { value: None, .. }));
        assert!(matches!(items[1], Stmt::Prefab { .. }));
    }

    #[test]
    fn colon_brace_is_a_scope_error() {
        for src in ["zed { terminal: { font_size: 13 } }", "x { \"a.b\": default { c: 1 } }"] {
            let err = parse(src, 0).unwrap_err();
            assert!(err.message.contains("{ ... }`: a nested object is a scope"), "{src}: {}", err.message);
        }
        assert!(parse("x { a: json({ b: 1 }); c: [{ d: 2 }] }", 0).is_ok());
    }

    #[test]
    fn block_string_strips_closing_indent() {
        let src = "x { c: r\"\"\"\n    a $b\n      c\n\n    \"\"\" }";
        let items = parse(src, 0).unwrap();
        let Stmt::Entity { body, .. } = &items[0] else { panic!() };
        let Stmt::Component { value: Some(Expr { kind: ExprKind::Str(parts), .. }), .. } = &body[0] else { panic!() };
        assert_eq!(parts, &[StrPart::Lit("a $b\n  c\n".into())]);
    }
}
