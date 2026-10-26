mod build;
mod db;
mod eval;
mod lower;
mod merge;
mod render;
mod schema;
mod store;
mod syntax;
mod types;
mod value;

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use salsa::Setter;

use crate::build::Loaded;
use crate::db::Db;
use crate::store::{Content, Store, Unit};
use crate::syntax::ast::{FactSource, Stmt};

#[derive(Parser)]
#[command(version, about = "Entities and components in, files out")]
struct Cli {
    /// Config directories; every `.cfg` under them is one config.
    #[arg(long = "config", short, global = true, default_value = "config")]
    configs: Vec<PathBuf>,
    /// More config directories, merged in when they exist and skipped when
    /// they don't (a private checkout).
    #[arg(long = "config-optional", global = true)]
    optional: Vec<PathBuf>,
    /// Override a fact table: `NAME=file.json`.
    #[arg(long = "facts", global = true, value_parser = parse_facts)]
    facts: Vec<(String, PathBuf)>,
    /// Where generations and the content store live.
    #[arg(long, global = true, env = "STRATA_STATE")]
    state: Option<PathBuf>,
    /// The directory `~/` means.
    #[arg(long, global = true, env = "HOME", default_value = "/")]
    home: PathBuf,
    /// System for `nix eval` fact tables.
    #[arg(long, global = true, env = "STRATA_SYSTEM", default_value = default_system())]
    system: String,
    /// Flush every file to stable storage as it is written (F_FULLFSYNC on
    /// macOS). Without it, a crash is repaired on the next run instead.
    #[arg(long, global = true, env = "STRATA_DURABLE")]
    durable: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Report every error in the config.
    Check,
    /// Render and realize into the store; print the generation hash.
    Build {
        /// Also write each file under DIR at its target path, for diffing.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Build, then switch `~` to the new generation.
    Apply {
        /// Move files strata doesn't own into the state dir's backups instead of refusing.
        #[arg(long)]
        adopt: bool,
    },
    /// Rebuild and switch on every change under the config directories.
    Watch,
    /// Show what `apply` would do, reading `~` only.
    Plan {
        #[arg(long)]
        json: bool,
    },
    /// Every contribution to an address like `zed[font].size`, and which won.
    Explain { address: String },
    /// Switch to an earlier generation.
    Rollback {
        #[arg(long)]
        to: Option<String>,
    },
    /// Delete store objects that kept generations don't use.
    Gc {
        #[arg(long, default_value_t = 5)]
        keep: usize,
    },
    /// Evaluate `facts ... from nix` tables and cache them by lock hash.
    NixFacts,
    /// JSON Schemas as strata types.
    Schema {
        #[command(subcommand)]
        cmd: SchemaCmd,
    },
}

#[derive(Subcommand)]
enum SchemaCmd {
    /// Fetch a JSON Schema (a URL or a file) and write it as strata types:
    /// `type <name> = ...`, one alias per `$defs` entry.
    Import {
        source: String,
        #[arg(long)]
        name: String,
        /// Defaults to strata/stdlib/schemas/<name>.cfg.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

const fn default_system() -> &'static str {
    match (std::env::consts::ARCH.as_bytes(), std::env::consts::OS.as_bytes()) {
        (b"aarch64", b"macos") => "aarch64-darwin",
        (b"aarch64", _) => "aarch64-linux",
        (_, b"macos") => "x86_64-darwin",
        _ => "x86_64-linux",
    }
}

fn parse_facts(s: &str) -> Result<(String, PathBuf), String> {
    let (name, path) = s.split_once('=').ok_or("expected NAME=file.json")?;
    Ok((name.to_string(), PathBuf::from(path)))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

impl Cli {
    fn state(&self) -> PathBuf {
        self.state.clone().unwrap_or_else(|| {
            std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| self.home.join(".local/state"))
                .join("strata")
        })
    }

    fn store(&self) -> Store {
        Store::new(self.state(), self.home.clone(), self.durable)
    }

    fn load(&self) -> Result<Loaded> {
        let mut loaded = build::load(&self.configs, &self.optional, &self.facts)?;
        self.load_nix_facts(&mut loaded)?;
        Ok(loaded)
    }

    fn nix_decls(&self, loaded: &Loaded) -> Vec<(String, PathBuf, PathBuf)> {
        let db: &dyn Db = &loaded.db;
        let mut out = Vec::new();
        for src in db.ws().sources(db).iter() {
            if let Ok(stmts) = db::parse(db, *src) {
                for s in stmts {
                    if let Stmt::Facts { name, source: FactSource::Nix { expr, lock }, .. } = s {
                        let dir = Path::new(src.dir(db));
                        out.push((name.name.to_string(), dir.join(expr), dir.join(lock)));
                    }
                }
            }
        }
        out
    }

    fn facts_cache(&self, name: &str, expr: &Path, lock: &Path) -> Result<PathBuf> {
        let mut h = blake3::Hasher::new();
        h.update(&fs::read(expr).with_context(|| format!("reading {}", expr.display()))?);
        h.update(&[0]);
        h.update(&fs::read(lock).with_context(|| format!("reading {}", lock.display()))?);
        h.update(&[0]);
        h.update(self.system.as_bytes());
        Ok(self.state().join("facts").join(format!("{name}-{}.json", &h.finalize().to_hex()[..32])))
    }

    /// Cached `nix eval` results only: evaluation never happens on the hot path.
    fn load_nix_facts(&self, loaded: &mut Loaded) -> Result<()> {
        let ws = loaded.db.ws();
        let mut table = (**ws.facts(&loaded.db)).clone();
        for (name, expr, lock) in self.nix_decls(loaded) {
            if table.contains_key(&name) {
                continue;
            }
            if let Ok(text) = fs::read_to_string(self.facts_cache(&name, &expr, &lock)?) {
                table.insert(name, Arc::from(text.as_str()));
            }
        }
        ws.set_facts(&mut loaded.db).to(Arc::new(table));
        Ok(())
    }
}

/// Check, then turn `files.*` into units. Errors are rendered.
fn evaluate(loaded: &Loaded) -> Result<Vec<Unit>> {
    let diags = db::check(&loaded.db);
    if !diags.is_empty() {
        bail!("{} error{}\n{}", diags.len(), if diags.len() == 1 { "" } else { "s" }, loaded.render(&diags));
    }
    loaded.units().map_err(|d| anyhow::anyhow!("{}", loaded.render(&d)))
}

fn run(cli: Cli) -> Result<ExitCode> {
    match &cli.cmd {
        Cmd::Check => {
            let loaded = cli.load()?;
            evaluate(&loaded)?;
            println!("ok");
        }
        Cmd::Build { out } => {
            let loaded = cli.load()?;
            let units = evaluate(&loaded)?;
            let (hash, _) = cli.store().realize(&units)?;
            if let Some(out) = out {
                write_out(out, &units)?;
            }
            println!("{hash}");
        }
        Cmd::Apply { adopt } => {
            let t = Instant::now();
            let loaded = cli.load()?;
            let units = evaluate(&loaded)?;
            let t_eval = t.elapsed();
            let store = cli.store();
            let (hash, generation) = store.realize(&units)?;
            let t_realize = t.elapsed() - t_eval;
            let stats = store.stats();
            let report = store.switch(&hash, &generation, *adopt)?;
            let t_switch = t.elapsed() - t_eval - t_realize;
            for c in &report.changed {
                println!("  updated {c}");
            }
            for c in &report.removed {
                println!("  removed {c}");
            }
            for c in &report.kept {
                println!("  kept {c} (changed since strata wrote it)");
            }
            for c in &report.backed_up {
                println!("  backed up the old {c}");
            }
            println!(
                "generation {hash}: {} units, eval+render {:.1} ms, realize {:.1} ms \
                 (verify {} {:.2} ms, {} repaired; order {} {:.2} ms), switch {:.1} ms",
                generation.units.len(),
                ms(t_eval),
                ms(t_realize),
                stats.verified,
                ms(stats.verify),
                stats.repaired,
                stats.fresh,
                ms(stats.order),
                ms(t_switch)
            );
        }
        Cmd::Watch => watch(&cli)?,
        Cmd::Plan { json } => {
            let loaded = cli.load()?;
            let units = evaluate(&loaded)?;
            plan(&cli, &units, *json)?;
        }
        Cmd::Explain { address } => {
            let loaded = cli.load()?;
            print!("{}", loaded.explain(address)?);
        }
        Cmd::Rollback { to } => {
            let store = cli.store();
            let history = store.history();
            let cur = store.current()?.map(|(h, _)| h);
            let target = match to {
                Some(t) => t.clone(),
                None => {
                    // Skip lines a crash cut short: they name no generation.
                    history
                        .iter()
                        .rev()
                        .find(|h| Some(*h) != cur.as_ref() && store.load(h).is_ok())
                        .cloned()
                        .context("no earlier generation")?
                }
            };
            let generation = store.load(&target)?;
            let report = store.switch(&target, &generation, false)?;
            println!("rolled back to {target}: {} changed, {} removed", report.changed.len(), report.removed.len());
        }
        Cmd::Gc { keep } => {
            let (n, bytes) = cli.store().gc(*keep)?;
            println!("deleted {n} objects, {bytes} bytes");
        }
        Cmd::NixFacts => {
            let loaded = build::load(&cli.configs, &cli.optional, &[])?;
            for (name, expr, lock) in cli.nix_decls(&loaded) {
                let cache = cli.facts_cache(&name, &expr, &lock)?;
                let t = Instant::now();
                // `--file` doesn't auto-call a function, so call it in an --expr.
                let q = |s: &str| serde_json::to_string(s).expect("a string serializes");
                let call = format!("import {} {{ system = {}; }}", q(&expr.to_string_lossy()), q(&cli.system));
                let out = std::process::Command::new("nix")
                    .args(["eval", "--json", "--impure", "--expr", &call])
                    .output()
                    .context("running nix eval")?;
                if !out.status.success() {
                    bail!(
                        "nix eval {} failed ({}):\n{}",
                        expr.display(),
                        out.status,
                        String::from_utf8_lossy(&out.stderr)
                    );
                }
                store::write_atomic(&cache, &out.stdout, 0o644, true)?;
                println!("{name}: {} in {:.1} s -> {}", cli.system, t.elapsed().as_secs_f64(), cache.display());
            }
        }
        Cmd::Schema { cmd: SchemaCmd::Import { source, name, out } } => {
            let bytes = if source.starts_with("https://") || source.starts_with("http://") {
                let res = std::process::Command::new("curl")
                    .args(["-fsSL", "--", source])
                    .output()
                    .context("running curl")?;
                if !res.status.success() {
                    bail!("curl {source} failed ({}):\n{}", res.status, String::from_utf8_lossy(&res.stderr));
                }
                res.stdout
            } else {
                fs::read(source).with_context(|| format!("reading {source}"))?
            };
            let json: serde_json::Value =
                serde_json::from_slice(&bytes).with_context(|| format!("parsing {source}"))?;
            let hash = blake3::hash(&bytes).to_hex();
            let text =
                schema::convert(&json, name, source, hash.as_str()).map_err(|e| anyhow::anyhow!("{source}: {e}"))?;
            syntax::parser::parse(&text, 0).map_err(|d| anyhow::anyhow!("generated text does not parse: {d}"))?;
            let out = out.clone().unwrap_or_else(|| PathBuf::from(format!("strata/stdlib/schemas/{name}.cfg")));
            store::write_atomic(&out, text.as_bytes(), 0o644, false)?;
            println!("{} ({} bytes, blake3 {})", out.display(), text.len(), &hash.as_str()[..16]);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn write_out(out: &Path, units: &[Unit]) -> Result<()> {
    for u in units {
        let rel = u.target.trim_start_matches("~/");
        match &u.content {
            Content::File { bytes, .. } => {
                let p = out.join(rel);
                fs::create_dir_all(p.parent().unwrap())?;
                fs::write(p, bytes)?;
            }
            Content::Tree { entries } => {
                for (name, bytes, _) in entries {
                    let p = out.join(rel).join(name);
                    fs::create_dir_all(p.parent().unwrap())?;
                    fs::write(p, bytes)?;
                }
            }
            Content::External { path } => {
                let p = out.join(rel);
                fs::create_dir_all(p.parent().unwrap())?;
                let _ = fs::remove_file(&p);
                std::os::unix::fs::symlink(path, p)?;
            }
        }
    }
    Ok(())
}

fn watch(cli: &Cli) -> Result<()> {
    use notify::{RecursiveMode, Watcher};
    let mut loaded = cli.load()?;
    let store = cli.store();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })?;
    for r in &loaded.roots {
        watcher.watch(&r.abs, RecursiveMode::Recursive)?;
    }
    let stdout = std::io::stdout();
    let report = |line: serde_json::Value| {
        let mut out = stdout.lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    };
    report(rebuild(&loaded, &store, Instant::now(), 0));
    let mut seq = 0u64;
    loop {
        let first = rx.recv()?;
        let t0 = Instant::now();
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut take = |ev: notify::Result<notify::Event>| {
            // Opens and reads are strata's own (inotify reports them): acting
            // on them re-reads, which reports again, without end.
            if let Ok(ev) = ev
                && !ev.kind.is_access()
            {
                paths.extend(ev.paths);
            }
        };
        take(first);
        // Editors save in bursts (write, rename, chmod); take the whole burst.
        while let Ok(ev) = rx.recv_timeout(Duration::from_millis(1)) {
            take(ev);
        }
        // kqueue (macOS) reports a file renamed into a directory as a write
        // to the directory, then guesses which entry is new, and after an
        // editor's atomic save it can guess wrong and the save is lost. So a
        // directory event checks every file directly in it; unchanged ones
        // cost one read and compare.
        let mut files = Vec::new();
        for p in paths {
            match fs::read_dir(&p) {
                Ok(dir) => files.extend(dir.flatten().map(|e| e.path()).filter(|f| f.is_file())),
                Err(_) => files.push(p),
            }
        }
        let mut paths = files;
        paths.sort();
        paths.dedup();
        let mut changed = false;
        for p in &paths {
            match loaded.refresh(p) {
                Ok(c) => changed |= c,
                Err(e) => report(serde_json::json!({ "error": format!("{e:#}") })),
            }
        }
        if changed {
            seq += 1;
            report(rebuild(&loaded, &store, t0, seq));
        }
    }
}

fn rebuild(loaded: &Loaded, store: &Store, t0: Instant, seq: u64) -> serde_json::Value {
    let t = Instant::now();
    let units = match evaluate(loaded) {
        Ok(u) => u,
        Err(e) => return serde_json::json!({ "seq": seq, "error": format!("{e:#}") }),
    };
    let t_eval = t.elapsed();
    let realized = store.realize(&units);
    let t_realize = t.elapsed() - t_eval;
    let (hash, generation) = match realized {
        Ok(r) => r,
        Err(e) => return serde_json::json!({ "seq": seq, "error": format!("{e:#}") }),
    };
    let stats = store.stats();
    let switched = store.switch(&hash, &generation, false);
    let t_switch = t.elapsed() - t_eval - t_realize;
    match switched {
        Ok(r) => serde_json::json!({
            "seq": seq,
            "generation": hash,
            "changed": r.changed,
            "removed": r.removed,
            "eval_ms": ms(t_eval),
            "realize_ms": ms(t_realize),
            "verify_ms": ms(stats.verify),
            "verified": stats.verified,
            "repaired": stats.repaired,
            "order_ms": ms(stats.order),
            "switch_ms": ms(t_switch),
            "total_ms": ms(t0.elapsed()),
        }),
        Err(e) => serde_json::json!({ "seq": seq, "generation": hash, "error": format!("{e:#}") }),
    }
}

/// Read-only: what is at each target now and whether strata's bytes match.
fn plan(cli: &Cli, units: &[Unit], json: bool) -> Result<()> {
    let store = cli.store();
    let prev = store.current().ok().flatten().map(|(_, g)| g).unwrap_or_default();
    let mut rows = Vec::new();
    for u in units {
        let path = store.target(&u.target)?;
        let now = match fs::symlink_metadata(&path) {
            Err(_) => "missing".to_string(),
            Ok(m) if m.file_type().is_symlink() => format!("symlink -> {}", fs::read_link(&path)?.display()),
            Ok(m) if m.is_dir() => "directory".into(),
            Ok(_) => "file".into(),
        };
        let equal = live_equal(&path, &u.content);
        // Same ownership rule as switch, so plan predicts what apply does.
        let mine = store.matches(&prev, &u.target) || store.owns(&u.target);
        let action = match (now.as_str(), mine, equal) {
            ("missing", _, _) => "create",
            (_, true, true) => "none",
            (_, true, false) => "update",
            (_, false, true) => "adopt (same content, not made by strata: needs --adopt)",
            (_, false, false) => "adopt (differs, not made by strata: needs --adopt)",
        };
        rows.push(serde_json::json!({
            "target": u.target,
            "now": now,
            "content_equal": equal,
            "kind": u.kind,
            "action": action,
        }));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    for r in &rows {
        println!(
            "{}\n    now: {}\n    content: {}\n    apply: {} ({})",
            r["target"].as_str().unwrap(),
            r["now"].as_str().unwrap(),
            if r["content_equal"].as_bool().unwrap() { "same as strata's" } else { "differs" },
            r["action"].as_str().unwrap(),
            r["kind"].as_str().unwrap(),
        );
    }
    Ok(())
}

fn live_equal(path: &Path, content: &Content) -> bool {
    match content {
        Content::File { bytes, .. } => fs::read(path).is_ok_and(|b| *b == **bytes),
        Content::Tree { entries } => {
            let Ok(files) = build::walk(path) else { return false };
            // As sets: a directory walk puts `infra/status.nu` before `infra.nu`.
            let names: BTreeSet<String> =
                files.iter().map(|f| f.strip_prefix(path).unwrap().to_string_lossy().to_string()).collect();
            let want: BTreeSet<String> = entries.iter().map(|(n, _, _)| n.clone()).collect();
            names == want && entries.iter().all(|(n, bytes, _)| fs::read(path.join(n)).is_ok_and(|b| *b == **bytes))
        }
        Content::External { path: to } => fs::read_link(path).is_ok_and(|l| l == Path::new(to)),
    }
}
