//! Crash convergence. Each trial kills `apply` at a crash point, loses what
//! a power cut could lose, and then checks that the next `apply` converges,
//! that `rollback` restores the previous generation exactly or refuses it by
//! name, and that a re-apply converges again. One state and home carry
//! through every trial, so old generations pile up as they do in use.
//!
//! Two damage modes alternate. "model" loses only writes after the last
//! ordering point; there, rollback must never refuse. "random" truncates
//! any store objects and copies, sparing only an object whose copy it also
//! damaged (the ordering point put the object on disk first).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

const POINTS: [&str; 5] =
    ["realize:object", "realize:before-order", "switch:swap:1", "switch:swap:2", "switch:before-pointer"];
/// strata's exit code at a `STRATA_CRASH_AT` point.
const CRASH_EXIT: i32 = 86;
/// One edit to a copy (zed settings) and one to a link (ghostty).
const EDITS: [(&str, &str, i64); 2] = [("zed.cfg", "tab_size: {}", 2), ("ghostty.cfg", "window-padding-x: {}", 5)];
const TRIALS: usize = 200;

/// SplitMix64: a fixed seed gives the same trials on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// True with probability `tenths / 10`.
    fn chance(&mut self, tenths: usize) -> bool {
        self.below(10) < tenths
    }

    fn sample<T: Clone>(&mut self, xs: &[T], k: usize) -> Vec<T> {
        let mut v = xs.to_vec();
        let k = k.min(v.len());
        for i in 0..k {
            let j = i + self.below(v.len() - i);
            v.swap(i, j);
        }
        v.truncate(k);
        v
    }
}

fn with(pat: &str, v: i64) -> String {
    pat.replace("{}", &v.to_string())
}

fn stderr(o: &Output) -> String {
    let s = String::from_utf8_lossy(&o.stderr);
    let mut from = s.len().saturating_sub(800);
    while !s.is_char_boundary(from) {
        from += 1;
    }
    s[from..].to_string()
}

/// Like an editor's save: a temp file renamed over.
fn replace_atomic(path: &Path, text: &str) {
    let tmp = path.with_file_name(format!(".{}.tmp", path.file_name().unwrap().to_string_lossy()));
    fs::write(&tmp, text).unwrap();
    fs::rename(&tmp, path).unwrap();
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() { copy_dir(&src, &dst) } else { fs::copy(&src, &dst).map(|_| ()).unwrap() }
    }
}

/// Every path under `root`, without following symlinks.
fn walk(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(dir) = fs::read_dir(root) else { return };
    for e in dir.flatten() {
        let p = e.path();
        if fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) {
            walk(&p, out);
        }
        out.push(p);
    }
}

/// Regular files under `root`, minus hidden temp files.
fn tree_files(root: &Path) -> Vec<PathBuf> {
    let mut all = Vec::new();
    walk(root, &mut all);
    all.into_iter()
        .filter(|p| fs::symlink_metadata(p).is_ok_and(|m| m.is_file()))
        .filter(|p| !p.file_name().unwrap().to_string_lossy().starts_with('.'))
        .collect()
}

fn written_since(p: &Path, start: SystemTime) -> bool {
    fs::metadata(p).and_then(|m| m.modified()).is_ok_and(|t| t >= start)
}

/// Truncate to zero or half, as a write the disk never got would leave it.
fn damage(p: &Path, rng: &mut Rng) -> String {
    let meta = fs::metadata(p).unwrap();
    let size = meta.len();
    let new = if size == 0 || rng.chance(5) { 0 } else { size / 2 };
    let mode = meta.permissions().mode();
    fs::set_permissions(p, fs::Permissions::from_mode(mode | 0o200)).unwrap();
    fs::OpenOptions::new().write(true).open(p).unwrap().set_len(new).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
    format!("{}:{size}->{new}", p.file_name().unwrap().to_string_lossy())
}

/// What `expected` (a `build --out` tree) has that `home` lacks or differs in.
fn home_diff(home: &Path, expected: &Path) -> Vec<String> {
    let mut want = Vec::new();
    walk(expected, &mut want);
    want.sort();
    let mut bad = Vec::new();
    for w in want {
        let rel = w.strip_prefix(expected).unwrap();
        let got = home.join(rel);
        let meta = fs::symlink_metadata(&w).unwrap();
        if meta.file_type().is_symlink() {
            if fs::read_link(&got).ok() != fs::read_link(&w).ok() {
                bad.push(format!("{}: link differs", rel.display()));
            }
        } else if meta.is_file() {
            match fs::read(&got) {
                Err(_) => bad.push(format!("{}: missing", rel.display())),
                Ok(b) if b != fs::read(&w).unwrap() => {
                    bad.push(format!("{}: {} bytes, want {}", rel.display(), b.len(), meta.len()))
                }
                Ok(_) => {}
            }
        }
    }
    bad
}

struct Crash {
    work: PathBuf,
    config: PathBuf,
    state: PathBuf,
    home: PathBuf,
    originals: HashMap<&'static str, String>,
    /// Generation hash -> the config value it was applied from.
    by_gen: HashMap<String, i64>,
    expected: HashMap<i64, PathBuf>,
    rollbacks: [usize; 2],
}

impl Crash {
    fn new(work: &Path) -> Self {
        let config = work.join("config");
        copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/common"), &config);
        let originals = EDITS.iter().map(|(f, _, _)| (*f, fs::read_to_string(config.join(f)).unwrap())).collect();
        Crash {
            work: work.to_path_buf(),
            state: work.join("state"),
            home: work.join("home"),
            config,
            originals,
            by_gen: HashMap::new(),
            expected: HashMap::new(),
            rollbacks: [0, 0],
        }
    }

    fn cli(&self, config: &Path, state: &Path, home: &Path) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_strata"));
        c.arg("--config").arg(config).arg("--state").arg(state).arg("--home").arg(home).env_remove("STRATA_CRASH_AT");
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cli(&self.config, &self.state, &self.home).args(args).output().unwrap()
    }

    fn edited(&self, f: &str, value: i64) -> String {
        let (_, pat, base) = EDITS.iter().find(|(n, _, _)| *n == f).unwrap();
        let text = &self.originals[f];
        assert!(text.contains(&with(pat, *base)), "{} is not in {f}", with(pat, *base));
        text.replace(&with(pat, *base), &with(pat, value))
    }

    fn write_config(&self, value: i64) {
        for (f, _, _) in EDITS {
            replace_atomic(&self.config.join(f), &self.edited(f, value));
        }
    }

    /// What a clean apply of config `value` puts in the home.
    fn expect(&mut self, value: i64) -> PathBuf {
        if let Some(p) = self.expected.get(&value) {
            return p.clone();
        }
        let cfg = self.work.join(format!("exp-config-{value}"));
        copy_dir(&self.config, &cfg);
        for (f, _, _) in EDITS {
            fs::write(cfg.join(f), self.edited(f, value)).unwrap();
        }
        let out = self.work.join(format!("exp-{value}"));
        let res = self
            .cli(&cfg, &self.work.join("exp-state"), &self.work.join("exp-home"))
            .arg("build")
            .arg("--out")
            .arg(&out)
            .output()
            .unwrap();
        assert!(res.status.success(), "expected build {value}: {}", stderr(&res));
        fs::remove_dir_all(&cfg).unwrap();
        self.expected.insert(value, out.clone());
        out
    }

    fn apply(&mut self, value: i64, crash_at: Option<&str>) -> Output {
        let mut cmd = self.cli(&self.config, &self.state, &self.home);
        if let Some(point) = crash_at {
            cmd.env("STRATA_CRASH_AT", point);
        }
        let res = cmd.arg("apply").output().unwrap();
        if res.status.success() {
            let out = String::from_utf8_lossy(&res.stdout);
            let line = out.lines().find(|l| l.starts_with("generation ")).expect("apply reports its generation");
            let hash = line.split_whitespace().nth(1).unwrap().trim_end_matches(':');
            self.by_gen.insert(hash.to_string(), value);
        }
        res
    }

    /// The home holds config `value`'s output; with `plan`, so does the config on disk.
    fn check(&mut self, step: &str, value: i64, plan: bool) -> Vec<String> {
        let expected = self.expect(value);
        let mut bad = home_diff(&self.home, &expected);
        if plan {
            let res = self.run(&["plan", "--json"]);
            if !res.status.success() {
                bad.push(format!("plan failed: {}", stderr(&res)));
            } else {
                let rows: Vec<serde_json::Value> = serde_json::from_slice(&res.stdout).unwrap();
                for r in rows.iter().filter(|r| r["action"] != "none") {
                    bad.push(format!("plan: {} -> {}", r["target"], r["action"]));
                }
            }
        }
        bad.into_iter().map(|b| format!("{step}: {b}")).collect()
    }

    fn copy_targets(&self) -> Vec<PathBuf> {
        let cur = fs::read_to_string(self.state.join("current")).unwrap_or_default();
        let Ok(text) = fs::read_to_string(self.state.join("generations").join(format!("{}.json", cur.trim()))) else {
            return vec![];
        };
        let g: serde_json::Value = serde_json::from_str(&text).unwrap();
        g["units"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, e)| e["kind"] == "copy")
            .map(|(t, _)| self.home.join(t.trim_start_matches("~/")))
            .collect()
    }

    fn trial(&mut self, i: usize, rng: &mut Rng, value: i64) -> (String, Vec<String>) {
        let model = i.is_multiple_of(2);
        let point = POINTS[i / 2 % POINTS.len()];
        // Half the time the config moves on before anyone re-applies.
        let fin = if rng.chance(5) { value + 1 } else { value };
        let rewritable: HashSet<Vec<u8>> = tree_files(&self.expect(fin)).iter().map(|p| fs::read(p).unwrap()).collect();
        let copies = self.copy_targets();
        self.write_config(value);
        let start = SystemTime::now();
        let res = self.apply(value, Some(point));
        let rc = res.status.code();
        let name = format!("{} {point}", if model { "model" } else { "random" });
        if rc != Some(CRASH_EXIT) && rc != Some(0) {
            return (name, vec![format!("crashing apply exited {rc:?}: {}", stderr(&res))]);
        }
        let crashed = rc == Some(CRASH_EXIT);
        let store = self.state.join("store");
        let hist = self.state.join("history");
        let mut hit = Vec::new();
        if model {
            // Only writes after the last completed ordering point can be lost:
            // before this run's barrier, its new store files and everything
            // the previous run wrote after its own; after it, this run's copies.
            let mut lost: Vec<PathBuf>;
            if crashed && point.starts_with("switch:") {
                lost = copies.iter().filter(|p| written_since(p, start)).cloned().collect();
            } else {
                lost = copies.iter().filter(|p| p.exists()).cloned().collect();
                lost.push(self.state.join("current"));
                if crashed {
                    lost.extend(tree_files(&store).into_iter().filter(|p| written_since(p, start)));
                    let gens = fs::read_dir(self.state.join("generations")).unwrap();
                    lost.extend(gens.flatten().map(|e| e.path()).filter(|p| written_since(p, start)));
                }
                if rng.chance(5)
                    && let Ok(bytes) = fs::read(&hist)
                {
                    let mut lines: Vec<&[u8]> = bytes.split_inclusive(|&b| b == b'\n').collect();
                    if let Some(last) = lines.pop() {
                        let cut = &last[..rng.below(last.len())];
                        fs::write(&hist, [lines.concat(), cut.to_vec()].concat()).unwrap();
                        hit.push("history: last line cut".to_string());
                    }
                }
            }
            for p in lost {
                if p.exists() && rng.chance(6) {
                    hit.push(damage(&p, rng));
                }
            }
        } else {
            let objects = tree_files(&store);
            let targets: Vec<PathBuf> = copies.iter().filter(|p| p.is_file() && !p.is_symlink()).cloned().collect();
            let k = rng.below(3);
            let chosen_t = rng.sample(&targets, k);
            let spared: HashSet<&PathBuf> = chosen_t
                .iter()
                .map(|t| fs::read(t).unwrap())
                .filter(|b| !rewritable.contains(b))
                .flat_map(|b| objects.iter().filter(move |o| fs::read(o).is_ok_and(|ob| ob == b)))
                .collect();
            let pool: Vec<PathBuf> = objects.iter().filter(|o| !spared.contains(o)).cloned().collect();
            let k = 1 + rng.below(6);
            for p in rng.sample(&pool, k).iter().chain(&chosen_t) {
                hit.push(damage(p, rng));
            }
        }
        if fin != value {
            self.write_config(fin);
        }
        let name = if crashed { name } else { format!("{name} (not reached)") };
        let res = self.apply(fin, None);
        if !res.status.success() {
            return (name, vec![format!("recovering apply failed after damage {hit:?}: {}", stderr(&res))]);
        }
        let mut bad = self.check("recover", fin, true);
        // Rollback reads only the store: it must restore the previous
        // generation exactly, or refuse a damaged one by name.
        let res = self.run(&["rollback"]);
        if res.status.success() {
            let prev = fs::read_to_string(self.state.join("current")).unwrap().trim().to_string();
            match self.by_gen.get(&prev).copied() {
                Some(v) => bad.extend(self.check("rollback", v, false)),
                None => bad.push(format!("rollback landed on unknown generation {prev}")),
            }
        } else if !stderr(&res).contains("damaged store objects") {
            bad.push(format!("rollback failed: {}", stderr(&res)));
        } else if model {
            bad.push(format!("rollback refused under the crash model: {}", stderr(&res)));
        }
        self.rollbacks[usize::from(res.status.success())] += 1;
        let res = self.apply(fin, None);
        if !res.status.success() {
            bad.push(format!("re-apply after rollback failed: {}", stderr(&res)));
        }
        bad.extend(self.check("reapply", fin, true));
        if !bad.is_empty() {
            bad.push(format!("(damage: {hit:?})"));
        }
        (name, bad)
    }
}

#[test]
fn crashes_converge() {
    let work = tempfile::tempdir().unwrap();
    let mut c = Crash::new(work.path());
    let mut rng = Rng(1);
    let base = 1000;
    c.write_config(base - 1);
    let res = c.apply(base - 1, None);
    assert!(res.status.success(), "initial apply: {}", stderr(&res));

    let mut failures = Vec::new();
    let mut reached = 0;
    for i in 0..TRIALS {
        let (name, bad) = c.trial(i, &mut rng, base + 2 * i as i64);
        reached += usize::from(!name.ends_with("(not reached)"));
        if !bad.is_empty() {
            failures.push(format!("trial {i} [{name}]:\n      {}", bad[..bad.len().min(8)].join("\n      ")));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {TRIALS} trials did not converge:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
    assert!(reached > TRIALS / 2, "only {reached} of {TRIALS} trials reached their crash point");
    let [refused, restored] = c.rollbacks;
    eprintln!("{TRIALS} trials converged; rollback restored {restored}, refused {refused} damaged");

    // A real edit to a copy target is still refused.
    let victim = c.copy_targets()[0].clone();
    fs::write(&victim, "{\"edited\": \"by the user\"}\n").unwrap();
    let res = c.run(&["apply"]);
    assert!(
        !res.status.success() && stderr(&res).contains("not made by strata"),
        "a user edit to {} was not refused",
        victim.display()
    );
}
