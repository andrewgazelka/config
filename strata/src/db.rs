//! The incremental engine. Inputs are source texts and blobs; everything
//! else is a salsa query:
//!
//!   parse(file) -> contribs(file) -> index -> resolved(slot) -> value(slot)
//!
//! `index` holds only structure (names, components, bases), never spans or
//! values, so editing a value re-runs the slots it feeds and nothing else.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::eval::{Eval, Fault};
use crate::lower::{self, FileContribs, Origin, Thunk};
use crate::merge::{self, Leaf, LeafValue, Level, Merged};
use crate::syntax::ast::Stmt;
use crate::syntax::{Diag, Sources, Span, parser};
use crate::types::Ty;
use crate::value::Value;

pub type EntPath = Arc<[Arc<str>]>;

pub const PREFAB: &str = "\0prefab";
pub const FILES: &str = "files";

pub fn show_ent(path: &[Arc<str>]) -> String {
    let parts: Vec<String> = path.iter().map(|s| s.to_string()).collect();
    match parts.split_first() {
        Some((head, rest)) if head == PREFAB => format!("prefab {}", merge::show_path(rest)),
        _ => merge::show_path(&parts),
    }
}

#[salsa::input]
pub struct Source {
    /// The path shown in diagnostics.
    #[returns(ref)]
    pub path: String,
    /// Directory that `source()` and `tree()` paths are relative to.
    #[returns(ref)]
    pub dir: String,
    #[returns(ref)]
    pub text: Arc<str>,
}

#[salsa::input]
pub struct Blob {
    #[returns(ref)]
    pub bytes: Arc<[u8]>,
    #[returns(copy)]
    pub exec: bool,
}

#[salsa::input]
pub struct Workspace {
    #[returns(ref)]
    pub sources: Arc<Vec<Source>>,
    /// Every non-source file under the config roots, by absolute path.
    #[returns(ref)]
    pub blobs: Arc<BTreeMap<String, Blob>>,
    /// Fact tables given on the command line or cached from `nix eval`.
    #[returns(ref)]
    pub facts: Arc<BTreeMap<String, Arc<str>>>,
}

#[salsa::db]
pub trait Db: salsa::Database {
    fn ws(&self) -> Workspace;
}

#[salsa::db]
#[derive(Clone, Default)]
pub struct RootDb {
    storage: salsa::Storage<Self>,
    ws: Option<Workspace>,
}

#[salsa::db]
impl salsa::Database for RootDb {}

#[salsa::db]
impl Db for RootDb {
    fn ws(&self) -> Workspace {
        self.ws.expect("workspace is set at load")
    }
}

impl RootDb {
    pub fn new(sources: Vec<(String, String, String)>, blobs: BTreeMap<String, (Vec<u8>, bool)>) -> Self {
        let mut db = RootDb::default();
        let sources = sources.into_iter().map(|(path, dir, text)| Source::new(&db, path, dir, text.into())).collect();
        let blobs = blobs.into_iter().map(|(k, (bytes, exec))| (k, Blob::new(&db, bytes.into(), exec))).collect();
        db.ws = Some(Workspace::new(&db, Arc::new(sources), Arc::new(blobs), Arc::default()));
        db
    }
}

#[salsa::interned]
pub struct Slot<'db> {
    #[returns(ref)]
    pub entity: EntPath,
    #[returns(ref)]
    pub comp: Arc<str>,
}

#[salsa::tracked(returns(ref))]
pub fn parse(db: &dyn Db, src: Source) -> Result<Vec<Stmt>, Diag> {
    parser::parse(src.text(db), file_id(db, src))
}

fn file_id(db: &dyn Db, src: Source) -> u32 {
    db.ws().sources(db).iter().position(|s| *s == src).expect("source is in the workspace") as u32
}

#[salsa::tracked(returns(clone))]
pub fn contribs(db: &dyn Db, src: Source) -> Arc<FileContribs> {
    let file = file_id(db, src);
    Arc::new(match parse(db, src) {
        Ok(stmts) => lower::lower_file(db, file, stmts),
        Err(d) => FileContribs { diags: vec![d.clone()], ..FileContribs::default() },
    })
}

#[salsa::tracked(returns(ref))]
pub fn blob_hash(db: &dyn Db, blob: Blob) -> [u8; 32] {
    *blake3::hash(blob.bytes(db)).as_bytes()
}

#[salsa::interned]
pub struct DirKey<'db> {
    #[returns(ref)]
    pub path: String,
}

/// The files under a directory, for `tree()`: (relative path, exec, blake3).
pub fn tree_entries(db: &dyn Db, dir: String) -> Arc<Vec<(String, bool, [u8; 32])>> {
    tree_listing(db, DirKey::new(db, dir))
}

#[salsa::tracked(returns(clone))]
fn tree_listing<'db>(db: &'db dyn Db, dir: DirKey<'db>) -> Arc<Vec<(String, bool, [u8; 32])>> {
    let prefix = format!("{}/", dir.path(db).trim_end_matches('/'));
    let blobs = db.ws().blobs(db);
    Arc::new(
        blobs
            .range(prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(&prefix))
            .map(|(k, b)| (k[prefix.len()..].to_string(), b.exec(db), *blob_hash(db, *b)))
            .collect(),
    )
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntInfo {
    /// Prefabs this entity inherits from, directly or through its parent.
    pub bases: BTreeSet<EntPath>,
    pub children: BTreeSet<Arc<str>>,
    /// Components set here or on a base.
    pub comps: BTreeSet<Arc<str>>,
}

/// The structure of the whole config. No spans and no values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Index {
    pub entities: BTreeMap<EntPath, EntInfo>,
    /// Which origins contribute to each (entity, component).
    pub slots: BTreeMap<(EntPath, Arc<str>), BTreeSet<Origin>>,
    /// Component types by scope: `""` holds the global ones (core and the
    /// user's files), every other key is an app module's own.
    pub decls: BTreeMap<Arc<str>, BTreeMap<Arc<str>, Ty>>,
    /// The app modules (`<stdlib>/<app>.cfg`): an entity named after one is
    /// that app, and holds its config.
    pub apps: BTreeSet<Arc<str>>,
    pub rules: Vec<(u32, u32)>,
}

static ANY: Ty = Ty::Any;

/// What the declarations say about the value at a path.
pub enum Typed<'a> {
    /// The value there must have one of these types.
    Types(Vec<&'a Ty>),
    /// No type admits the key at `at` (an index into the path).
    Unknown { at: usize, message: String },
    /// No declaration covers the path.
    Undeclared,
}

impl Index {
    pub fn has(&self, e: &[Arc<str>], comp: &str) -> bool {
        self.entities.get(e).is_some_and(|i| i.comps.contains(comp))
    }

    /// Whether `name` on the app entity `app` is one of the module's own
    /// fields (which it translates), not one of the app's keys.
    pub fn strata_field(&self, app: &str, name: &str) -> bool {
        name != app && self.decls.get(app).is_some_and(|d| d.contains_key(name))
    }

    /// The type at `path`: an entity's path, or an entity's path and a
    /// component name. Under an app entity, a key the module declares is its
    /// own field and that declaration types the field's whole subtree; any
    /// other key is the app's own, typed by the declaration named after the
    /// app (`component claude: claude-settings`), else anything. Elsewhere
    /// the first name along the path with a global declaration types the
    /// rest. A prefab binds where it is used, so it is not checked itself.
    pub fn typed<'a>(&'a self, path: &[Arc<str>]) -> Typed<'a> {
        let Some(root) = path.first() else { return Typed::Types(vec![&ANY]) };
        let names: Vec<&str> = path.iter().map(|s| s.as_ref()).collect();
        let descend = |t: &'a Ty, from: usize| match t.descend(&names[from..]) {
            Ok(ts) => Typed::Types(ts),
            Err((i, message)) => Typed::Unknown { at: from + i, message },
        };
        if root.as_ref() == PREFAB {
            return Typed::Types(vec![&ANY]);
        }
        if self.apps.contains(root) {
            let own = self.decls.get(root.as_ref());
            if let Some(field) = names.get(1)
                && self.strata_field(root, field)
                && let Some(t) = own.and_then(|d| d.get(*field))
            {
                return descend(t, 2);
            }
            return match own.and_then(|d| d.get(root.as_ref())) {
                Some(t) => descend(t, 1),
                None => Typed::Types(vec![&ANY]),
            };
        }
        let global = self.decls.get("");
        for (i, name) in names.iter().enumerate() {
            if let Some(t) = global.and_then(|d| d.get(*name)) {
                return descend(t, i + 1);
            }
        }
        Typed::Undeclared
    }

    /// The global names, for "did you mean".
    fn visible(&self) -> Vec<&str> {
        self.decls.get("").map(|d| d.keys().map(|k| k.as_ref()).collect()).unwrap_or_default()
    }
}

/// The app module a source declares components for: `<stdlib>/zed.cfg` is
/// `zed`. Core, `<stdlib>/schemas/*` and the user's files are global.
pub fn module_of(path: &str) -> &str {
    match path.strip_prefix("<stdlib>/").and_then(|p| p.strip_suffix(".cfg")) {
        Some(m) if m != "core" && !m.contains('/') => m,
        _ => "",
    }
}

/// Structure from base files (no rules): everything outside `files` is final here.
#[salsa::tracked(returns(clone))]
pub fn base_index(db: &dyn Db) -> Arc<Index> {
    let ws = db.ws();
    let mut idx = Index::default();
    let mut direct: BTreeMap<EntPath, BTreeSet<EntPath>> = BTreeMap::new();
    let mut aliases = BTreeMap::new();
    let mut decls = Vec::new();
    for (file, src) in ws.sources(db).iter().enumerate() {
        let c = contribs(db, *src);
        let file = file as u32;
        for (path, bases, _) in &c.entities {
            add_entity(&mut idx, path);
            direct.entry(path.clone()).or_default().extend(bases.iter().cloned());
        }
        for key in c.slots.keys() {
            idx.slots.entry(key.clone()).or_default().insert(Origin::Base(file));
        }
        for (name, ty, _) in &c.types {
            aliases.insert(name.to_string(), ty.clone());
        }
        let module: Arc<str> = module_of(src.path(db)).into();
        if !module.is_empty() {
            idx.apps.insert(module.clone());
        }
        decls.extend(c.decls.iter().map(|(n, t, _)| (module.clone(), n.clone(), t.clone())));
        for (i, _) in c.rules.iter().enumerate() {
            idx.rules.push((file, i as u32));
        }
        for f in &c.facts {
            if let Ok(table) = lower::fact_table(db, f) {
                for name in table.keys() {
                    let path: EntPath = [f.name.clone(), Arc::from(name.as_str())].into();
                    add_entity(&mut idx, &path);
                    idx.slots.entry((path, f.comp.clone())).or_default().insert(Origin::Base(file));
                }
            }
        }
    }
    for (module, name, ty) in decls {
        if let Ok(t) = Ty::from_expr(&ty, &aliases) {
            idx.decls.entry(module).or_default().insert(name, t);
        }
    }
    let slot_entities: Vec<EntPath> = idx.slots.keys().map(|(e, _)| e.clone()).collect();
    for e in slot_entities {
        add_entity_owned(&mut idx, e);
    }
    for ((e, c), _) in idx.slots.clone() {
        idx.entities.get_mut(&e).unwrap().comps.insert(c);
    }
    close_bases(&mut idx, &direct);
    Arc::new(idx)
}

fn add_entity(idx: &mut Index, path: &EntPath) {
    add_entity_owned(idx, path.clone());
}

fn add_entity_owned(idx: &mut Index, path: EntPath) {
    for n in 0..=path.len() {
        let prefix: EntPath = path[..n].into();
        let info = idx.entities.entry(prefix).or_default();
        if n < path.len() {
            info.children.insert(path[n].clone());
        }
    }
}

/// Give every entity the bases it inherits through its parent, and the
/// children and components of its bases. Bases resolve first, so a prefab
/// built from another prefab is complete before anything inherits from it.
fn close_bases(idx: &mut Index, direct: &BTreeMap<EntPath, BTreeSet<EntPath>>) {
    fn visit(
        e: &EntPath,
        idx: &mut Index,
        direct: &BTreeMap<EntPath, BTreeSet<EntPath>>,
        done: &mut BTreeSet<EntPath>,
        active: &mut BTreeSet<EntPath>,
    ) {
        if done.contains(e) || !active.insert(e.clone()) {
            return;
        }
        let mut bases: BTreeSet<EntPath> = direct.get(e).cloned().unwrap_or_default();
        if let Some((last, parent)) = e.split_last() {
            let parent: EntPath = parent.into();
            visit(&parent, idx, direct, done, active);
            let inherited: Vec<EntPath> =
                idx.entities.get(&parent).map(|i| i.bases.iter().cloned().collect()).unwrap_or_default();
            for b in inherited {
                let child: EntPath = b.iter().cloned().chain([last.clone()]).collect();
                if idx.entities.contains_key(&child) {
                    bases.insert(child);
                }
            }
        }
        bases.retain(|b| b != e);
        for b in &bases {
            visit(b, idx, direct, done, active);
        }
        let mut children = BTreeSet::new();
        let mut comps = BTreeSet::new();
        for b in &bases {
            if let Some(info) = idx.entities.get(b) {
                children.extend(info.children.iter().cloned());
                comps.extend(info.comps.iter().cloned());
            }
        }
        let info = idx.entities.entry(e.clone()).or_default();
        info.bases = bases;
        info.comps.extend(comps);
        let new: Vec<Arc<str>> = children.into_iter().filter(|c| info.children.insert(c.clone())).collect();
        active.remove(e);
        done.insert(e.clone());
        for c in new {
            let child: EntPath = e.iter().cloned().chain([c]).collect();
            idx.entities.entry(child.clone()).or_default();
            visit(&child, idx, direct, done, active);
        }
    }
    let mut done = BTreeSet::new();
    let mut active = BTreeSet::new();
    let all: Vec<EntPath> = idx.entities.keys().cloned().collect();
    for e in all {
        visit(&e, idx, direct, &mut done, &mut active);
    }
}

#[salsa::tracked(returns(clone))]
pub fn rule_out(db: &dyn Db, file: u32, idx: u32) -> Arc<FileContribs> {
    let src = db.ws().sources(db)[file as usize];
    let c = contribs(db, src);
    Arc::new(lower::lower_rule(db, &c.rules[idx as usize]))
}

/// The base index plus every entity a rule creates under `files`.
#[salsa::tracked(returns(clone))]
pub fn full_index(db: &dyn Db) -> Arc<Index> {
    let base = base_index(db);
    let mut idx = (*base).clone();
    for &(file, i) in &base.rules {
        let out = rule_out(db, file, i);
        for (path, _, _) in &out.entities {
            add_entity(&mut idx, path);
        }
        for (e, c) in out.slots.keys() {
            add_entity(&mut idx, e);
            idx.entities.get_mut(e).unwrap().comps.insert(c.clone());
            idx.slots.entry((e.clone(), c.clone())).or_default().insert(Origin::Rule(file, i));
        }
    }
    Arc::new(idx)
}

/// The index a slot reads: only `files` sees rule output.
pub fn index_for(db: &dyn Db, e: &[Arc<str>]) -> Arc<Index> {
    if e.first().is_some_and(|s| s.as_ref() == FILES) { full_index(db) } else { base_index(db) }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub result: Result<Merged, Diag>,
    /// Every leaf that took part, for `explain`.
    pub leaves: Vec<Leaf>,
}

fn fail(span: Span, msg: impl Into<String>) -> Arc<Resolved> {
    Arc::new(Resolved { result: Err(Diag::new(span, msg)), leaves: vec![] })
}

#[salsa::tracked(returns(clone), cycle_result = resolved_cycle)]
pub fn resolved<'db>(db: &'db dyn Db, slot: Slot<'db>) -> Arc<Resolved> {
    let entity = slot.entity(db).clone();
    let comp = slot.comp(db).clone();
    let idx = index_for(db, &entity);
    let own = thunks(db, &idx, &entity, &comp);
    let span = own.first().map(|t| t.span).unwrap_or_default();
    let path: Vec<Arc<str>> = entity.iter().cloned().chain([comp.clone()]).collect();
    let types = match idx.typed(&path) {
        Typed::Types(ts) => ts,
        // A key that leaves its type above the component is the entity's
        // error, reported once where the entity is opened (`check`).
        Typed::Unknown { at, .. } if at + 1 < path.len() => vec![&ANY],
        Typed::Unknown { message, .. } => return fail(span, format!("{}[{comp}]: {message}", show_ent(&entity))),
        Typed::Undeclared => {
            let known = idx.visible();
            let hint =
                crate::types::closest(&comp, &known).map(|c| format!("; did you mean `{c}`?")).unwrap_or_default();
            return fail(span, format!("unknown component `{comp}`{hint}"));
        }
    };
    let mut leaves = Vec::new();
    let ev = Eval::new(Some(db));
    for t in &own {
        match ev.thunk_leaf(t) {
            Ok(l) => leaves.push(l),
            Err(Fault::Fail(d) | Fault::Missing(d)) => return Arc::new(Resolved { result: Err(d), leaves }),
        }
    }
    if let Some(d) = single_owner(&own) {
        return Arc::new(Resolved { result: Err(d), leaves });
    }
    let bases = idx.entities.get(&entity).map(|i| i.bases.clone()).unwrap_or_default();
    for b in bases {
        if !idx.has(&b, &comp) {
            continue;
        }
        let r = resolved(db, Slot::new(db, b.clone(), comp.clone()));
        match &r.result {
            Ok(m) => {
                let via: Arc<str> = show_ent(&b).into();
                leaves.extend(m.winners.iter().map(|l| Leaf {
                    level: Level::Inherited,
                    via: Some(via.clone()),
                    ..l.clone()
                }));
            }
            Err(d) => return Arc::new(Resolved { result: Err(d.clone()), leaves }),
        }
    }
    // A component is one value: the highest level replaces the rest whole,
    // and only a set unions what its writers give at that level.
    let set = SetShape(types.iter().any(|t| matches!(t, Ty::Set(_))));
    let result = match merge::merge(leaves.clone(), &set) {
        Ok(m) if types.iter().any(|t| t.check_path(&m.value).is_ok()) => Ok(m),
        Ok(m) => {
            let one;
            let ty = match types.as_slice() {
                [t] => *t,
                ts => {
                    one = Ty::Union(ts.iter().map(|t| (*t).clone()).collect());
                    &one
                }
            };
            let e = ty.check_path(&m.value).expect_err("no type admits the value");
            let span = m.winners.first().map(|l| l.span).unwrap_or(span);
            let at = if e.path.is_empty() { String::new() } else { format!(" at {}", merge::show_path(&e.path)) };
            Err(Diag::new(span, format!("{}[{comp}]{at}: {}", show_ent(&entity), e.message)))
        }
        Err(e) => {
            let [a, b] = e.spans();
            let mut d = Diag::new(a.span, format!("{}[{comp}]: {}", show_ent(&entity), e.message()));
            d = d.note(b.span, "the other value is here");
            Err(d)
        }
    };
    Arc::new(Resolved { result, leaves })
}

struct SetShape(bool);

impl merge::Shape for SetShape {
    fn is_set(&self, path: &[String]) -> bool {
        self.0 && path.is_empty()
    }
}

fn resolved_cycle<'db>(db: &'db dyn Db, _id: salsa::Id, slot: Slot<'db>) -> Arc<Resolved> {
    fail(Span::default(), format!("{}[{}] depends on itself", show_ent(slot.entity(db)), slot.comp(db)))
}

/// A component set inside an `if` must have one owner, so flipping the
/// condition can't silently hand the value to another writer.
fn single_owner(own: &[Thunk]) -> Option<Diag> {
    let cond = own.iter().find(|t| t.cond)?;
    let other = own.iter().find(|t| t.span != cond.span)?;
    Some(
        Diag::new(cond.span, "a component set under `if` must have a single owner")
            .note(other.span, "it is also set here"),
    )
}

pub fn thunks(db: &dyn Db, idx: &Index, entity: &EntPath, comp: &Arc<str>) -> Vec<Thunk> {
    let key = (entity.clone(), comp.clone());
    let Some(origins) = idx.slots.get(&key) else { return vec![] };
    let sources = db.ws().sources(db);
    let mut out = Vec::new();
    for o in origins {
        let c = match *o {
            Origin::Base(file) => contribs(db, sources[file as usize]),
            Origin::Rule(file, i) => rule_out(db, file, i),
        };
        if let Some(ts) = c.slots.get(&key) {
            out.extend(ts.iter().cloned());
        }
        if let Origin::Base(_) = o {
            for f in &c.facts {
                if *f.name == *entity[0]
                    && entity.len() == 2
                    && f.comp == *comp
                    && let Ok(table) = lower::fact_table(db, f)
                    && let Some(v) = table.get(entity[1].as_ref())
                {
                    out.push(Thunk::literal(v.clone(), f.span));
                }
            }
        }
    }
    out
}

#[salsa::tracked(returns(clone), cycle_result = value_cycle)]
pub fn value<'db>(db: &'db dyn Db, slot: Slot<'db>) -> Result<Value, Diag> {
    resolved(db, slot).result.clone().map(|m| m.value)
}

fn value_cycle<'db>(db: &'db dyn Db, _id: salsa::Id, slot: Slot<'db>) -> Result<Value, Diag> {
    Err(Diag::new(Span::default(), format!("{}[{}] depends on itself", show_ent(slot.entity(db)), slot.comp(db))))
}

/// Every diagnostic: parse and structure errors, then every slot's value.
pub fn check(db: &dyn Db) -> Vec<Diag> {
    let ws = db.ws();
    let mut diags = Vec::new();
    let mut aliases = BTreeMap::new();
    for src in ws.sources(db).iter() {
        let c = contribs(db, *src);
        diags.extend(c.diags.iter().cloned());
        for (name, ty, _) in &c.types {
            aliases.insert(name.to_string(), ty.clone());
        }
    }
    let base = base_index(db);
    // One name per scope: two apps may each declare `bindings`.
    let mut declared: BTreeMap<(&str, Arc<str>), Span> = BTreeMap::new();
    let mut opened: BTreeMap<EntPath, Span> = BTreeMap::new();
    let mut set: BTreeMap<(EntPath, Arc<str>), Span> = BTreeMap::new();
    for src in ws.sources(db).iter() {
        let c = contribs(db, *src);
        let module = module_of(src.path(db));
        for (path, _, span) in &c.entities {
            for n in 1..=path.len() {
                opened.entry(path[..n].into()).or_insert(*span);
            }
        }
        for (key, thunks) in &c.slots {
            if let Some(t) = thunks.first() {
                set.entry(key.clone()).or_insert(t.span);
            }
        }
        for (name, ty, span) in &c.decls {
            if let Err(e) = Ty::from_expr(ty, &aliases) {
                diags.push(Diag::new(*span, e));
            }
            // A field the module translates can't share a name with one of
            // the app's own keys: one of the two would be unreachable.
            if let Some(Ty::Record { fields, .. }) = base.decls.get(module).and_then(|d| d.get(module))
                && base.strata_field(module, name)
                && fields.contains_key(name.as_ref())
            {
                diags.push(Diag::new(
                    *span,
                    format!("`{name}` is both a field of the `{module}` module and one of {module}'s own keys"),
                ));
            }
            if let Some(prev) = declared.insert((module, name.clone()), *span) {
                diags.push(
                    Diag::new(*span, format!("component `{name}` is declared twice")).note(prev, "first declared here"),
                );
            }
        }
        for f in &c.facts {
            if let Err(d) = lower::fact_table(db, f) {
                diags.push(d);
            }
        }
    }
    // Every scope must be one its type admits, with its required keys.
    for (e, span) in &opened {
        if let Some((last, parent)) = e.split_last()
            && let Some(at) = set.get(&(parent.into(), last.clone()))
        {
            diags.push(
                Diag::new(*span, format!("`{}` is both a scope and a value: use one", show_ent(e)))
                    .note(*at, "set with `:` here"),
            );
            continue;
        }
        let msg = match base.typed(e) {
            Typed::Unknown { at, message } if at + 1 == e.len() => message,
            Typed::Types(ts) => {
                let info = base.entities.get(e);
                let present = |k: &str| info.is_some_and(|i| i.comps.contains(k) || i.children.contains(k));
                match Ty::check_scope(&ts, &present) {
                    Ok(()) => continue,
                    Err(m) => m,
                }
            }
            _ => continue,
        };
        diags.push(Diag::new(*span, format!("{}: {msg}", show_ent(e))));
    }
    for &(file, i) in &base.rules {
        diags.extend(rule_out(db, file, i).diags.iter().cloned());
    }
    let idx = full_index(db);
    for (e, c) in idx.slots.keys() {
        if let Err(d) = value(db, Slot::new(db, e.clone(), c.clone())) {
            diags.push(d);
        }
    }
    diags.sort_by(|a, b| a.span.cmp(&b.span).then(a.message.cmp(&b.message)));
    diags.dedup();
    diags
}

pub struct SourceMap<'a> {
    pub db: &'a dyn Db,
}

impl Sources for SourceMap<'_> {
    fn path(&self, file: u32) -> String {
        match self.db.ws().sources(self.db).get(file as usize) {
            Some(s) => s.path(self.db).clone(),
            None => "<command line>".into(),
        }
    }

    fn text(&self, file: u32) -> Option<&str> {
        let s = self.db.ws().sources(self.db).get(file as usize)?;
        Some(s.text(self.db).as_ref())
    }
}

/// Leaves of a resolved slot that won, for display.
pub fn is_winner(m: &Merged, l: &Leaf) -> bool {
    m.winners.iter().any(|w| w.path == l.path && w.span == l.span && w.level == l.level && w.value == l.value)
}

pub fn leaf_text(l: &Leaf) -> String {
    match &l.value {
        LeafValue::Scalar(v) => v.to_string(),
        LeafValue::Record => "{}".into(),
    }
}
