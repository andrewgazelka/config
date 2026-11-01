mod build;
mod db;
mod document;
mod eval;
mod lower;
mod merge;
mod render;
mod schema;
mod status;
mod store;
mod syntax;
mod types;
mod value;

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::build::Loaded;
use crate::status::{Status, unix_now};
use crate::store::{Content, Kind, Store, SwitchReport, Unit};

#[derive(Parser)]
#[command(version, about = "Entities and components in, files out")]
struct Cli {
    /// Config directories; every `.cfg` under them is one config. The
    /// layers they declare (`layer "../other"`) join them.
    #[arg(long = "config", short, global = true, default_value = "config")]
    configs: Vec<PathBuf>,
    /// Where generations and the content store live.
    #[arg(long, global = true, env = "STRATA_STATE")]
    state: Option<PathBuf>,
    /// The directory `~/` means.
    #[arg(long, global = true, env = "HOME", default_value = "/")]
    home: PathBuf,
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
        /// A config layer the current generation has that is gone on
        /// purpose; without it, a switch that drops a layer is refused.
        #[arg(long = "drop-layer")]
        drop_layer: Vec<PathBuf>,
    },
    /// Rebuild and switch on every change under the config directories.
    /// A failed apply posts a notification and is recorded for `status`.
    Watch,
    /// The last good generation, and the error applying is stuck on, if any.
    Status,
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
        #[arg(long = "drop-layer")]
        drop_layer: Vec<PathBuf>,
    },
    /// Delete store objects that kept generations don't use.
    Gc {
        #[arg(long, default_value_t = 5)]
        keep: usize,
    },
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
        let loaded = build::load(&self.configs, &self.home)?;
        for m in &loaded.missing {
            eprintln!("strata: optional layer {} does not exist; building without it", m.display());
        }
        Ok(loaded)
    }
}

/// `--drop-layer` as generations record layers: absolute, symlinks resolved
/// while the directory still exists.
fn layer_names(dirs: &[PathBuf]) -> Result<Vec<String>> {
    dirs.iter()
        .map(|d| {
            let abs = fs::canonicalize(d).or_else(|_| std::path::absolute(d).map(|a| eval::normalize(&a)))?;
            Ok(abs.to_string_lossy().to_string())
        })
        .collect()
}

fn print_report(report: &SwitchReport) {
    for c in &report.changed {
        println!("  updated {c}");
    }
    for (target, d) in &report.drift {
        println!("  drift {target} {}: the app had {}, strata's {} is back", d.path, d.app, d.strata);
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
}

/// Record an apply's outcome for `strata status`. Returns whether the
/// error is new since the last success.
fn record(store: &Store, outcome: Result<&str, &str>) -> bool {
    let mut status = Status::load(&store.state);
    let new = match outcome {
        Ok(generation) => {
            status.succeeded(generation, unix_now());
            false
        }
        Err(e) => status.failed(e, unix_now()),
    };
    if let Err(e) = status.save(&store.state) {
        eprintln!("strata: writing {}: {e:#}", Status::path(&store.state).display());
    }
    new
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
            let (hash, _) = cli.store().realize(&units, loaded.inputs())?;
            if let Some(out) = out {
                write_out(out, &units)?;
            }
            println!("{hash}");
        }
        Cmd::Apply { adopt, drop_layer } => {
            let store = cli.store();
            let drop = layer_names(drop_layer)?;
            match apply(&cli, &store, *adopt, &drop) {
                Ok(hash) => {
                    record(&store, Ok(&hash));
                }
                Err(e) => {
                    record(&store, Err(&format!("{e:#}")));
                    return Err(e);
                }
            }
        }
        Cmd::Watch => watch(&cli)?,
        Cmd::Plan { json } => {
            let loaded = cli.load()?;
            let units = evaluate(&loaded)?;
            plan(&cli, &loaded, &units, *json)?;
        }
        Cmd::Explain { address } => {
            let loaded = cli.load()?;
            print!("{}", loaded.explain(address)?);
        }
        Cmd::Rollback { to, drop_layer } => {
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
            let report = store.switch(&target, &generation, false, &layer_names(drop_layer)?)?;
            record(&store, Ok(&target));
            println!("rolled back to {target}: {} changed, {} removed", report.changed.len(), report.removed.len());
        }
        Cmd::Status => {
            let store = cli.store();
            let status = Status::load(&store.state);
            let current = store.current()?.map(|(h, _)| h);
            print!("{}", status.render(current.as_deref()));
            if status.failing.is_some() {
                return Ok(ExitCode::FAILURE);
            }
        }
        Cmd::Gc { keep } => {
            let (n, bytes) = cli.store().gc(*keep)?;
            println!("deleted {n} objects, {bytes} bytes");
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

fn apply(cli: &Cli, store: &Store, adopt: bool, drop: &[String]) -> Result<String> {
    let t = Instant::now();
    let loaded = cli.load()?;
    let units = evaluate(&loaded)?;
    let t_eval = t.elapsed();
    let (hash, generation) = store.realize(&units, loaded.inputs())?;
    let t_realize = t.elapsed() - t_eval;
    let stats = store.stats();
    let report = store.switch(&hash, &generation, adopt, drop)?;
    let t_switch = t.elapsed() - t_eval - t_realize;
    print_report(&report);
    println!(
        "generation {hash}: {} units from {} layer{}, eval+render {:.1} ms, realize {:.1} ms \
         (verify {} {:.2} ms, {} repaired; order {} {:.2} ms), switch {:.1} ms",
        generation.units.len(),
        generation.inputs.layers.len(),
        if generation.inputs.layers.len() == 1 { "" } else { "s" },
        ms(t_eval),
        ms(t_realize),
        stats.verified,
        ms(stats.verify),
        stats.repaired,
        stats.fresh,
        ms(stats.order),
        ms(t_switch)
    );
    Ok(hash)
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
    let store = cli.store();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })?;
    let stdout = std::io::stdout();
    let report = |line: &serde_json::Value| {
        let mut out = stdout.lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    };
    // Every failure goes to the log, the status file and, the first time
    // it is seen since the last success, a notification.
    let fail = |seq: u64, generation: Option<&str>, error: String| {
        report(&serde_json::json!({ "seq": seq, "generation": generation, "error": error }));
        if record(&store, Err(&error)) {
            status::notify(&error);
        }
    };
    let mut watched: Vec<PathBuf> = Vec::new();
    let mut watch_all = |watcher: &mut notify::RecommendedWatcher, loaded: &Loaded| -> Result<()> {
        for p in watched.drain(..) {
            let _ = watcher.unwatch(&p);
        }
        for r in &loaded.roots {
            watcher.watch(&r.abs, RecursiveMode::Recursive)?;
            watched.push(r.abs.clone());
        }
        // A fact file outside the config is replaced, not edited (home-manager
        // relinks it), so its directory is what reports the change.
        for (_, f) in &loaded.decls.facts {
            if let Some(dir) = f.parent().filter(|d| d.is_dir())
                && !watched.iter().any(|w| w == dir)
            {
                watcher.watch(dir, RecursiveMode::NonRecursive)?;
                watched.push(dir.to_path_buf());
            }
        }
        Ok(())
    };
    // A config that doesn't load is retried on any change to it, or after a minute.
    let mut loaded = loop {
        match cli.load() {
            Ok(l) => break l,
            Err(e) => {
                fail(0, None, format!("{e:#}"));
                for c in &cli.configs {
                    let _ = watcher.watch(c, RecursiveMode::Recursive);
                }
                let _ = rx.recv_timeout(Duration::from_secs(60));
                while rx.try_recv().is_ok() {}
            }
        }
    };
    for c in &cli.configs {
        let _ = watcher.unwatch(c);
    }
    watch_all(&mut watcher, &loaded)?;
    let mut seq = 0u64;
    let done = |seq: u64, loaded: &Loaded, t0: Instant| match rebuild(loaded, &store, t0, seq) {
        Ok((hash, line)) => {
            report(&line);
            record(&store, Ok(&hash));
        }
        Err((hash, e)) => fail(seq, hash.as_deref(), e),
    };
    done(seq, &loaded, Instant::now());
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
        let mut changed = loaded.refresh_facts();
        for p in &paths {
            match loaded.refresh(p) {
                Ok(c) => changed |= c,
                Err(e) => report(&serde_json::json!({ "error": format!("{e:#}") })),
            }
        }
        if !changed {
            continue;
        }
        seq += 1;
        if loaded.decls_changed() {
            // A layer or fact file came or went: build the config afresh.
            match cli.load() {
                Ok(l) => {
                    loaded = l;
                    watch_all(&mut watcher, &loaded)?;
                }
                Err(e) => {
                    fail(seq, None, format!("{e:#}"));
                    continue;
                }
            }
        }
        done(seq, &loaded, t0);
    }
}

/// One build and switch: the generation and its log line, or the
/// generation (if it got that far) and the error.
fn rebuild(
    loaded: &Loaded,
    store: &Store,
    t0: Instant,
    seq: u64,
) -> Result<(String, serde_json::Value), (Option<String>, String)> {
    let t = Instant::now();
    let units = evaluate(loaded).map_err(|e| (None, format!("{e:#}")))?;
    let t_eval = t.elapsed();
    let (hash, generation) = store.realize(&units, loaded.inputs()).map_err(|e| (None, format!("{e:#}")))?;
    let t_realize = t.elapsed() - t_eval;
    let stats = store.stats();
    let r = store.switch(&hash, &generation, false, &[]).map_err(|e| (Some(hash.clone()), format!("{e:#}")))?;
    let t_switch = t.elapsed() - t_eval - t_realize;
    let drift: Vec<_> = r
        .drift
        .iter()
        .map(|(target, d)| serde_json::json!({ "target": target, "key": d.path, "app": d.app, "strata": d.strata }))
        .collect();
    let line = serde_json::json!({
        "seq": seq,
        "generation": hash,
        "changed": r.changed,
        "removed": r.removed,
        "drift": drift,
        "eval_ms": ms(t_eval),
        "realize_ms": ms(t_realize),
        "verify_ms": ms(stats.verify),
        "verified": stats.verified,
        "repaired": stats.repaired,
        "order_ms": ms(stats.order),
        "switch_ms": ms(t_switch),
        "total_ms": ms(t0.elapsed()),
    });
    Ok((hash, line))
}

/// Read-only: what is at each target now and whether strata's bytes match.
fn plan(cli: &Cli, loaded: &Loaded, units: &[Unit], json: bool) -> Result<()> {
    let store = cli.store();
    let prev = store.current().ok().flatten().map(|(_, g)| g).unwrap_or_default();
    let layers = loaded.inputs().layers;
    let dropped: Vec<&String> = prev.inputs.layers.iter().filter(|l| !layers.contains(l)).collect();
    let mut rows = Vec::new();
    for u in units {
        if u.kind == Kind::Merge {
            let Content::File { bytes, .. } = &u.content else { continue };
            let (action, drift) = match store.merge_bytes(&u.target, bytes, prev.units.get(&u.target), false) {
                Ok((false, _)) => ("none".to_string(), vec![]),
                Ok((true, drift)) => ("merge strata's keys in".to_string(), drift),
                Err(e) => (format!("refuse: {e:#}"), vec![]),
            };
            rows.push(serde_json::json!({
                "target": u.target,
                "now": if store.target(&u.target)?.exists() { "file" } else { "missing" },
                "content_equal": action == "none",
                "kind": u.kind,
                "action": action,
                "drift": drift,
            }));
            continue;
        }
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
    for l in &dropped {
        eprintln!("apply would refuse: the current generation has layer {l} and this config does not");
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
        for d in r["drift"].as_array().into_iter().flatten() {
            println!(
                "    drift {}: the app has {}, strata's {} wins",
                d["path"].as_str().unwrap(),
                d["app"],
                d["strata"]
            );
        }
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
