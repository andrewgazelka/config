//! The materializer.
//!
//! Phase 1 (realize, pure): every unit's bytes go into a content-addressed
//! store and the units form one generation, named by a single hash.
//!
//! Phase 2 (switch): each unit (a file or a directory tree) is swapped in
//! with one rename(): a symlink into the store, or a copy for apps that
//! replace symlinks. The `current` pointer is written last; a crash
//! mid-switch leaves the old generation recorded and the next run
//! converges.
//!
//! Durability: nothing is flushed file by file. Every object is a pure
//! function of the config, which lives in git, so a crash only has to be
//! noticed, not prevented: each run checks the bytes of every object it
//! links (once per process) and rewrites any that are short or wrong, and
//! the ownership rule treats a torn copy of strata's own file as strata's.
//! On macOS one ordering point per generation (`F_BARRIERFSYNC`), after
//! the objects and before anything links to them, keeps the objects of
//! every recorded generation intact for `rollback`; Linux has none (see
//! `barrier`). `--durable` restores a full flush of every file instead.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::document::{Doc, Drift, Format};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Copy,
    Link,
    /// A JSON or TOML file the app writes too: strata owns the keys it
    /// declares and merges them into the file on disk (see `merge`).
    Merge,
}

#[derive(Clone, Debug)]
pub enum Content {
    File {
        bytes: Arc<[u8]>,
        exec: bool,
    },
    Tree {
        entries: Vec<(String, Arc<[u8]>, bool)>,
    },
    /// A symlink to a path strata does not store, such as a Nix store path.
    External {
        path: String,
    },
}

#[derive(Clone, Debug)]
pub struct Unit {
    /// `~/...`, as written in the config.
    pub target: String,
    pub kind: Kind,
    pub content: Content,
}

/// One unit as recorded in a generation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub kind: Kind,
    /// Store object: `<blake3>` for a file, `<blake3>.d` for a tree, or an
    /// absolute path for an external link.
    pub object: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exec: bool,
}

/// What a generation was built from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inputs {
    /// Every config directory, absolute.
    #[serde(default)]
    pub layers: Vec<String>,
    /// Fact tables read from outside the config: name -> blake3 of the
    /// table, or `missing`.
    #[serde(default)]
    pub facts: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Generation {
    #[serde(flatten)]
    pub inputs: Inputs,
    pub units: BTreeMap<String, Entry>,
}

/// A switch that would leave out a config layer the current generation
/// was built from, and so remove or change the files that layer makes.
#[derive(Debug)]
pub struct DropsLayer {
    pub layers: Vec<String>,
    /// Targets the switch would remove.
    pub removes: Vec<String>,
}

impl std::fmt::Display for DropsLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let s = if self.layers.len() == 1 { "" } else { "s" };
        writeln!(f, "refusing to switch: the current generation was built with config layer{s} this one lacks:")?;
        for l in &self.layers {
            writeln!(f, "  {l}")?;
        }
        if self.removes.is_empty() {
            writeln!(f, "it would remove no files, but every value the layer sets would go")?;
        } else {
            writeln!(
                f,
                "it would remove {} file{}:",
                self.removes.len(),
                if self.removes.len() == 1 { "" } else { "s" }
            )?;
            for r in &self.removes {
                writeln!(f, "  {r}")?;
            }
        }
        write!(f, "if the layer is gone on purpose, pass --drop-layer <dir> naming it")
    }
}

impl std::error::Error for DropsLayer {}

impl Generation {
    pub fn hash(&self) -> String {
        let text = serde_json::to_vec(self).expect("a generation serializes");
        blake3::hash(&text).to_hex()[..32].to_string()
    }
}

pub struct Store {
    pub state: PathBuf,
    pub home: PathBuf,
    /// Flush every file to stable storage as it is written (`--durable`).
    pub durable: bool,
    /// Objects (and generation files) whose bytes this process has checked
    /// or written; each is read back at most once per process.
    verified: Mutex<HashSet<String>>,
    stats: Mutex<Stats>,
}

/// Where the last `realize` spent its time.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Reading back objects that were already on disk.
    pub verify: Duration,
    pub verified: usize,
    pub repaired: usize,
    /// The ordering point (or, with `--durable`, the flushes).
    pub order: Duration,
    pub fresh: usize,
}

impl Store {
    pub fn new(state: PathBuf, home: PathBuf, durable: bool) -> Self {
        Store { state, home, durable, verified: Mutex::default(), stats: Mutex::default() }
    }

    pub fn stats(&self) -> Stats {
        *self.stats.lock().unwrap()
    }

    fn is_verified(&self, name: &str) -> bool {
        self.verified.lock().unwrap().contains(name)
    }

    fn mark_verified(&self, name: &str) {
        self.verified.lock().unwrap().insert(name.to_string());
    }

    fn objects(&self) -> PathBuf {
        self.state.join("store")
    }

    pub fn object_path(&self, object: &str) -> PathBuf {
        // An absolute object is an external link target; join keeps it as is.
        self.objects().join(object)
    }

    pub fn target(&self, t: &str) -> Result<PathBuf> {
        match t.strip_prefix("~/") {
            Some(rest) if !rest.is_empty() && !rest.split('/').any(|c| c == ".." || c.is_empty()) => {
                Ok(self.home.join(rest))
            }
            _ => bail!("target `{t}` must be a path under `~/`"),
        }
    }

    /// Phase 1: write every object (idempotent) and the generation file.
    /// An object already on disk is read back and rewritten if its bytes
    /// are not the ones its name promises (a crash can leave it short or
    /// zeroed). Then one ordering point for everything written.
    pub fn realize(&self, units: &[Unit], inputs: Inputs) -> Result<(String, Generation)> {
        fs::create_dir_all(self.objects())?;
        fs::create_dir_all(self.state.join("generations"))?;
        let mut w = Writes::default();
        let mut generation = Generation { inputs, units: BTreeMap::new() };
        for u in units {
            let (object, exec) = match &u.content {
                Content::File { bytes, exec } => (self.put_blob(bytes, *exec, &mut w)?, *exec),
                Content::Tree { .. } | Content::External { .. } if u.kind == Kind::Merge => {
                    bail!("{}: only a JSON or TOML file merges", u.target)
                }
                Content::Tree { entries } => (self.put_tree(entries, &mut w)?, false),
                Content::External { path } => {
                    if u.kind == Kind::Copy {
                        bail!("{}: an external link can't be a copy", u.target);
                    }
                    (path.clone(), false)
                }
            };
            let entry = Entry { kind: u.kind, object, exec };
            generation.units.insert(u.target.clone(), entry);
        }
        let hash = generation.hash();
        let key = format!("generations/{hash}");
        if !self.is_verified(&key) {
            let path = self.state.join("generations").join(format!("{hash}.json"));
            let text = serde_json::to_vec_pretty(&generation)?;
            w.check(&path, &text, 0o444)?;
            self.mark_verified(&key);
        }
        crash_point("realize:before-order");
        let t = Instant::now();
        self.order(&w.fresh)?;
        *self.stats.lock().unwrap() = Stats {
            verify: w.verify,
            verified: w.verified,
            repaired: w.repaired,
            order: t.elapsed(),
            fresh: w.fresh.len(),
        };
        Ok((hash, generation))
    }

    /// The one ordering point per generation: every file written above is
    /// on the device before anything after it (links, copies, `current`).
    fn order(&self, fresh: &[PathBuf]) -> Result<()> {
        if fresh.is_empty() {
            return Ok(());
        }
        if self.durable {
            sync_all(fresh)?;
            // Make the renames durable before anything links to them.
            fs::File::open(self.objects())?.sync_all()?;
            fs::File::open(self.state.join("generations"))?.sync_all()?;
            return Ok(());
        }
        barrier(fresh).context("ordering the store's writes")
    }

    fn put_blob(&self, bytes: &[u8], exec: bool, w: &mut Writes) -> Result<String> {
        let name = blob_name(bytes, exec);
        if !self.is_verified(&name) {
            w.check(&self.object_path(&name), bytes, if exec { 0o555 } else { 0o444 })?;
            self.mark_verified(&name);
        }
        Ok(name)
    }

    fn put_tree(&self, entries: &[(String, Arc<[u8]>, bool)], w: &mut Writes) -> Result<String> {
        let mut h = blake3::Hasher::new();
        for (rel, bytes, exec) in entries {
            h.update(rel.as_bytes()).update(&[0]).update(blob_name(bytes, *exec).as_bytes()).update(&[0]);
        }
        let name = format!("{}.d", &h.finalize().to_hex()[..32]);
        if self.is_verified(&name) {
            return Ok(name);
        }
        let dir = self.object_path(&name);
        if dir.exists() {
            // Tree files are hard links to blobs, so a damaged blob damaged
            // them too, and rewriting the blob made a new inode: relink each
            // bad file, one rename each.
            for (rel, bytes, exec) in entries {
                let blob = self.object_path(&self.put_blob(bytes, *exec, w)?);
                let dest = dir.join(rel);
                let t = Instant::now();
                let ok = fs::read(&dest).is_ok_and(|b| *b == **bytes);
                w.verify += t.elapsed();
                w.verified += 1;
                if !ok {
                    fs::create_dir_all(dest.parent().unwrap())?;
                    let tmp = dest.with_file_name(format!(".{}.tmp", dest.file_name().unwrap().to_string_lossy()));
                    let _ = fs::remove_file(&tmp);
                    if fs::hard_link(&blob, &tmp).is_err() {
                        fs::copy(&blob, &tmp)?;
                    }
                    fs::rename(&tmp, &dest).with_context(|| format!("repairing {}", dest.display()))?;
                    w.repaired += 1;
                    w.fresh.push(dest);
                }
            }
            self.mark_verified(&name);
            return Ok(name);
        }
        let tmp = self.objects().join(format!(".{name}.tmp"));
        let _ = fs::remove_dir_all(&tmp);
        for (rel, bytes, exec) in entries {
            let blob = self.object_path(&self.put_blob(bytes, *exec, w)?);
            let dest = tmp.join(rel);
            fs::create_dir_all(dest.parent().unwrap())?;
            if fs::hard_link(&blob, &dest).is_err() {
                fs::copy(&blob, &dest)?;
            }
        }
        fs::rename(&tmp, &dir).with_context(|| format!("publishing tree {name}"))?;
        // The tree's own directory entries need flushing too.
        let mut dirs: BTreeSet<PathBuf> = [dir.clone()].into();
        for (rel, _, _) in entries {
            dirs.extend(Path::new(rel).ancestors().skip(1).map(|a| dir.join(a)));
        }
        w.fresh.extend(dirs);
        self.mark_verified(&name);
        Ok(name)
    }

    /// Check that every store object `g` names holds the bytes its name
    /// promises; `rollback` has no source to rewrite them from, so it
    /// refuses a damaged generation instead of linking to bad bytes.
    pub fn check_objects(&self, g: &Generation) -> Result<()> {
        let mut bad = Vec::new();
        for (target, e) in &g.units {
            if e.object.starts_with('/') || self.is_verified(&e.object) {
                continue;
            }
            let path = self.object_path(&e.object);
            let ok = if let Some(stem) = e.object.strip_suffix(".d") {
                // Same order as `put_tree`: entries sorted by their relative
                // path as a string. Dot files are repair leftovers, not entries.
                let mut files = Vec::new();
                for f in walk(&path).unwrap_or_default() {
                    if f.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                        continue;
                    }
                    let bytes = fs::read(&f)?;
                    let exec = fs::metadata(&f)?.permissions().mode() & 0o111 != 0;
                    files.push((f.strip_prefix(&path).unwrap().to_string_lossy().to_string(), blob_name(&bytes, exec)));
                }
                files.sort();
                let mut h = blake3::Hasher::new();
                for (rel, blob) in &files {
                    h.update(rel.as_bytes()).update(&[0]).update(blob.as_bytes()).update(&[0]);
                }
                !files.is_empty() && h.finalize().to_hex()[..32] == *stem
            } else {
                fs::read(&path).is_ok_and(|b| blob_name(&b, e.exec) == e.object)
            };
            if ok {
                self.mark_verified(&e.object);
            } else {
                bad.push(format!("{target}: {}", e.object));
            }
        }
        if !bad.is_empty() {
            bail!(
                "damaged store objects (a crash before they reached the disk); `strata apply` rewrites them from the config:\n  {}",
                bad.join("\n  ")
            );
        }
        Ok(())
    }

    /// The recorded generation. `current` is not fsynced as a directory
    /// entry, so after a crash it can be an older one; switch's ownership
    /// rule converges from there. A pointer whose generation is gone falls
    /// back to the newest loadable one in history.
    pub fn current(&self) -> Result<Option<(String, Generation)>> {
        let pointed = fs::read_to_string(self.state.join("current")).ok().map(|h| h.trim().to_string());
        let candidates = pointed.into_iter().chain(self.history().into_iter().rev());
        for hash in candidates {
            if let Ok(g) = self.load(&hash) {
                return Ok(Some((hash, g)));
            }
        }
        Ok(None)
    }

    pub fn load(&self, hash: &str) -> Result<Generation> {
        let path = self.state.join("generations").join(format!("{hash}.json"));
        let text = fs::read(&path).with_context(|| format!("reading generation {hash}"))?;
        Ok(serde_json::from_slice(&text)?)
    }

    pub fn history(&self) -> Vec<String> {
        fs::read_to_string(self.state.join("history"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_whitespace().nth(1).map(String::from))
            .collect()
    }

    /// What is at `target` now, relative to an entry.
    pub fn observe(&self, target: &str, entry: &Entry) -> Result<State> {
        let path = self.target(target)?;
        let want = self.object_path(&entry.object);
        Ok(match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => State::Missing,
            Err(e) => return Err(e.into()),
            Ok(m) if m.file_type().is_symlink() => {
                let to = fs::read_link(&path)?;
                if entry.kind == Kind::Link && to == want { State::Matches } else { State::Symlink(to) }
            }
            Ok(m) if m.is_file() && entry.kind == Kind::Merge => {
                if self.merge_into(target, entry, None, false).is_ok_and(|(changed, _)| !changed) {
                    State::Matches
                } else {
                    State::File
                }
            }
            Ok(m) if m.is_file() => {
                let object = self.object_path(&entry.object);
                if entry.kind == Kind::Copy && !entry.object.ends_with(".d") && same_file_content(&path, &object)? {
                    State::Matches
                } else {
                    State::File
                }
            }
            Ok(_) => State::Dir,
        })
    }

    /// Phase 2. `adopt` backs up foreign files instead of refusing. A
    /// config layer the current generation had and `next` lacks is refused
    /// (`DropsLayer`) unless `drop` names it.
    pub fn switch(&self, hash: &str, next: &Generation, adopt: bool, drop: &[String]) -> Result<SwitchReport> {
        let prev = self.current()?.map(|(_, g)| g).unwrap_or_default();
        let dropped: Vec<String> = prev
            .inputs
            .layers
            .iter()
            .filter(|l| !next.inputs.layers.contains(l) && !drop.contains(l))
            .cloned()
            .collect();
        if !dropped.is_empty() {
            let removes = prev.units.keys().filter(|t| !next.units.contains_key(*t)).cloned().collect();
            return Err(DropsLayer { layers: dropped, removes }.into());
        }
        self.check_objects(next)?;
        let mut report = SwitchReport::default();
        let mut todo = Vec::new();
        for (target, entry) in &next.units {
            let state = self.observe(target, entry)?;
            if entry.kind == Kind::Merge {
                // The app's file is welcome: only what isn't a document strata
                // can merge into is refused.
                let bad = match &state {
                    State::Missing | State::Matches => None,
                    State::File => self.merge_into(target, entry, None, false).err().map(|e| format!("{e:#}")),
                    s if self.owned(target, s) => None,
                    s => Some(format!("{} (not made by strata)", s.describe())),
                };
                match bad {
                    None => todo.push((target, entry, false)),
                    Some(_) if adopt => todo.push((target, entry, true)),
                    Some(why) => report.refused.push(format!("{target}: {why}")),
                }
                continue;
            }
            let owned = self.owned(target, &state) || self.matches(&prev, target);
            match state {
                State::Matches => {}
                State::Missing => todo.push((target, entry, false)),
                _ if owned => todo.push((target, entry, false)),
                _ if adopt => todo.push((target, entry, true)),
                s => report.refused.push(format!("{target}: {} (not made by strata)", s.describe())),
            }
        }
        let mut blocked = BTreeSet::new();
        for target in next.units.keys() {
            if let Some(parent) = self.blocked_parent(target)? {
                blocked.insert(parent);
            }
        }
        if !adopt {
            report.refused.extend(blocked.iter().map(|p| format!("{p}: cannot hold files (not made by strata)")));
        }
        if !report.refused.is_empty() {
            bail!("refusing to replace files strata does not own:\n  {}", report.refused.join("\n  "));
        }
        for parent in &blocked {
            self.back_up(hash, parent)?;
            report.backed_up.push(parent.clone());
        }
        for (i, (target, entry, backup)) in todo.into_iter().enumerate() {
            if backup {
                self.back_up(hash, target)?;
                report.backed_up.push(target.clone());
            }
            if entry.kind == Kind::Merge {
                let (changed, drift) = self.merge_into(target, entry, prev.units.get(target), true)?;
                if changed {
                    report.changed.push(target.clone());
                }
                report.drift.extend(drift.into_iter().map(|d| (target.clone(), d)));
                continue;
            }
            self.swap(target, entry)?;
            report.changed.push(target.clone());
            if crash_armed() {
                crash_point(&format!("switch:swap:{}", i + 1));
            }
        }
        for (target, old) in &prev.units {
            if next.units.contains_key(target) {
                continue;
            }
            if old.kind == Kind::Merge {
                // The file stays the app's; only strata's keys leave it.
                if self.unmerge(target, old)? {
                    report.removed.push(target.clone());
                } else {
                    report.kept.push(target.clone());
                }
                continue;
            }
            if self.observe(target, old)? == State::Matches {
                let path = self.target(target)?;
                if old.object.ends_with(".d") && old.kind == Kind::Copy {
                    fs::remove_dir_all(&path)?;
                } else {
                    fs::remove_file(&path)?;
                }
                report.removed.push(target.clone());
            } else {
                report.kept.push(target.clone());
            }
        }
        // The commit point: everything above is idempotent and converges on rerun.
        // It comes after the ordering point in `realize`, so a `current` that
        // survives a crash names a generation whose objects did too. No
        // directory fsync even with --durable (it costs a journal commit): a
        // lost rename leaves the old pointer, which `owned` already tolerates.
        crash_point("switch:before-pointer");
        let pointer = format!("{hash}\n");
        if self.durable {
            write_atomic(&self.state.join("current"), pointer.as_bytes(), 0o644, false)?;
        } else {
            write_file(&self.state.join("current"), pointer.as_bytes(), 0o644)?;
        }
        let mut log = fs::OpenOptions::new().create(true).append(true).read(true).open(self.state.join("history"))?;
        // A crash can cut the last line short; start on a line of our own.
        let len = log.metadata()?.len();
        let mut last = *b"\n";
        if len > 0 {
            std::os::unix::fs::FileExt::read_exact_at(&log, &mut last, len - 1)?;
        }
        let sep = if last[0] == b'\n' { "" } else { "\n" };
        writeln!(log, "{sep}{} {hash}", unix_now())?;
        Ok(report)
    }

    /// The nearest parent of `target` that files cannot be created under:
    /// a dangling link, a link into the read-only nix store (what
    /// home-manager leaves), or a file. A link to a writable directory is
    /// fine. `--adopt` moves it aside like any other foreign file.
    fn blocked_parent(&self, target: &str) -> Result<Option<String>> {
        let rest = target.trim_start_matches("~/");
        let mut parts: Vec<&str> = rest.split('/').collect();
        parts.pop();
        let mut path = self.home.clone();
        for (i, part) in parts.iter().enumerate() {
            path.push(part);
            let blocked = match fs::symlink_metadata(&path) {
                Err(_) => return Ok(None),
                Ok(m) if m.file_type().is_symlink() => {
                    fs::canonicalize(&path).map_or(true, |real| real.starts_with("/nix/store") || !real.is_dir())
                }
                Ok(m) => !m.is_dir(),
            };
            if blocked {
                return Ok(Some(format!("~/{}", parts[..=i].join("/"))));
            }
        }
        Ok(None)
    }

    /// Move `target` into this generation's backups, keeping its path.
    fn back_up(&self, hash: &str, target: &str) -> Result<()> {
        let path = self.target(target)?;
        let base = self.state.join("backups").join(hash).join(target.trim_start_matches("~/"));
        let mut dest = base.clone();
        for n in 1.. {
            if fs::symlink_metadata(&dest).is_err() {
                break;
            }
            dest = PathBuf::from(format!("{}.{n}", base.display()));
        }
        fs::create_dir_all(dest.parent().unwrap())?;
        fs::rename(&path, &dest).with_context(|| format!("backing up {}", path.display()))?;
        Ok(())
    }

    /// Whether `target` is exactly what generation `g` put there.
    pub fn matches(&self, g: &Generation, target: &str) -> bool {
        g.units.get(target).is_some_and(|e| self.observe(target, e).is_ok_and(|s| s == State::Matches))
    }

    /// Whether strata made what is at `target`, judged from the filesystem
    /// alone: a link into the store, or a copy whose bytes are
    /// a store object, or a torn copy (see `torn_copy`). This is what lets a
    /// run after a crash, with `current` still naming an older generation,
    /// replace its own half-applied work. A copy the user edited has bytes
    /// strata never stored, so it is refused as drift.
    pub fn owns(&self, target: &str) -> bool {
        let Ok(path) = self.target(target) else { return false };
        let state = match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => fs::read_link(&path).map_or(State::Missing, State::Symlink),
            Ok(m) if m.is_file() => State::File,
            _ => State::Missing,
        };
        self.owned(target, &state)
    }

    fn owned(&self, target: &str, state: &State) -> bool {
        match state {
            State::Symlink(to) => to.starts_with(self.objects()),
            State::File => self.target(target).is_ok_and(|p| {
                let exec = fs::metadata(&p).is_ok_and(|m| m.permissions().mode() & 0o100 != 0);
                fs::read(&p).is_ok_and(|bytes| {
                    self.object_path(&blob_name(&bytes, exec)).exists() || self.torn_copy(target, &bytes)
                })
            }),
            _ => false,
        }
    }

    /// Whether `file` is what a crash can leave of a copy strata wrote at
    /// `target`: empty or all zeros (nothing in it to lose, and it needs no
    /// record, which a crash may have lost too), or a torn image (see
    /// `torn_image`) of the object some generation on disk records as the
    /// copy at `target`. Only objects whose bytes match their name vouch.
    fn torn_copy(&self, target: &str, file: &[u8]) -> bool {
        if file.iter().all(|&b| b == 0) {
            return true;
        }
        let Ok(dir) = fs::read_dir(self.state.join("generations")) else { return false };
        let mut seen = HashSet::new();
        for name in dir.flatten() {
            let name = name.file_name().to_string_lossy().to_string();
            let Some(hash) = name.strip_suffix(".json") else { continue };
            let Ok(g) = self.load(hash) else { continue };
            let Some(e) = g.units.get(target) else { continue };
            if e.kind != Kind::Copy || e.object.ends_with(".d") || !seen.insert(e.object.clone()) {
                continue;
            }
            if fs::read(self.object_path(&e.object))
                .is_ok_and(|obj| blob_name(&obj, e.exec) == e.object && torn_image(file, &obj))
            {
                return true;
            }
        }
        false
    }

    fn swap(&self, target: &str, entry: &Entry) -> Result<()> {
        match entry.kind {
            Kind::Link => {
                let tmp = self.swap_tmp(target)?;
                symlink(self.object_path(&entry.object), &tmp)?;
                self.swap_in(target, &tmp)
            }
            Kind::Copy if entry.object.ends_with(".d") => bail!("{target}: trees are linked, not copied"),
            Kind::Copy => self.put_copy(target, &fs::read(self.object_path(&entry.object))?, entry.exec),
            Kind::Merge => bail!("{target}: a merged file is merged, not swapped"),
        }
    }

    fn swap_tmp(&self, target: &str) -> Result<PathBuf> {
        let path = self.target(target)?;
        let parent = path.parent().unwrap();
        fs::create_dir_all(parent)?;
        let tmp = parent.join(format!(".{}.strata-tmp", path.file_name().unwrap().to_string_lossy()));
        let _ = fs::remove_file(&tmp);
        Ok(tmp)
    }

    fn swap_in(&self, target: &str, tmp: &Path) -> Result<()> {
        let path = self.target(target)?;
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            bail!("{target}: a directory is in the way");
        }
        fs::rename(tmp, &path).with_context(|| format!("swapping in {}", path.display()))?;
        Ok(())
    }

    fn put_copy(&self, target: &str, bytes: &[u8], exec: bool) -> Result<()> {
        let tmp = self.swap_tmp(target)?;
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.set_permissions(fs::Permissions::from_mode(if exec { 0o755 } else { 0o644 }))?;
        // Without --durable a crash can leave this short or zeroed;
        // the next run rewrites it (`torn_copy` keeps it strata's).
        if self.durable {
            f.sync_all()?;
        }
        self.swap_in(target, &tmp)
    }

    fn doc_of(&self, target: &str, entry: &Entry) -> Result<Doc> {
        let format = Format::of(target).with_context(|| format!("{target}: only .json and .toml files merge"))?;
        let bytes = fs::read(self.object_path(&entry.object))?;
        Doc::parse(format, &bytes).map_err(|e| anyhow!("{target}: strata's keys: {e}"))
    }

    /// Merge a `Kind::Merge` unit's keys into the file at its target (a
    /// link into the store is read through and replaced by a file). `prev`
    /// is the generation before's entry for the target: keys it declared
    /// that `entry` does not leave the file. Returns whether the file
    /// changed (or, without `write`, would) and the drift.
    pub fn merge_into(
        &self,
        target: &str,
        entry: &Entry,
        prev: Option<&Entry>,
        write: bool,
    ) -> Result<(bool, Vec<Drift>)> {
        let want = fs::read(self.object_path(&entry.object))?;
        self.merge_bytes(target, &want, prev, write)
    }

    /// `merge_into` with strata's keys as bytes, for `plan`, which has no store objects.
    pub fn merge_bytes(
        &self,
        target: &str,
        want: &[u8],
        prev: Option<&Entry>,
        write: bool,
    ) -> Result<(bool, Vec<Drift>)> {
        let format = Format::of(target).with_context(|| format!("{target}: only .json and .toml files merge"))?;
        let want = Doc::parse(format, want).map_err(|e| anyhow!("{target}: strata's keys: {e}"))?;
        // A copy or link strata made before is what it declared then.
        let prev = prev
            .filter(|e| !e.object.starts_with('/') && !e.object.ends_with(".d"))
            .and_then(|e| fs::read(self.object_path(&e.object)).ok())
            .and_then(|b| Doc::parse(format, &b).ok());
        let path = self.target(target)?;
        let is_file = fs::symlink_metadata(&path).is_ok_and(|m| m.is_file());
        let disk = match fs::read(&path) {
            Ok(b) => Some(Doc::parse(format, &b).map_err(|e| anyhow!("{e}; fix it or pass --adopt"))?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(anyhow!(e).context(format!("reading {}", path.display()))),
        };
        let mut doc = disk.clone().unwrap_or_else(|| Doc::empty(format));
        let drift = doc.merge(&want, prev.as_ref());
        if is_file && disk.as_ref() == Some(&doc) {
            return Ok((false, drift));
        }
        if write {
            self.put_copy(target, &doc.render(), false)?;
        }
        Ok((true, drift))
    }

    /// Take a dropped `Kind::Merge` unit's keys out of the file at its
    /// target, deleting the file if nothing is left. False when the file
    /// is not a document strata can read (it is kept as it is).
    fn unmerge(&self, target: &str, old: &Entry) -> Result<bool> {
        let Ok(prev) = self.doc_of(target, old) else { return Ok(false) };
        let format = Format::of(target).expect("doc_of checked");
        let path = self.target(target)?;
        let Ok(bytes) = fs::read(&path) else { return Ok(true) };
        let Ok(mut doc) = Doc::parse(format, &bytes) else { return Ok(false) };
        doc.unmerge(&prev);
        if doc.is_empty() {
            fs::remove_file(&path)?;
        } else {
            self.put_copy(target, &doc.render(), false)?;
        }
        Ok(true)
    }

    /// Delete store objects no kept generation references.
    pub fn gc(&self, keep: usize) -> Result<(usize, u64)> {
        let mut gens = self.history();
        gens.dedup();
        let mut live = BTreeSet::new();
        let mut kept: Vec<String> = gens.iter().rev().take(keep).cloned().collect();
        if let Some((cur, _)) = self.current()? {
            kept.push(cur);
        }
        for g in &kept {
            for e in self.load(g)?.units.values() {
                live.insert(e.object.clone());
                if e.object.ends_with(".d") && !e.object.starts_with('/') {
                    for f in walk(&self.object_path(&e.object))? {
                        let bytes = fs::read(&f)?;
                        let exec = fs::metadata(&f)?.permissions().mode() & 0o111 != 0;
                        live.insert(blob_name(&bytes, exec));
                    }
                }
            }
        }
        let (mut n, mut bytes) = (0, 0);
        for entry in fs::read_dir(self.objects())? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if live.contains(&name) || name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                for f in walk(&path)? {
                    bytes += fs::metadata(&f)?.len();
                }
                fs::remove_dir_all(&path)?;
            } else {
                bytes += entry.metadata()?.len();
                make_writable(&path)?;
                fs::remove_file(&path)?;
            }
            n += 1;
        }
        for name in fs::read_dir(self.state.join("generations"))? {
            let name = name?.file_name().to_string_lossy().to_string();
            let hash = name.trim_end_matches(".json");
            if !kept.iter().any(|k| k == hash) {
                let path = self.state.join("generations").join(&name);
                make_writable(&path)?;
                fs::remove_file(path)?;
            }
        }
        Ok((n, bytes))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Missing,
    Matches,
    Symlink(PathBuf),
    File,
    Dir,
}

impl State {
    pub fn describe(&self) -> String {
        match self {
            State::Missing => "missing".into(),
            State::Matches => "up to date".into(),
            State::Symlink(to) => format!("symlink -> {}", to.display()),
            State::File => "a regular file".into(),
            State::Dir => "a directory".into(),
        }
    }
}

#[derive(Debug, Default)]
pub struct SwitchReport {
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    pub kept: Vec<String>,
    pub backed_up: Vec<String>,
    pub refused: Vec<String>,
    /// (target, key) where the app had changed a key strata declares;
    /// strata's value is back.
    pub drift: Vec<(String, Drift)>,
}

/// What one `realize` wrote and checked.
#[derive(Default)]
struct Writes {
    fresh: Vec<PathBuf>,
    verify: Duration,
    verified: usize,
    repaired: usize,
}

impl Writes {
    /// Make `path` hold `bytes`: read back what is there (a crash can leave
    /// it short or zeroed under the right name) and rewrite it if it differs.
    fn check(&mut self, path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
        let t = Instant::now();
        let existing = fs::read(path);
        self.verify += t.elapsed();
        match existing {
            Ok(b) if b == bytes => {
                self.verified += 1;
                return Ok(());
            }
            Ok(_) => {
                self.verified += 1;
                self.repaired += 1;
            }
            Err(_) => {}
        }
        write_file(path, bytes, mode)?;
        if self.fresh.is_empty() {
            crash_point("realize:object");
        }
        self.fresh.push(path.to_path_buf());
        Ok(())
    }
}

/// Whether `file` could be `want` after a crash lost some of its writes:
/// no longer than `want`, and each 4 KiB block either holds `want`'s bytes
/// at that offset or reads as zeros (an extent allocated but never
/// written). Empty, half-length and zero-filled copies all qualify; a real
/// edit changes bytes somewhere, so it does not. The one edit this cannot
/// tell from a crash is cutting bytes off the end.
pub fn torn_image(file: &[u8], want: &[u8]) -> bool {
    file.len() <= want.len()
        && file.chunks(4096).zip(want.chunks(4096)).all(|(f, w)| *f == w[..f.len()] || f.iter().all(|&b| b == 0))
}

/// Test hook: `STRATA_CRASH_AT=<point>` ends the process at that point
/// with no cleanup, as a kill would (`tests/crash.rs`). Points:
/// `realize:object` (after the first new store file), `realize:before-order`,
/// `switch:swap:<n>` (after the n-th swap), `switch:before-pointer`.
fn crash_at() -> Option<&'static str> {
    static AT: OnceLock<Option<String>> = OnceLock::new();
    AT.get_or_init(|| std::env::var("STRATA_CRASH_AT").ok()).as_deref()
}

fn crash_armed() -> bool {
    crash_at().is_some()
}

fn crash_point(point: &str) {
    if crash_at() == Some(point) {
        eprintln!("strata: crashing at {point} (STRATA_CRASH_AT)");
        // SAFETY: _exit ends the process at once; nothing runs after it.
        unsafe { libc::_exit(86) }
    }
}

/// The ordering point without a cache flush. Each new file's data is handed
/// to the device, then one barrier keeps every later write behind them.
#[cfg(target_os = "macos")]
fn barrier(fresh: &[PathBuf]) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // Plain fsync on macOS writes the file out to the drive but does not
    // flush the drive's cache (that is F_FULLFSYNC, which std uses).
    let (last, rest) = fresh.split_last().expect("fresh is not empty");
    for p in rest {
        let f = fs::File::open(p)?;
        // SAFETY: fsync on a descriptor this function owns.
        if unsafe { libc::fsync(f.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let f = fs::File::open(last)?;
    // SAFETY: fcntl(F_BARRIERFSYNC) on a descriptor this function owns.
    if unsafe { libc::fcntl(f.as_raw_fd(), libc::F_BARRIERFSYNC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// No ordering point on Linux. The candidate, one `syncfs` on the store's
/// filesystem, costs a journal commit and a cache flush per edit and waits
/// on every other writer's dirty pages there. What it would buy is that a power cut can't take the
/// objects of a generation `current` already names; without it, the next
/// run rewrites them and `rollback` refuses them by name instead.
#[cfg(not(target_os = "macos"))]
fn barrier(_fresh: &[PathBuf]) -> io::Result<()> {
    Ok(())
}

pub fn blob_name(bytes: &[u8], exec: bool) -> String {
    let h = blake3::hash(bytes).to_hex();
    if exec { format!("{}-x", &h[..32]) } else { h[..32].to_string() }
}

/// tmp + rename without fsync: readers see old or new bytes; durability is
/// the caller's (see `sync_all`).
fn write_file(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let dir = path.parent().ok_or_else(|| anyhow!("{} has no parent", path.display()))?;
    let tmp = dir.join(format!(".{}.tmp", path.file_name().unwrap().to_string_lossy()));
    let _ = fs::remove_file(&tmp);
    fs::write(&tmp, bytes)?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

/// fsync many files at once: the filesystem folds concurrent flushes into
/// fewer journal commits.
fn sync_all(paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    let chunk = paths.len().div_ceil(workers);
    std::thread::scope(|s| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|c| s.spawn(move || c.iter().try_for_each(|p| fs::File::open(p)?.sync_all())))
            .collect();
        handles.into_iter().try_for_each(|h| h.join().expect("an fsync thread panicked"))
    })?;
    Ok(())
}

/// tmp + fsync + rename, so the name holds the old bytes or the new ones.
/// With `sync_dir` the rename itself is durable once this returns.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32, sync_dir: bool) -> Result<()> {
    let dir = path.parent().ok_or_else(|| anyhow!("{} has no parent", path.display()))?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.tmp", path.file_name().unwrap().to_string_lossy()));
    let _ = fs::remove_file(&tmp);
    let mut f = fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    if sync_dir {
        fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

fn make_writable(path: &Path) -> io::Result<()> {
    let mut p = fs::metadata(path)?.permissions();
    p.set_mode(p.mode() | 0o200);
    fs::set_permissions(path, p)
}

fn same_file_content(a: &Path, b: &Path) -> Result<bool> {
    let (ma, mb) = (fs::metadata(a)?, fs::metadata(b)?);
    if ma.len() != mb.len() {
        return Ok(false);
    }
    Ok(fs::read(a)? == fs::read(b)?)
}

fn walk(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let p = e?.path();
            if fs::metadata(&p)?.is_dir() { stack.push(p) } else { out.push(p) }
        }
    }
    out.sort();
    Ok(out)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(target: &str, text: &str) -> Unit {
        let content = Content::File { bytes: text.as_bytes().into(), exec: false };
        Unit { target: target.into(), kind: Kind::Copy, content }
    }

    /// A crash can leave strata's copy empty, cut short or with zeroed
    /// blocks; that is still strata's file. A real edit is not.
    #[test]
    fn torn_copies_stay_strata_s() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("state"), dir.path().join("home"), false);
        let text = "x".repeat(10_000) + "\n";
        let (hash, g) = store.realize(&[copy("~/a.json", &text)], Inputs::default()).unwrap();
        store.switch(&hash, &g, false, &[]).unwrap();
        let path = store.target("~/a.json").unwrap();
        let mut zeroed = text.clone().into_bytes();
        zeroed[4096..8192].fill(0);
        let cases: Vec<(&str, Vec<u8>, bool)> = vec![
            ("intact", text.clone().into_bytes(), true),
            ("empty", vec![], true),
            ("half", text.as_bytes()[..text.len() / 2].to_vec(), true),
            ("a zeroed block", zeroed, true),
            ("all zeros", vec![0; text.len()], true),
            ("one byte changed", text.replacen('x', "y", 1).into_bytes(), false),
            ("rewritten shorter", b"{\"mine\": 1}\n".to_vec(), false),
            ("appended to", format!("{text}more").into_bytes(), false),
        ];
        for (what, bytes, mine) in cases {
            fs::write(&path, &bytes).unwrap();
            assert_eq!(store.owns("~/a.json"), mine, "{what}");
        }
        // apply converges from a torn copy without --adopt, even after the
        // config moved on to a new generation.
        fs::write(&path, &text.as_bytes()[..100]).unwrap();
        let (hash, g) = store.realize(&[copy("~/a.json", "new\n")], Inputs::default()).unwrap();
        store.switch(&hash, &g, false, &[]).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
    }

    fn unit(target: &str, kind: Kind, text: &str) -> Unit {
        Unit { target: target.into(), kind, content: Content::File { bytes: text.as_bytes().into(), exec: false } }
    }

    fn layers(dirs: &[&str]) -> Inputs {
        Inputs { layers: dirs.iter().map(|d| d.to_string()).collect(), facts: BTreeMap::new() }
    }

    /// A build that lost a layer (an optional directory missing, or a
    /// caller that never named it) would delete every file the layer
    /// makes. The switch refuses, naming the layer and those files, until
    /// a flag says the layer is gone on purpose.
    #[test]
    fn a_switch_that_would_drop_a_config_layer_is_refused_and_names_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("state"), dir.path().join("home"), false);
        let public = unit("~/.gitconfig", Kind::Link, "[user]\n");
        let private = unit("~/.npmrc", Kind::Link, "token\n");
        let (hash, g) = store.realize(&[public.clone(), private], layers(&["/c/config", "/c/private/config"])).unwrap();
        store.switch(&hash, &g, false, &[]).unwrap();

        let (hash, g) = store.realize(std::slice::from_ref(&public), layers(&["/c/config"])).unwrap();
        let err = store.switch(&hash, &g, false, &[]).unwrap_err();
        let drop = err.downcast_ref::<DropsLayer>().expect("a typed refusal");
        assert_eq!(drop.layers, ["/c/private/config"]);
        assert_eq!(drop.removes, ["~/.npmrc"]);
        assert!(err.to_string().contains("/c/private/config") && err.to_string().contains("~/.npmrc"), "{err}");
        assert!(store.target("~/.npmrc").unwrap().exists(), "nothing was removed");

        let report = store.switch(&hash, &g, false, &["/c/private/config".into()]).unwrap();
        assert_eq!(report.removed, ["~/.npmrc"]);
    }

    fn settings(store: &Store) -> serde_json::Value {
        serde_json::from_slice(&fs::read(store.target("~/.claude/settings.json").unwrap()).unwrap()).unwrap()
    }

    /// Claude Code adds its own keys to settings.json; an apply keeps them.
    #[test]
    fn an_app_s_added_key_survives_an_apply() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("state"), dir.path().join("home"), false);
        let ours = r#"{ "model": "opus", "permissions": { "allow": ["Read"] } }"#;
        let (hash, g) =
            store.realize(&[unit("~/.claude/settings.json", Kind::Merge, ours)], Inputs::default()).unwrap();
        store.switch(&hash, &g, false, &[]).unwrap();
        let path = store.target("~/.claude/settings.json").unwrap();
        fs::write(&path, r#"{ "model": "opus", "permissions": { "allow": ["Read"], "additionalDirectories": ["/x"] }, "feedbackSurveyState": { "n": 1 } }"#).unwrap();

        // The config moves on too: a key changes, one goes away, one is new.
        let ours = r#"{ "model": "opus", "permissions": { "allow": ["Read", "Grep"] }, "theme": "auto" }"#;
        let (hash, g) =
            store.realize(&[unit("~/.claude/settings.json", Kind::Merge, ours)], Inputs::default()).unwrap();
        let report = store.switch(&hash, &g, false, &[]).unwrap();
        assert!(report.drift.is_empty(), "{:?}", report.drift);
        assert_eq!(
            settings(&store),
            serde_json::json!({
                "model": "opus",
                "permissions": { "allow": ["Read", "Grep"], "additionalDirectories": ["/x"] },
                "feedbackSurveyState": { "n": 1 },
                "theme": "auto",
            })
        );

        // Dropping the unit takes strata's keys out and leaves the app's.
        let (hash, g) = store.realize(&[], Inputs::default()).unwrap();
        store.switch(&hash, &g, false, &[]).unwrap();
        assert_eq!(
            settings(&store),
            serde_json::json!({ "permissions": { "additionalDirectories": ["/x"] }, "feedbackSurveyState": { "n": 1 } })
        );
    }

    /// `/model sonnet` rewrites a key strata declares: the next apply
    /// reports both values and puts strata's back.
    #[test]
    fn an_app_s_change_to_a_strata_key_is_reported_as_drift_and_reset() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("state"), dir.path().join("home"), false);
        let ours = r#"{ "model": "opus", "env": { "A": "1" } }"#;
        let (hash, g) =
            store.realize(&[unit("~/.claude/settings.json", Kind::Merge, ours)], Inputs::default()).unwrap();
        store.switch(&hash, &g, false, &[]).unwrap();
        let path = store.target("~/.claude/settings.json").unwrap();
        fs::write(&path, r#"{ "model": "sonnet", "env": { "A": "1", "B": "2" } }"#).unwrap();

        assert_eq!(store.observe("~/.claude/settings.json", &g.units["~/.claude/settings.json"]).unwrap(), State::File);
        let report = store.switch(&hash, &g, false, &[]).unwrap();
        assert_eq!(
            report.drift,
            [(
                "~/.claude/settings.json".to_string(),
                Drift { path: "model".into(), app: "\"sonnet\"".into(), strata: "\"opus\"".into() }
            )]
        );
        assert_eq!(report.changed, ["~/.claude/settings.json"]);
        assert_eq!(settings(&store), serde_json::json!({ "model": "opus", "env": { "A": "1", "B": "2" } }));
    }

    /// TOML merges the same way.
    #[test]
    fn a_toml_file_keeps_the_app_s_tables() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("state"), dir.path().join("home"), false);
        let (hash, g) =
            store.realize(&[unit("~/.codex/config.toml", Kind::Merge, "model = \"a\"\n")], Inputs::default()).unwrap();
        store.switch(&hash, &g, false, &[]).unwrap();
        let path = store.target("~/.codex/config.toml").unwrap();
        fs::write(&path, "model = \"b\"\n[projects.\"/x\"]\ntrust_level = \"trusted\"\n").unwrap();
        let report = store.switch(&hash, &g, false, &[]).unwrap();
        assert_eq!(report.drift.len(), 1);
        let t: toml::Table = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(t["model"].as_str(), Some("a"));
        assert_eq!(t["projects"]["/x"]["trust_level"].as_str(), Some("trusted"));
    }
}
