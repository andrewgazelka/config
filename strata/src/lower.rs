//! Statements to contributions: which entities exist and which expressions
//! set which (entity, component). Values stay unevaluated; base files are
//! lowered without a database, so their structure can't depend on values.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crate::db::{Db, EntPath, FILES, PREFAB};
use crate::eval::{Env, Eval, Fault, Scope, normalize};
use crate::syntax::ast::*;
use crate::syntax::{Diag, Span};
use crate::value::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    Base(u32),
    Rule(u32, u32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Expr(Arc<Expr>, Env),
    /// A bare marker component: `unbind`.
    Tag,
    Lit(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Thunk {
    pub body: Body,
    pub span: Span,
    /// Set under a base-level `if`.
    pub cond: bool,
}

impl Thunk {
    pub fn literal(v: Value, span: Span) -> Self {
        Thunk { body: Body::Lit(v), span, cond: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FactsDecl {
    pub name: Arc<str>,
    pub comp: Arc<str>,
    pub source: FactSource,
    pub dir: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleDecl {
    pub name: Arc<str>,
    pub body: Arc<Vec<Stmt>>,
    pub scope: Arc<Scope>,
    pub span: Span,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileContribs {
    /// Entities named here, with the prefabs each one lists and where.
    pub entities: Vec<(EntPath, Vec<EntPath>, Span)>,
    pub slots: BTreeMap<(EntPath, Arc<str>), Vec<Thunk>>,
    pub decls: Vec<(Arc<str>, TypeExpr, Span)>,
    pub types: Vec<(Arc<str>, TypeExpr, Span)>,
    pub facts: Vec<FactsDecl>,
    pub rules: Vec<RuleDecl>,
    pub diags: Vec<Diag>,
}

struct Walker<'a> {
    ev: Eval<'a>,
    out: FileContribs,
    rule: bool,
    cond: bool,
}

pub fn lower_file(db: &dyn Db, file: u32, stmts: &[Stmt]) -> FileContribs {
    let src = db.ws().sources(db)[file as usize];
    let mut scope = Scope::default();
    let mut w = Walker { ev: Eval::new(None), out: FileContribs::default(), rule: false, cond: false };
    let mut seen: BTreeMap<Arc<str>, Span> = BTreeMap::new();
    for s in stmts {
        if let Stmt::Const { name, value, span } = s {
            if let Some(prev) = seen.insert(name.name.clone(), *span) {
                w.out.diags.push(
                    Diag::new(*span, format!("`{}` is defined twice", name.name)).note(prev, "first defined here"),
                );
            }
            scope.consts.insert(name.name.clone(), Arc::new(value.clone()));
        }
    }
    let scope = Arc::new(scope);
    let env = Env::new(scope.clone());
    for s in stmts {
        match s {
            Stmt::Const { .. } => {}
            Stmt::ComponentDecl { name, ty, span } => w.out.decls.push((name.name.clone(), ty.clone(), *span)),
            Stmt::TypeDecl { name, ty, span } => w.out.types.push((name.name.clone(), ty.clone(), *span)),
            Stmt::Facts { name, component, source, span } => w.out.facts.push(FactsDecl {
                name: name.name.clone(),
                comp: component.name.clone(),
                source: source.clone(),
                dir: src.dir(db).clone(),
                span: *span,
            }),
            Stmt::Rule { name, body, span } => w.out.rules.push(RuleDecl {
                name: name.name.clone(),
                body: Arc::new(body.clone()),
                scope: scope.clone(),
                span: *span,
            }),
            Stmt::Prefab { name, bases, body, span } => {
                let path: EntPath = [Arc::from(PREFAB), name.name.clone()].into();
                w.out.entities.push((path.clone(), bases.iter().map(|b| prefab_path(b)).collect(), *span));
                w.block(body, &path, &env);
            }
            s => w.stmt(s, &[].into(), &mut env.clone()),
        }
    }
    w.out
}

pub fn lower_rule(db: &dyn Db, rule: &RuleDecl) -> FileContribs {
    let mut w = Walker { ev: Eval::new(Some(db)), out: FileContribs::default(), rule: true, cond: false };
    w.block(&rule.body, &[].into(), &Env::new(rule.scope.clone()));
    w.out
}

fn prefab_path(b: &[Ident]) -> EntPath {
    std::iter::once(Arc::from(PREFAB)).chain(b.iter().map(|i| i.name.clone())).collect()
}

fn under_files(e: &[Arc<str>]) -> bool {
    e.first().is_some_and(|s| s.as_ref() == FILES)
}

impl Walker<'_> {
    fn block(&mut self, stmts: &[Stmt], entity: &EntPath, env: &Env) {
        let mut env = env.clone();
        for s in stmts {
            self.stmt(s, entity, &mut env);
        }
    }

    fn report(&mut self, f: Fault) {
        self.out.diags.push(f.diag());
    }

    fn stmt(&mut self, s: &Stmt, entity: &EntPath, env: &mut Env) {
        match s {
            Stmt::Entity { path, bases, body, span } => {
                let mut e: Vec<Arc<str>> = entity.to_vec();
                for n in path {
                    match self.ev.name(n, env) {
                        Ok(n) => e.push(n),
                        Err(f) => return self.report(f),
                    }
                }
                let e: EntPath = e.into();
                if self.rule && !under_files(&e) {
                    let msg = "a rule may only create entities under `files`";
                    return self.out.diags.push(Diag::new(*span, msg));
                }
                if self.rule && !bases.is_empty() {
                    return self.out.diags.push(Diag::new(*span, "entities made by a rule can't use prefabs"));
                }
                self.out.entities.push((e.clone(), bases.iter().map(|b| prefab_path(b)).collect(), *span));
                self.block(body, &e, env);
            }
            Stmt::Component { name, value, span } => {
                if entity.is_empty() {
                    return self.out.diags.push(Diag::new(*span, "a component must be set inside an entity"));
                }
                if !self.rule && under_files(entity) && entity.len() == 1 {
                    return self.out.diags.push(Diag::new(*span, "`files` itself has no components"));
                }
                let body = match value {
                    Some(e) => Body::Expr(Arc::new(e.clone()), env.clone()),
                    None => Body::Tag,
                };
                let name = match self.ev.name(name, env) {
                    Ok(n) => n,
                    Err(f) => return self.report(f),
                };
                let thunk = Thunk { body, span: *span, cond: self.cond };
                self.out.slots.entry((entity.clone(), name)).or_default().push(thunk);
            }
            Stmt::For { pattern, iter, body, .. } => match self.ev.iterate(iter, env) {
                Ok(items) => {
                    for item in items {
                        self.block(body, entity, &env.bind_pattern(pattern, &item));
                    }
                }
                Err(f) => self.report(f),
            },
            Stmt::If { cond, then, otherwise, .. } => match self.ev.bool(cond, env) {
                Ok(c) => {
                    let saved = self.cond;
                    self.cond = !self.rule;
                    self.block(if c { then } else { otherwise }, entity, env);
                    self.cond = saved;
                }
                Err(f) => self.report(f),
            },
            Stmt::Const { name, value, .. } => *env = env.bind_lazy(name, Arc::new(value.clone())),
            Stmt::Prefab { span, .. }
            | Stmt::ComponentDecl { span, .. }
            | Stmt::TypeDecl { span, .. }
            | Stmt::Facts { span, .. }
            | Stmt::Rule { span, .. } => {
                self.out.diags.push(Diag::new(*span, "declarations belong at the top level of a file"));
            }
        }
    }
}

/// A fact table: a JSON object of name -> value, from the command line,
/// the `nix eval` cache, or a JSON file next to the source.
pub fn fact_table(db: &dyn Db, f: &FactsDecl) -> Result<Arc<BTreeMap<String, Value>>, Diag> {
    let ws = db.ws();
    let text: Arc<str> = if let Some(t) = ws.facts(db).get(f.name.as_ref()) {
        t.clone()
    } else {
        match &f.source {
            FactSource::Json(rel) => {
                let abs = normalize(&Path::new(&f.dir).join(rel)).to_string_lossy().to_string();
                let Some(blob) = ws.blobs(db).get(&abs) else {
                    return Err(Diag::new(f.span, format!("no fact file {rel}")));
                };
                String::from_utf8_lossy(blob.bytes(db)).into()
            }
            FactSource::Nix { .. } => {
                return Err(Diag::new(f.span, format!("no cached facts for `{}`: run `strata nix-facts`", f.name)));
            }
        }
    };
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| Diag::new(f.span, format!("facts `{}`: {e}", f.name)))?;
    let Some(obj) = json.as_object() else {
        return Err(Diag::new(f.span, format!("facts `{}` must be a JSON object", f.name)));
    };
    Ok(Arc::new(obj.iter().map(|(k, v)| (k.clone(), Value::from(v))).collect()))
}
