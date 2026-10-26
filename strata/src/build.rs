//! Loading a config into the database, and turning `files.*` into units.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use salsa::Setter;

use crate::db::{self, Blob, Db, FILES, RootDb, Slot, Source};
use crate::eval::normalize;
use crate::store::{Content, Kind, Unit};
use crate::syntax::{Diag, Sources, Span};
use crate::value::Value;

/// The standard library, compiled in. It has no powers user files lack.
pub const STDLIB: &[(&str, &str)] = &[
    ("core.cfg", include_str!("../stdlib/core.cfg")),
    ("schemas/claude-settings.cfg", include_str!("../stdlib/schemas/claude-settings.cfg")),
    ("aerospace.cfg", include_str!("../stdlib/aerospace.cfg")),
    ("claude.cfg", include_str!("../stdlib/claude.cfg")),
    ("cursor.cfg", include_str!("../stdlib/cursor.cfg")),
    ("ghostty.cfg", include_str!("../stdlib/ghostty.cfg")),
    ("git.cfg", include_str!("../stdlib/git.cfg")),
    ("ssh.cfg", include_str!("../stdlib/ssh.cfg")),
    ("starship.cfg", include_str!("../stdlib/starship.cfg")),
    ("starship-presets.cfg", include_str!("../stdlib/starship-presets.cfg")),
    ("tmux.cfg", include_str!("../stdlib/tmux.cfg")),
    ("zed.cfg", include_str!("../stdlib/zed.cfg")),
];

pub struct Root {
    /// As given on the command line, normalized: what diagnostics show.
    pub shown: PathBuf,
    pub abs: PathBuf,
}

pub struct Loaded {
    pub db: RootDb,
    pub roots: Vec<Root>,
}

/// `optional` directories join the config when they exist and are skipped
/// when they don't, so a public checkout builds without its private half.
pub fn load(configs: &[PathBuf], optional: &[PathBuf], facts: &[(String, PathBuf)]) -> Result<Loaded> {
    let mut sources = Vec::new();
    for (name, text) in STDLIB {
        sources.push((format!("<stdlib>/{name}"), "<stdlib>".to_string(), text.to_string()));
    }
    let mut blobs = BTreeMap::new();
    let mut roots = Vec::new();
    for c in configs.iter().chain(optional.iter().filter(|o| o.exists())) {
        let abs = fs::canonicalize(c).with_context(|| format!("config directory {}", c.display()))?;
        let shown = normalize(c);
        for file in walk(&abs)? {
            let rel = file.strip_prefix(&abs).unwrap();
            if file.extension().is_some_and(|e| e == "cfg") {
                let text = fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
                let dir = file.parent().unwrap().to_string_lossy().to_string();
                sources.push((shown.join(rel).to_string_lossy().to_string(), dir, text));
            } else {
                blobs.insert(file.to_string_lossy().to_string(), read_blob(&file)?);
            }
        }
        roots.push(Root { shown, abs });
    }
    let mut db = RootDb::new(sources, blobs);
    let mut table = BTreeMap::new();
    for (name, path) in facts {
        let text = fs::read_to_string(path).with_context(|| format!("facts file {}", path.display()))?;
        table.insert(name.clone(), Arc::from(text.as_str()));
    }
    let ws = db.ws();
    ws.set_facts(&mut db).to(Arc::new(table));
    Ok(Loaded { db, roots })
}

pub fn read_blob(file: &Path) -> Result<(Vec<u8>, bool)> {
    let bytes = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let exec = fs::metadata(file)?.permissions().mode() & 0o111 != 0;
    Ok((bytes, exec))
}

/// Every file under `dir`, skipping hidden entries (editor swap files, .git).
pub fn walk(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).with_context(|| format!("listing {}", d.display()))? {
            let e = e?;
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let p = e.path();
            if fs::metadata(&p)?.is_dir() { stack.push(p) } else { out.push(p) }
        }
    }
    out.sort();
    Ok(out)
}

impl Loaded {
    /// Apply one changed path from the watcher. Returns whether anything changed.
    pub fn refresh(&mut self, path: &Path) -> Result<bool> {
        if path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
            return Ok(false);
        }
        let Some(root) = self.roots.iter().find(|r| path.starts_with(&r.abs)) else { return Ok(false) };
        let shown = root.shown.join(path.strip_prefix(&root.abs).unwrap()).to_string_lossy().to_string();
        let ws = self.db.ws();
        let exists = fs::metadata(path).is_ok_and(|m| m.is_file());
        if path.extension().is_some_and(|e| e == "cfg") {
            let sources = ws.sources(&self.db).clone();
            let found = sources.iter().position(|s| *s.path(&self.db) == shown);
            match (found, exists) {
                (Some(i), true) => {
                    let text = fs::read_to_string(path)?;
                    if *sources[i].text(&self.db).as_ref() == *text {
                        return Ok(false);
                    }
                    sources[i].set_text(&mut self.db).to(text.into());
                }
                (Some(i), false) => {
                    let mut next = (*sources).clone();
                    next.remove(i);
                    ws.set_sources(&mut self.db).to(Arc::new(next));
                }
                (None, true) => {
                    let text = fs::read_to_string(path)?;
                    let dir = path.parent().unwrap().to_string_lossy().to_string();
                    let src = Source::new(&self.db, shown, dir, text.into());
                    let mut next = (*sources).clone();
                    next.push(src);
                    ws.set_sources(&mut self.db).to(Arc::new(next));
                }
                (None, false) => return Ok(false),
            }
            return Ok(true);
        }
        let key = path.to_string_lossy().to_string();
        let blobs = ws.blobs(&self.db).clone();
        match (blobs.get(&key).copied(), exists) {
            (Some(b), true) => {
                let (bytes, exec) = read_blob(path)?;
                if **b.bytes(&self.db) == *bytes && b.exec(&self.db) == exec {
                    return Ok(false);
                }
                b.set_bytes(&mut self.db).to(bytes.into());
                b.set_exec(&mut self.db).to(exec);
            }
            (Some(_), false) => {
                let mut next = (*blobs).clone();
                next.remove(&key);
                ws.set_blobs(&mut self.db).to(Arc::new(next));
            }
            (None, true) => {
                let (bytes, exec) = read_blob(path)?;
                let mut next = (*blobs).clone();
                next.insert(key, Blob::new(&self.db, bytes.into(), exec));
                ws.set_blobs(&mut self.db).to(Arc::new(next));
            }
            (None, false) => return Ok(false),
        }
        Ok(true)
    }

    pub fn render(&self, diags: &[Diag]) -> String {
        let map = db::SourceMap { db: &self.db };
        diags.iter().map(|d| map.render(d)).collect::<Vec<_>>().join("\n")
    }

    fn abs_of(&self, shown: &str) -> Option<PathBuf> {
        let shown = Path::new(shown);
        self.roots.iter().find_map(|r| shown.strip_prefix(&r.shown).ok().map(|rest| r.abs.join(rest)))
    }

    /// Every `files."<target>"[file]` as a unit to materialize.
    pub fn units(&self) -> Result<Vec<Unit>, Vec<Diag>> {
        let db: &dyn Db = &self.db;
        let idx = db::full_index(db);
        let files: db::EntPath = [Arc::from(FILES)].into();
        let Some(info) = idx.entities.get(&files) else { return Ok(vec![]) };
        let mut units = Vec::new();
        let mut diags = Vec::new();
        for target in &info.children {
            let path: db::EntPath = [files[0].clone(), target.clone()].into();
            if !idx.has(&path, "file") {
                continue;
            }
            let slot = Slot::new(db, path.clone(), Arc::from("file"));
            let span = db::resolved(db, slot).leaves.first().map(|l| l.span).unwrap_or_default();
            match db::value(db, slot) {
                Ok(v) => match self.unit(target, &v) {
                    Ok(u) => units.push(u),
                    Err(msg) => diags.push(Diag::new(span, format!("files.{target:?}: {msg}"))),
                },
                Err(d) => diags.push(d),
            }
        }
        if diags.is_empty() { Ok(units) } else { Err(diags) }
    }

    fn unit(&self, target: &str, v: &Value) -> Result<Unit, String> {
        let r = v.as_record().ok_or("`file` must be a record")?;
        let get = |k: &str| r.get(k).and_then(Value::as_str);
        let kind = match get("kind") {
            Some("copy") => Kind::Copy,
            _ => Kind::Link,
        };
        let exec = get("mode") == Some("0755");
        let content = if let Some(text) = get("content") {
            Content::File { bytes: text.as_bytes().into(), exec }
        } else if let Some(hash) = get("blob") {
            let from = get("from").ok_or("blob without `from`")?;
            let (bytes, exec) = self.blob_bytes(from).ok_or_else(|| format!("no file {from}"))?;
            if blake3::hash(&bytes).to_hex().as_str() != hash {
                return Err(format!("{from} changed while building"));
            }
            Content::File { bytes, exec }
        } else if get("tree").is_some() {
            let from = get("from").ok_or("tree without `from`")?;
            let abs = self.abs_of(from).ok_or_else(|| format!("no directory {from}"))?;
            let listing = db::tree_entries(&self.db, abs.to_string_lossy().to_string());
            let mut entries = Vec::new();
            for (rel, _, _) in listing.iter() {
                let (bytes, exec) =
                    self.blob_bytes_abs(&abs.join(rel)).ok_or_else(|| format!("{from}/{rel} vanished"))?;
                entries.push((rel.clone(), bytes, exec));
            }
            Content::Tree { entries }
        } else if let Some(path) = get("symlink") {
            if !path.starts_with('/') {
                return Err(format!("symlink target {path:?} must be absolute"));
            }
            Content::External { path: path.to_string() }
        } else {
            return Err("`file` needs `content`, `blob`, `tree` or `symlink`".into());
        };
        Ok(Unit { target: target.to_string(), kind, content })
    }

    fn blob_bytes(&self, shown: &str) -> Option<(Arc<[u8]>, bool)> {
        self.blob_bytes_abs(&self.abs_of(shown)?)
    }

    fn blob_bytes_abs(&self, abs: &Path) -> Option<(Arc<[u8]>, bool)> {
        let blobs = self.db.ws().blobs(&self.db);
        let b = blobs.get(abs.to_string_lossy().as_ref())?;
        Some((b.bytes(&self.db).clone(), b.exec(&self.db)))
    }

    /// `strata explain`: every contribution to a slot, and which one won.
    pub fn explain(&self, address: &str) -> Result<String> {
        use crate::syntax::ast::ExprKind;
        let expr =
            crate::syntax::parser::parse_address(address).map_err(|d| anyhow::anyhow!("bad address: {}", d.message))?;
        let mut fields = Vec::new();
        let mut cur = &expr;
        while let ExprKind::Field(base, name) = &cur.kind {
            fields.push(name.clone());
            cur = base;
        }
        fields.reverse();
        let db: &dyn Db = &self.db;
        let ev = crate::eval::Eval::new(Some(db));
        let env = crate::eval::Env::default();
        let field_path: Vec<String> = fields
            .iter()
            .map(|n| ev.name(n, &env).map(|s| s.to_string()))
            .collect::<Result<_, _>>()
            .map_err(|f| anyhow::anyhow!(f.diag().message))?;
        let (entity, comp) = match &cur.kind {
            ExprKind::Component(base, comp) => (entity_of(&ev, base)?, comp.name.clone()),
            _ => {
                let entity = entity_of(&ev, &expr)?;
                return Ok(self.describe_entity(&entity));
            }
        };
        let slot = Slot::new(db, entity.clone(), comp.clone());
        let r = db::resolved(db, slot);
        let map = db::SourceMap { db };
        let mut out = String::new();
        let shown = format!(
            "{}[{comp}]{}",
            db::show_ent(&entity),
            field_path.iter().map(|f| format!(".{f}")).collect::<String>()
        );
        match &r.result {
            Ok(m) => {
                let mut v = &m.value;
                for f in &field_path {
                    match v.as_record().and_then(|r| r.get(f)) {
                        Some(x) => v = x,
                        None => anyhow::bail!("{shown} is not set"),
                    }
                }
                // The component's identity: blake3 of its canonical CBOR.
                let h = m.value.hash().to_hex();
                out.push_str(&format!("{shown} = {v}\n  component hash {}\n", &h[..16]));
            }
            Err(d) => out.push_str(&map.render(d)),
        }
        let mut leaves: Vec<_> =
            r.leaves.iter().filter(|l| l.path.starts_with(&field_path) || field_path.starts_with(&l.path)).collect();
        leaves.sort_by(|a, b| a.path.cmp(&b.path).then(b.level.cmp(&a.level)).then(a.span.cmp(&b.span)));
        let mut last_path = None;
        for l in leaves {
            if last_path != Some(&l.path) {
                out.push_str(&format!(
                    "  {}\n",
                    if l.path.is_empty() { "(whole value)".into() } else { crate::merge::show_path(&l.path) }
                ));
                last_path = Some(&l.path);
            }
            let won = r.result.as_ref().is_ok_and(|m| db::is_winner(m, l));
            let via = l.via.as_ref().map(|v| format!(" via {v}")).unwrap_or_default();
            out.push_str(&format!(
                "    {} {:9} {}{via}  {}\n",
                if won { "*" } else { " " },
                l.level.to_string(),
                where_(&map, l.span),
                db::leaf_text(l)
            ));
        }
        out.push_str("  * = decides the value: the highest level wins; equal values at one level agree, different ones are an error\n");
        Ok(out)
    }

    fn describe_entity(&self, e: &db::EntPath) -> String {
        let idx = db::index_for(&self.db, e);
        let Some(info) = idx.entities.get(e) else { return format!("no entity {}\n", db::show_ent(e)) };
        let mut out = format!("{}\n", db::show_ent(e));
        for b in &info.bases {
            out.push_str(&format!("  inherits {}\n", db::show_ent(b)));
        }
        for c in &info.comps {
            out.push_str(&format!("  [{c}]\n"));
        }
        for c in &info.children {
            out.push_str(&format!("  .{}\n", crate::merge::show_path(&[c.to_string()])));
        }
        out
    }
}

fn where_(map: &db::SourceMap, span: Span) -> String {
    if span == Span::default() { "<unknown>".into() } else { map.location(span) }
}

fn entity_of(ev: &crate::eval::Eval, e: &crate::syntax::ast::Expr) -> Result<db::EntPath> {
    use crate::syntax::ast::ExprKind;
    let ExprKind::Path(names) = &e.kind else { anyhow::bail!("expected an entity path like zed or files.\"~/.x\"") };
    let env = crate::eval::Env::default();
    let mut parts: Vec<Arc<str>> = Vec::new();
    for n in names {
        parts.push(ev.name(n, &env).map_err(|f| anyhow::anyhow!(f.diag().message))?);
    }
    Ok(parts.into())
}
