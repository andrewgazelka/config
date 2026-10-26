//! Expressions. Strict and first-order: no functions, no recursion. Without a
//! database only constants and loop variables are visible, which is how the
//! structure of the config (names, `for`, `if`) is kept independent of values.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::db::{self, Db, EntPath, FILES, PREFAB, Slot};
use crate::lower::{Body, Thunk};
use crate::merge::{self, Leaf, LeafValue, Level, NoSets};
use crate::render;
use crate::syntax::Diag;
use crate::syntax::Span;
use crate::syntax::ast::*;
use crate::value::Value;

/// Top-level constants of one file, visible everywhere in it.
#[derive(Debug, Default, PartialEq)]
pub struct Scope {
    pub consts: BTreeMap<Arc<str>, Arc<Expr>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Env {
    frames: Option<Arc<Frame>>,
    scope: Arc<Scope>,
}

#[derive(Debug, PartialEq)]
struct Frame {
    name: Arc<str>,
    bind: Bind,
    next: Option<Arc<Frame>>,
}

#[derive(Debug, PartialEq)]
enum Bind {
    Val(Ev),
    /// A block-level `const`, evaluated where it is used.
    Lazy(Arc<Expr>, Env),
}

impl Env {
    pub fn new(scope: Arc<Scope>) -> Self {
        Env { frames: None, scope }
    }

    fn push(&self, name: Arc<str>, bind: Bind) -> Env {
        Env { frames: Some(Arc::new(Frame { name, bind, next: self.frames.clone() })), scope: self.scope.clone() }
    }

    pub fn bind(&self, name: &Ident, v: Ev) -> Env {
        self.push(name.name.clone(), Bind::Val(v))
    }

    pub fn bind_lazy(&self, name: &Ident, e: Arc<Expr>) -> Env {
        self.push(name.name.clone(), Bind::Lazy(e, self.clone()))
    }

    /// Bind a `for` pattern: one name takes the item (lists) or the key
    /// (records, children); two names take key and item.
    pub fn bind_pattern(&self, p: &Pattern, item: &Item) -> Env {
        match &p.second {
            None => self.bind(&p.first, if item.list { item.value.clone() } else { item.key.clone() }),
            Some(second) => self.bind(&p.first, item.key.clone()).bind(second, item.value.clone()),
        }
    }
}

/// What an expression evaluates to: a value, or a reference to an entity
/// (or its children), which only component access and `for` consume.
#[derive(Clone, Debug, PartialEq)]
pub enum Ev {
    V(Value),
    Ent(EntPath),
    Kids(EntPath),
}

pub struct Item {
    pub key: Ev,
    pub value: Ev,
    list: bool,
}

/// `Missing` is what `??` catches: an unset component, field or entity.
#[derive(Debug)]
pub enum Fault {
    Missing(Diag),
    Fail(Diag),
}

impl Fault {
    pub fn diag(self) -> Diag {
        match self {
            Fault::Missing(d) | Fault::Fail(d) => d,
        }
    }
}

pub type R<T> = Result<T, Fault>;

fn fail<T>(span: Span, msg: impl Into<String>) -> R<T> {
    Err(Fault::Fail(Diag::new(span, msg)))
}

fn missing<T>(span: Span, msg: impl Into<String>) -> R<T> {
    Err(Fault::Missing(Diag::new(span, msg)))
}

pub struct Eval<'a> {
    db: Option<&'a dyn Db>,
    depth: Cell<u32>,
}

impl<'a> Eval<'a> {
    pub fn new(db: Option<&'a dyn Db>) -> Self {
        Eval { db, depth: Cell::new(0) }
    }

    fn db(&self, span: Span, what: &str) -> R<&'a dyn Db> {
        self.db.ok_or_else(|| {
            Fault::Fail(Diag::new(
                span,
                format!("{what} here: entity names, `for` and `if` may only use constants and loop variables (use a `rule` to build structure from values)"),
            ))
        })
    }

    pub fn value(&self, e: &Expr, env: &Env) -> R<Value> {
        match self.expr(e, env)? {
            Ev::V(v) => Ok(v),
            Ev::Ent(p) => {
                fail(e.span, format!("`{}` is an entity; read a component with `[component]`", db::show_ent(&p)))
            }
            Ev::Kids(p) => fail(e.span, format!("`{}.*` is a set of entities, not a value", db::show_ent(&p))),
        }
    }

    pub fn string(&self, e: &Expr, env: &Env) -> R<String> {
        match self.value(e, env)? {
            Value::Str(s) => Ok(s.to_string()),
            v => fail(e.span, format!("expected a string, found {}", v.kind())),
        }
    }

    pub fn bool(&self, e: &Expr, env: &Env) -> R<bool> {
        match self.value(e, env)? {
            Value::Bool(b) => Ok(b),
            v => fail(e.span, format!("expected a bool, found {}", v.kind())),
        }
    }

    pub fn name(&self, n: &Name, env: &Env) -> R<Arc<str>> {
        match n {
            Name::Word(i) => Ok(i.name.clone()),
            Name::Str(parts, _) => Ok(self.interpolate(parts, env)?.into()),
        }
    }

    fn interpolate(&self, parts: &[StrPart], env: &Env) -> R<String> {
        let mut out = String::new();
        for p in parts {
            match p {
                StrPart::Lit(s) => out.push_str(s),
                StrPart::Interp(e) => match self.value(e, env)? {
                    v @ (Value::List(_) | Value::Record(_)) => {
                        return fail(e.span, format!("cannot interpolate a {}", v.kind()));
                    }
                    v => out.push_str(&v.display_scalar().unwrap_or_default()),
                },
            }
        }
        Ok(out)
    }

    fn lookup(&self, env: &Env, name: &Ident) -> R<Option<Ev>> {
        let mut frame = env.frames.as_deref();
        while let Some(f) = frame {
            if f.name == name.name {
                return match &f.bind {
                    Bind::Val(v) => Ok(Some(v.clone())),
                    Bind::Lazy(e, env) => self.nested(name.span, || self.expr(e, env)).map(Some),
                };
            }
            frame = f.next.as_deref();
        }
        match env.scope.consts.get(&name.name) {
            Some(e) => {
                let top = Env::new(env.scope.clone());
                self.nested(name.span, || self.expr(e, &top)).map(Some)
            }
            None => Ok(None),
        }
    }

    /// Whether `name` is a local or constant, without evaluating it.
    fn bound(env: &Env, name: &str) -> bool {
        let mut frame = env.frames.as_deref();
        while let Some(f) = frame {
            if f.name.as_ref() == name {
                return true;
            }
            frame = f.next.as_deref();
        }
        env.scope.consts.contains_key(name)
    }

    /// `n-1` lexes as one name, as in Nix. When every piece of an unknown
    /// name is a local or a number, the author meant `n - 1`.
    fn subtraction_hint(word: &str, env: &Env) -> Option<String> {
        let parts: Vec<&str> = word.split('-').collect();
        let operand = |p: &&str| !p.is_empty() && (Self::bound(env, p) || p.bytes().all(|b| b.is_ascii_digit()));
        if parts.len() < 2 || !parts.iter().all(operand) {
            return None;
        }
        Some(format!("`{word}` is one name; subtraction needs spaces: `{}`", parts.join(" - ")))
    }

    fn nested<T>(&self, span: Span, f: impl FnOnce() -> R<T>) -> R<T> {
        let d = self.depth.get();
        if d > 64 {
            return fail(span, "constants refer to each other in a cycle");
        }
        self.depth.set(d + 1);
        let r = f();
        self.depth.set(d);
        r
    }

    pub fn expr(&self, e: &Expr, env: &Env) -> R<Ev> {
        let v = |v: Value| Ok(Ev::V(v));
        match &e.kind {
            ExprKind::Null => v(Value::Null),
            ExprKind::Bool(b) => v(Value::Bool(*b)),
            ExprKind::Int(i) => v(Value::Int(*i)),
            ExprKind::Float(f) => v(Value::float(*f)),
            ExprKind::Str(parts) => v(Value::str(self.interpolate(parts, env)?)),
            ExprKind::Record(fields) => {
                let mut leaves = Vec::new();
                self.field_leaves(fields, env, &mut Vec::new(), Level::Plain, &mut leaves)?;
                match merge::merge(leaves, &NoSets) {
                    Ok(m) => v(m.value),
                    Err(err) => {
                        let [a, b] = err.spans();
                        Err(Fault::Fail(Diag::new(a.span, err.message()).note(b.span, "the other value is here")))
                    }
                }
            }
            ExprKind::List(elems) => {
                let mut items = Vec::new();
                self.elems(elems, env, &mut items)?;
                v(Value::list(items))
            }
            ExprKind::Path(names) => self.path(names, env, e.span),
            ExprKind::Field(base, name) => {
                let base = self.expr(base, env)?;
                self.field(base, name, env)
            }
            ExprKind::Children(base) => match self.expr(base, env)? {
                Ev::Ent(p) => {
                    self.db(e.span, "`.*` reads the config's structure")?;
                    Ok(Ev::Kids(p))
                }
                _ => fail(e.span, "`.*` needs an entity"),
            },
            ExprKind::Component(base, comp) => match self.expr(base, env)? {
                Ev::Ent(p) => self.component(&p, comp, e.span).map(Ev::V),
                _ => fail(e.span, format!("`[{}]` needs an entity", comp.name)),
            },
            ExprKind::Call(name, args) => self.call(name, args, env, e.span).map(Ev::V),
            ExprKind::Unary(op, x) => {
                let x = self.value(x, env)?;
                match (op, x) {
                    (UnOp::Neg, Value::Int(i)) => v(Value::Int(-i)),
                    (UnOp::Neg, Value::Float(f)) => v(Value::Float(-f)),
                    (UnOp::Not, Value::Bool(b)) => v(Value::Bool(!b)),
                    (_, x) => fail(e.span, format!("cannot apply {op:?} to a {}", x.kind())),
                }
            }
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b, env, e.span),
            ExprKind::If(c, t, f) => {
                if self.bool(c, env)? {
                    self.expr(t, env)
                } else {
                    self.expr(f, env)
                }
            }
            ExprKind::Range(lo, hi, inclusive) => {
                let (Value::Int(lo), Value::Int(hi)) = (self.value(lo, env)?, self.value(hi, env)?) else {
                    return fail(e.span, "a range needs two integers");
                };
                let hi = if *inclusive { hi + 1 } else { hi };
                v(Value::list((lo..hi).map(Value::Int).collect()))
            }
            ExprKind::Priority(_, x) => self.expr(x, env),
        }
    }

    fn path(&self, names: &[Name], env: &Env, span: Span) -> R<Ev> {
        let (first, rest) = names.split_first().expect("a path has a name");
        if let Name::Word(i) = first
            && let Some(mut cur) = self.lookup(env, i)?
        {
            for n in rest {
                cur = self.field(cur, n, env)?;
            }
            return Ok(cur);
        }
        let hint = match first {
            Name::Word(i) => Self::subtraction_hint(&i.name, env),
            Name::Str(..) => None,
        };
        let shown = match first {
            Name::Word(i) => format!("`{}` is not a constant or loop variable", i.name),
            Name::Str(..) => "an entity path".into(),
        };
        if self.db.is_none()
            && let Some(h) = &hint
        {
            return fail(span, h.clone());
        }
        let db = self.db(span, &shown)?;
        let mut parts: Vec<Arc<str>> = Vec::with_capacity(names.len());
        for n in names {
            parts.push(self.name(n, env)?);
        }
        if parts[0].as_ref() == FILES {
            return fail(span, "nothing may read `files`: it is the output of the config");
        }
        let idx = db::base_index(db);
        let root: EntPath = [parts[0].clone()].into();
        if !idx.entities.contains_key(&root) {
            let prefab: EntPath = [Arc::from(PREFAB), parts[0].clone()].into();
            if idx.entities.contains_key(&prefab) {
                parts.insert(0, Arc::from(PREFAB));
            } else if let Some(h) = hint {
                return fail(span, h);
            } else {
                return missing(span, format!("no entity or prefab named `{}`", parts[0]));
            }
        }
        Ok(Ev::Ent(parts.into()))
    }

    fn field(&self, base: Ev, name: &Name, env: &Env) -> R<Ev> {
        let key = self.name(name, env)?;
        match base {
            Ev::V(Value::Record(map)) => match map.get(key.as_ref()) {
                Some(v) => Ok(Ev::V(v.clone())),
                None => missing(name.span(), format!("no field `{key}`")),
            },
            Ev::Ent(p) => Ok(Ev::Ent(p.iter().cloned().chain([key]).collect())),
            Ev::V(v) => fail(name.span(), format!("`.{key}` on a {}", v.kind())),
            Ev::Kids(_) => fail(name.span(), format!("`.{key}` on a set of entities")),
        }
    }

    fn component(&self, p: &EntPath, comp: &Ident, span: Span) -> R<Value> {
        let db = self.db(span, "a component value is read")?;
        let idx = db::index_for(db, p);
        if !idx.entities.contains_key(p) {
            return missing(span, format!("no entity `{}`", db::show_ent(p)));
        }
        if !idx.has(p, &comp.name) {
            return missing(span, format!("`{}` has no component `{}`", db::show_ent(p), comp.name));
        }
        db::value(db, Slot::new(db, p.clone(), comp.name.clone())).map_err(Fault::Fail)
    }

    fn record_of(&self, p: &EntPath, span: Span) -> R<Value> {
        let db = self.db(span, "an entity is read")?;
        let idx = db::index_for(db, p);
        let Some(info) = idx.entities.get(p) else {
            return missing(span, format!("no entity `{}`", db::show_ent(p)));
        };
        let app = match p.as_ref() {
            [root] if idx.apps.contains(root) => Some(root.as_ref()),
            _ => None,
        };
        let skip = |k: &str| app.is_some_and(|a| idx.strata_field(a, k));
        let mut map = BTreeMap::new();
        for c in info.comps.iter().filter(|c| !skip(c)) {
            let v = db::value(db, Slot::new(db, p.clone(), c.clone())).map_err(Fault::Fail)?;
            map.insert(c.to_string(), v);
        }
        for k in info.children.iter().filter(|k| !skip(k)) {
            let child: EntPath = p.iter().cloned().chain([k.clone()]).collect();
            if map.insert(k.to_string(), self.record_of(&child, span)?).is_some() {
                return fail(span, format!("`{}` is both a component and a scope", db::show_ent(&child)));
            }
        }
        Ok(Value::record(map))
    }

    fn binary(&self, op: BinOp, a: &Expr, b: &Expr, env: &Env, span: Span) -> R<Ev> {
        match op {
            BinOp::Else => {
                return match self.expr(a, env) {
                    Err(Fault::Missing(_)) => self.expr(b, env),
                    r => r,
                };
            }
            BinOp::And | BinOp::Or => {
                let x = self.bool(a, env)?;
                if (op == BinOp::And) != x {
                    return Ok(Ev::V(Value::Bool(x)));
                }
                return self.bool(b, env).map(|y| Ev::V(Value::Bool(y)));
            }
            _ => {}
        }
        let (x, y) = (self.value(a, env)?, self.value(b, env)?);
        let num = |v: &Value| match v {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        };
        let out = match (op, &x, &y) {
            (BinOp::Eq, ..) => Value::Bool(x == y),
            (BinOp::Ne, ..) => Value::Bool(x != y),
            (BinOp::Add, Value::Int(i), Value::Int(j)) => match i.checked_add(*j) {
                Some(k) => Value::Int(k),
                None => return fail(span, "integer overflow"),
            },
            (BinOp::Sub, Value::Int(i), Value::Int(j)) => match i.checked_sub(*j) {
                Some(k) => Value::Int(k),
                None => return fail(span, "integer overflow"),
            },
            (BinOp::Mul, Value::Int(i), Value::Int(j)) => match i.checked_mul(*j) {
                Some(k) => Value::Int(k),
                None => return fail(span, "integer overflow"),
            },
            (BinOp::Add, Value::Str(s), Value::Str(t)) => Value::str(format!("{s}{t}")),
            (BinOp::Add, Value::List(s), Value::List(t)) => Value::list(s.iter().chain(t.iter()).cloned().collect()),
            (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, Value::Str(_), Value::Str(_)) => cmp(op, x.cmp(&y)),
            _ => match (num(&x), num(&y)) {
                (Some(i), Some(j)) => match op {
                    BinOp::Add => Value::float(i + j),
                    BinOp::Sub => Value::float(i - j),
                    BinOp::Mul => Value::float(i * j),
                    BinOp::Div if j == 0.0 => return fail(span, "division by zero"),
                    BinOp::Div => Value::float(i / j),
                    _ => cmp(op, i.total_cmp(&j)),
                },
                _ => return fail(span, format!("cannot apply {op:?} to {} and {}", x.kind(), y.kind())),
            },
        };
        Ok(Ev::V(out))
    }

    pub fn iterate(&self, e: &Expr, env: &Env) -> R<Vec<Item>> {
        match self.expr(e, env)? {
            Ev::V(Value::List(items)) => Ok(items
                .iter()
                .enumerate()
                .map(|(i, v)| Item { key: Ev::V(Value::Int(i as i64)), value: Ev::V(v.clone()), list: true })
                .collect()),
            Ev::V(Value::Record(map)) => Ok(map
                .iter()
                .map(|(k, v)| Item { key: Ev::V(Value::str(k.as_str())), value: Ev::V(v.clone()), list: false })
                .collect()),
            Ev::Kids(p) => {
                let db = self.db(e.span, "`.*` reads the config's structure")?;
                let idx = db::index_for(db, &p);
                let kids = idx.entities.get(&p).map(|i| i.children.clone()).unwrap_or_default();
                Ok(kids
                    .into_iter()
                    .map(|k| Item {
                        key: Ev::V(Value::str(k.clone())),
                        value: Ev::Ent(p.iter().cloned().chain([k]).collect()),
                        list: false,
                    })
                    .collect())
            }
            Ev::V(v) => fail(e.span, format!("cannot iterate over a {}", v.kind())),
            Ev::Ent(p) => {
                fail(e.span, format!("cannot iterate over entity `{}`; use `.*` for its children", db::show_ent(&p)))
            }
        }
    }

    fn elems(&self, elems: &[Elem], env: &Env, out: &mut Vec<Value>) -> R<()> {
        for el in elems {
            match el {
                Elem::Item(e) => out.push(self.value(e, env)?),
                Elem::Spread(e) => match self.value(e, env)? {
                    Value::List(items) => out.extend(items.iter().cloned()),
                    v => return fail(e.span, format!("`...` in a list needs a list, found {}", v.kind())),
                },
                Elem::For { pattern, iter, body } => {
                    for item in self.iterate(iter, env)? {
                        self.elems(body, &env.bind_pattern(pattern, &item), out)?;
                    }
                }
                Elem::If { cond, then, otherwise } => {
                    let branch = if self.bool(cond, env)? { then } else { otherwise };
                    self.elems(branch, env, out)?;
                }
            }
        }
        Ok(())
    }

    /// A component value as leaves: record literals stay open so their
    /// fields merge with other contributions, and `default`/`override`
    /// apply to the subtree they annotate.
    pub fn leaves(&self, e: &Expr, env: &Env, prefix: &mut Vec<String>, level: Level, out: &mut Vec<Leaf>) -> R<()> {
        match &e.kind {
            ExprKind::Priority(l, inner) => self.leaves(inner, env, prefix, *l, out),
            ExprKind::Record(fields) => {
                let before = out.len();
                self.field_leaves(fields, env, prefix, level, out)?;
                if out.len() == before {
                    out.push(Leaf { path: prefix.clone(), value: LeafValue::Record, level, span: e.span, via: None });
                }
                Ok(())
            }
            _ => {
                let v = self.value(e, env)?;
                merge::flatten(&v, prefix, level, e.span, out);
                Ok(())
            }
        }
    }

    fn field_leaves(
        &self,
        fields: &[Field],
        env: &Env,
        prefix: &mut Vec<String>,
        level: Level,
        out: &mut Vec<Leaf>,
    ) -> R<()> {
        for f in fields {
            match f {
                Field::Pair { key, value } => {
                    prefix.push(self.name(key, env)?.to_string());
                    let r = self.leaves(value, env, prefix, level, out);
                    prefix.pop();
                    r?;
                }
                Field::Spread(e) => match self.value(e, env)? {
                    Value::Record(map) => {
                        for (k, v) in map.iter() {
                            prefix.push(k.clone());
                            merge::flatten(v, prefix, level, e.span, out);
                            prefix.pop();
                        }
                    }
                    v => return fail(e.span, format!("`...` in a record needs a record, found {}", v.kind())),
                },
                Field::For { pattern, iter, body } => {
                    for item in self.iterate(iter, env)? {
                        self.field_leaves(body, &env.bind_pattern(pattern, &item), prefix, level, out)?;
                    }
                }
                Field::If { cond, then, otherwise } => {
                    let branch = if self.bool(cond, env)? { then } else { otherwise };
                    self.field_leaves(branch, env, prefix, level, out)?;
                }
            }
        }
        Ok(())
    }

    /// A component's contribution: one whole value, at the level its
    /// outermost `default`/`override` names.
    pub fn thunk_leaf(&self, t: &Thunk) -> R<Leaf> {
        let (level, value) = match &t.body {
            Body::Tag => (Level::Plain, Value::Bool(true)),
            Body::Lit(v) => (Level::Plain, v.clone()),
            Body::Expr(e, env) => match &e.kind {
                ExprKind::Priority(l, inner) => (*l, self.value(inner, env)?),
                _ => (Level::Plain, self.value(e, env)?),
            },
        };
        Ok(Leaf { path: vec![], value: LeafValue::Scalar(value), level, span: t.span, via: None })
    }

    fn call(&self, name: &Ident, args: &[Expr], env: &Env, span: Span) -> R<Value> {
        let arity = |n: usize| {
            if args.len() == n {
                Ok(())
            } else {
                fail(span, format!("`{}` takes {n} argument{}", name.name, if n == 1 { "" } else { "s" }))
            }
        };
        let file_value = |content: Result<String, String>| match content {
            Ok(text) => Ok(record([("content", Value::str(text))])),
            Err(msg) => fail(span, format!("{}: {msg}", name.name)),
        };
        match name.name.as_ref() {
            "json" => {
                arity(1)?;
                file_value(render::json(&self.value(&args[0], env)?))
            }
            "toml" => {
                arity(1)?;
                file_value(render::toml(&self.value(&args[0], env)?))
            }
            "kv" => {
                arity(1)?;
                file_value(render::kv(&self.value(&args[0], env)?))
            }
            "ini" => {
                arity(1)?;
                file_value(render::ini(&self.value(&args[0], env)?))
            }
            "text" => {
                arity(1)?;
                file_value(Ok(self.string(&args[0], env)?))
            }
            "source" | "tree" => {
                arity(1)?;
                let rel = self.string(&args[0], env)?;
                self.source_file(&name.name, &rel, span)
            }
            "read" => {
                // The text of a file next to the source, for building a file from parts.
                arity(1)?;
                let rel = self.string(&args[0], env)?;
                self.read_file(&rel, span)
            }
            "entries" => {
                // A directory's top-level entries as name -> source() or tree(),
                // so each can be linked on its own (`dirs` in core.cfg).
                arity(1)?;
                let rel = self.string(&args[0], env)?;
                self.dir_entries(&rel, span)
            }
            "has" => {
                arity(2)?;
                let ExprKind::Path(comp) = &args[1].kind else {
                    return fail(args[1].span, "`has` takes an entity and a component name");
                };
                let [Name::Word(comp)] = comp.as_slice() else {
                    return fail(args[1].span, "`has` takes an entity and a component name");
                };
                match self.expr(&args[0], env) {
                    Ok(Ev::Ent(p)) => {
                        let db = self.db(span, "`has` reads the config's structure")?;
                        Ok(Value::Bool(db::index_for(db, &p).has(&p, &comp.name)))
                    }
                    Err(Fault::Missing(_)) => Ok(Value::Bool(false)),
                    Err(f) => Err(f),
                    Ok(_) => fail(args[0].span, "`has` needs an entity"),
                }
            }
            "exists" => {
                arity(1)?;
                match self.expr(&args[0], env) {
                    Ok(Ev::Ent(p)) => {
                        let db = self.db(span, "`exists` reads the config's structure")?;
                        Ok(Value::Bool(db::index_for(db, &p).entities.contains_key(&p)))
                    }
                    Err(Fault::Missing(_)) => Ok(Value::Bool(false)),
                    Err(f) => Err(f),
                    Ok(_) => fail(args[0].span, "`exists` needs an entity"),
                }
            }
            "record" => {
                // An entity's subtree as a value: children become records,
                // components keys. On an app entity the module's own fields
                // stay out: what is left is the app's config, verbatim.
                arity(1)?;
                match self.expr(&args[0], env)? {
                    Ev::Ent(p) => self.record_of(&p, span),
                    _ => fail(args[0].span, "`record` needs an entity"),
                }
            }
            "copy" => {
                // A file the app rewrites itself: a copy, not a link.
                arity(1)?;
                match self.value(&args[0], env)? {
                    Value::Record(map) => {
                        let mut map = (*map).clone();
                        map.insert("kind".into(), Value::str("copy"));
                        Ok(Value::record(map))
                    }
                    v => fail(args[0].span, format!("`copy` needs a file, found {}", v.kind())),
                }
            }
            "link" => {
                // A link to an absolute path, e.g. a Nix store path.
                arity(1)?;
                Ok(record([("symlink", Value::str(self.string(&args[0], env)?))]))
            }
            "join" => {
                arity(2)?;
                let sep = self.string(&args[1], env)?;
                match self.value(&args[0], env)? {
                    Value::List(items) => {
                        let parts: Option<Vec<String>> = items.iter().map(|v| v.display_scalar()).collect();
                        match parts {
                            Some(p) => Ok(Value::str(p.join(&sep))),
                            None => fail(args[0].span, "`join` needs a list of scalars"),
                        }
                    }
                    v => fail(args[0].span, format!("`join` needs a list, found {}", v.kind())),
                }
            }
            "str" => {
                arity(1)?;
                let v = self.value(&args[0], env)?;
                match v.display_scalar() {
                    Some(s) => Ok(Value::str(s)),
                    None => fail(args[0].span, format!("`str` of a {}", v.kind())),
                }
            }
            "keys" | "values" => {
                arity(1)?;
                let items = self.iterate(&args[0], env)?;
                let pick = |i: Item| if name.name.as_ref() == "keys" { i.key } else { i.value };
                items
                    .into_iter()
                    .map(|i| match pick(i) {
                        Ev::V(v) => Ok(v),
                        _ => fail(span, "`values` of entities; read a component instead"),
                    })
                    .collect::<R<Vec<_>>>()
                    .map(Value::list)
            }
            "sort" => {
                arity(1)?;
                match self.value(&args[0], env)? {
                    Value::List(items) => {
                        let mut items = items.to_vec();
                        items.sort();
                        Ok(Value::list(items))
                    }
                    v => fail(args[0].span, format!("`sort` needs a list, found {}", v.kind())),
                }
            }
            "sort-by" => {
                arity(2)?;
                let key = self.string(&args[1], env)?;
                match self.value(&args[0], env)? {
                    Value::List(items) => {
                        let mut items = items.to_vec();
                        let field = |v: &Value| v.as_record().and_then(|r| r.get(&key)).cloned().unwrap_or(Value::Null);
                        items.sort_by_key(field);
                        Ok(Value::list(items))
                    }
                    v => fail(args[0].span, format!("`sort-by` needs a list, found {}", v.kind())),
                }
            }
            "to-json" => {
                // Compact JSON of any value: quoting for formats whose strings are JSON's.
                arity(1)?;
                let v = self.value(&args[0], env)?;
                Ok(Value::str(serde_json::to_string(&v).expect("values serialize")))
            }
            "kind" => {
                arity(1)?;
                Ok(Value::str(self.value(&args[0], env)?.kind()))
            }
            "shquote" => {
                arity(1)?;
                let v = self.value(&args[0], env)?;
                let Some(s) = v.display_scalar() else {
                    return fail(args[0].span, format!("`shquote` of a {}", v.kind()));
                };
                let bare = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_./:+-@=,".contains(c));
                Ok(Value::str(if bare { s } else { format!("'{}'", s.replace('\'', "'\\''")) }))
            }
            "len" => {
                arity(1)?;
                match self.value(&args[0], env)? {
                    Value::List(items) => Ok(Value::Int(items.len() as i64)),
                    Value::Record(map) => Ok(Value::Int(map.len() as i64)),
                    Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
                    v => fail(args[0].span, format!("`len` of a {}", v.kind())),
                }
            }
            other => fail(name.span, format!("unknown function `{other}`")),
        }
    }

    /// A path next to the calling source file: the blob key, and the path
    /// diagnostics show.
    fn beside(&self, rel: &str, span: Span, what: &str) -> R<(&'a dyn Db, String, PathBuf)> {
        let db = self.db(span, what)?;
        let src = db.ws().sources(db)[span.file as usize];
        let abs = normalize(&Path::new(src.dir(db)).join(rel));
        let shown = normalize(&Path::new(src.path(db)).parent().unwrap_or(Path::new("")).join(rel));
        Ok((db, abs.to_string_lossy().to_string(), shown))
    }

    fn read_file(&self, rel: &str, span: Span) -> R<Value> {
        let (db, key, shown) = self.beside(rel, span, "a file is read")?;
        let Some(blob) = db.ws().blobs(db).get(&key) else {
            return fail(span, format!("no file {}", shown.display()));
        };
        match std::str::from_utf8(blob.bytes(db)) {
            Ok(text) => Ok(Value::str(text)),
            Err(_) => fail(span, format!("{} is not UTF-8 text", shown.display())),
        }
    }

    fn dir_entries(&self, rel: &str, span: Span) -> R<Value> {
        let (db, key, shown) = self.beside(rel, span, "a directory is read")?;
        let listing = db::tree_entries(db, key);
        if listing.is_empty() {
            return fail(span, format!("no directory {} (or it is empty)", shown.display()));
        }
        let mut names: BTreeMap<&str, bool> = BTreeMap::new();
        for (path, _, _) in listing.iter() {
            match path.split_once('/') {
                Some((dir, _)) => names.insert(dir, true),
                None => names.insert(path, false),
            };
        }
        let rel = rel.trim_end_matches('/');
        let mut out = BTreeMap::new();
        for (name, is_dir) in names {
            let kind = if is_dir { "tree" } else { "source" };
            out.insert(name.to_string(), self.source_file(kind, &format!("{rel}/{name}"), span)?);
        }
        Ok(Value::record(out))
    }

    /// `source("x")` / `tree("dir")`: content from next to the source file,
    /// named by its blake3 so the `file` value changes exactly when bytes do.
    fn source_file(&self, kind: &str, rel: &str, span: Span) -> R<Value> {
        let (db, key, shown) = self.beside(rel, span, "a file is read")?;
        let from = Value::str(shown.to_string_lossy().as_ref());
        let blobs = db.ws().blobs(db);
        if kind == "source" {
            let Some(blob) = blobs.get(&key) else {
                return fail(span, format!("no file {}", shown.display()));
            };
            let hash = blake3::Hash::from_bytes(*db::blob_hash(db, *blob));
            let mut fields = vec![("blob", Value::str(hash.to_hex().as_str())), ("from", from)];
            if blob.exec(db) {
                fields.push(("mode", Value::str("0755")));
            }
            return Ok(record(fields));
        }
        let entries = db::tree_entries(db, key.clone());
        if entries.is_empty() {
            return fail(span, format!("no directory {} (or it is empty)", shown.display()));
        }
        let mut h = blake3::Hasher::new();
        for (rel, exec, hash) in entries.iter() {
            h.update(rel.as_bytes()).update(&[0, u8::from(*exec)]).update(hash);
        }
        Ok(record([("tree", Value::str(h.finalize().to_hex().as_str())), ("from", from)]))
    }
}

fn cmp(op: BinOp, o: std::cmp::Ordering) -> Value {
    use std::cmp::Ordering::*;
    Value::Bool(match op {
        BinOp::Lt => o == Less,
        BinOp::Le => o != Greater,
        BinOp::Gt => o == Greater,
        _ => o != Less,
    })
}

pub fn record<'k>(fields: impl IntoIterator<Item = (&'k str, Value)>) -> Value {
    Value::record(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// Lexical `.`/`..` removal; config paths never go through symlinks.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}
