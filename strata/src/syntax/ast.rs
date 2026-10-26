//! The syntax tree. One construct carries the language: a nested tree of
//! named entities (scopes, `name { ... }`) whose bodies set components (one
//! value each, `name: value`).

use std::sync::Arc;

use super::Span;
use crate::merge::Level;

#[derive(Clone, Debug, PartialEq)]
pub struct Ident {
    pub name: Arc<str>,
    pub span: Span,
}

/// An entity or record-key name: a bare word or a (maybe interpolated) string.
#[derive(Clone, Debug, PartialEq)]
pub enum Name {
    Word(Ident),
    Str(Vec<StrPart>, Span),
}

impl Name {
    pub fn span(&self) -> Span {
        match self {
            Name::Word(i) => i.span,
            Name::Str(_, s) => *s,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StrPart {
    Lit(String),
    Interp(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub first: Ident,
    pub second: Option<Ident>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    /// `a.b : prefab { ... }`
    Entity {
        path: Vec<Name>,
        bases: Vec<Vec<Ident>>,
        body: Vec<Stmt>,
        span: Span,
    },
    /// `name: value`, or a bare tag `name`. The name may be quoted.
    Component {
        name: Name,
        value: Option<Expr>,
        span: Span,
    },
    For {
        pattern: Pattern,
        iter: Expr,
        body: Vec<Stmt>,
        span: Span,
    },
    If {
        cond: Expr,
        then: Vec<Stmt>,
        otherwise: Vec<Stmt>,
        span: Span,
    },
    Const {
        name: Ident,
        value: Expr,
        span: Span,
    },
    Prefab {
        name: Ident,
        bases: Vec<Vec<Ident>>,
        body: Vec<Stmt>,
        span: Span,
    },
    ComponentDecl {
        name: Ident,
        ty: TypeExpr,
        span: Span,
    },
    TypeDecl {
        name: Ident,
        ty: TypeExpr,
        span: Span,
    },
    /// `facts pkgs: store-path from "facts.json"`: a JSON object as entities.
    Facts {
        name: Ident,
        component: Ident,
        source: FactSource,
        span: Span,
    },
    /// A block whose structure may depend on entity values. It may only
    /// create entities under `files`.
    Rule {
        name: Ident,
        body: Vec<Stmt>,
        span: Span,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum FactSource {
    Json(String),
    /// A Nix expression evaluated with `nix eval --json`, cached by the
    /// hash of the lock file and the expression.
    Nix {
        expr: String,
        lock: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeExpr {
    Named(Ident),
    Literal(String),
    List(Box<TypeExpr>),
    Set(Box<TypeExpr>),
    Map(Box<TypeExpr>),
    /// `rest`: the type of keys not in `fields` (`..` is `any`); none: closed.
    Record {
        fields: Vec<(String, TypeExpr, bool)>,
        rest: Option<Box<TypeExpr>>,
    },
    Union(Vec<TypeExpr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Vec<StrPart>),
    Record(Vec<Field>),
    List(Vec<Elem>),
    /// `a.b."c"`: a local, a field of a local, or an entity path.
    Path(Vec<Name>),
    /// `e.*`
    Children(Box<Expr>),
    /// `e[component]`
    Component(Box<Expr>, Ident),
    /// `v.key` on a computed value.
    Field(Box<Expr>, Name),
    Call(Ident, Vec<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Range(Box<Expr>, Box<Expr>, bool),
    Priority(Level, Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    /// `a ?? b`: `b` when `a` names something missing.
    Else,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    Pair { key: Name, value: Expr },
    Spread(Expr),
    For { pattern: Pattern, iter: Expr, body: Vec<Field> },
    If { cond: Expr, then: Vec<Field>, otherwise: Vec<Field> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Elem {
    Item(Expr),
    Spread(Expr),
    For { pattern: Pattern, iter: Expr, body: Vec<Elem> },
    If { cond: Expr, then: Vec<Elem>, otherwise: Vec<Elem> },
}
