//! Latency of the real config (`../config`): cold build and apply, a
//! re-apply with nothing to do, and one-field edits through `strata watch`,
//! from the save to the daemon's report. Needs `nix` for the fact table.
//!
//!     cargo bench --bench latency

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const COLD: usize = 10;
const EDITS: usize = 100;

const STRATA: &str = env!("CARGO_BIN_EXE_strata");

struct Bench {
    work: PathBuf,
    config: PathBuf,
    facts: String,
    rows: Vec<(String, Vec<f64>)>,
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() { copy_dir(&src, &dst) } else { fs::copy(&src, &dst).map(|_| ()).unwrap() }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

impl Bench {
    fn strata(&self, name: &str) -> Command {
        let mut c = Command::new(STRATA);
        c.arg("--config").arg(&self.config).arg("--facts").arg(&self.facts);
        c.arg("--state").arg(self.work.join(format!("{name}-state")));
        c.arg("--home").arg(self.work.join(format!("{name}-home")));
        c
    }

    fn run(&self, name: &str, cmd: &str) -> String {
        let out = self.strata(name).arg(cmd).output().unwrap();
        assert!(out.status.success(), "{name} {cmd}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }

    fn timed(&self, name: &str, cmd: &str) -> (f64, String) {
        let t = Instant::now();
        let out = self.run(name, cmd);
        (ms(t.elapsed()), out)
    }

    /// Fresh process, fresh state: `build`, and `apply` into an empty home.
    fn cold(&mut self) {
        for cmd in ["build", "apply"] {
            let xs = (0..COLD).map(|i| self.timed(&format!("cold-{cmd}-{i}"), cmd).0).collect();
            self.rows.push((format!("cold {cmd}"), xs));
        }
    }

    /// `apply` over an applied state: nothing to write, every object read back.
    fn warm(&mut self) {
        self.run("warm", "apply");
        let (mut walls, mut verify, mut count) = (Vec::new(), Vec::new(), 0);
        for _ in 0..COLD {
            let (wall, out) = self.timed("warm", "apply");
            walls.push(wall);
            // "... (verify <n> <ms> ms, ..."
            let rest = out.split("(verify ").nth(1).expect("apply reports verify timing");
            let mut it = rest.split_whitespace();
            count = it.next().unwrap().parse().unwrap();
            verify.push(it.next().unwrap().parse().unwrap());
        }
        self.rows.push(("re-apply, nothing changed".into(), walls));
        self.rows.push((format!("re-apply: verify {count} objects"), verify));
    }

    /// One-field edits, each to a value never used before, so every edit
    /// stores a new object.
    fn edits(&mut self, what: &str, target: &str, file: &str, pat: &str, rendered: &str, base: i64) {
        let name = format!("watch-{file}");
        let mut child = self.strata(&name).arg("watch").stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let (tx, rx) = mpsc::channel();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(serde_json::from_str::<serde_json::Value>(&line).unwrap()).is_err() {
                    break;
                }
            }
        });
        let report = || rx.recv_timeout(Duration::from_secs(60)).expect("watch reports within 60 s");
        let first = report();
        assert!(first.get("error").is_none(), "watch start: {first}");

        let path = self.config.join(file);
        let original = fs::read_to_string(&path).unwrap();
        let from = pat.replace("{}", &base.to_string());
        assert!(original.contains(&from), "{from:?} is not in {file}");
        let tmp = path.with_file_name(format!(".{file}.tmp"));
        let keys = ["eval_ms", "realize_ms", "order_ms", "switch_ms"];
        let mut series: Vec<Vec<f64>> = vec![Vec::new(); 1 + keys.len()];
        for i in 0..EDITS {
            let value = base + 1 + i as i64;
            let t = Instant::now();
            // Like an editor's save: a temp file renamed over.
            fs::write(&tmp, original.replace(&from, &pat.replace("{}", &value.to_string()))).unwrap();
            fs::rename(&tmp, &path).unwrap();
            let line = loop {
                let line = report();
                if line["seq"].as_u64().unwrap_or(0) > 0 {
                    break line;
                }
            };
            series[0].push(ms(t.elapsed()));
            assert!(line.get("error").is_none(), "edit {i}: {line}");
            // The edit must have landed in the target, not just in a report.
            let live = fs::read_to_string(self.work.join(format!("{name}-home")).join(target)).unwrap();
            let want = rendered.replace("{}", &value.to_string());
            assert!(live.contains(&want), "edit {i}: {target} does not show {want:?}");
            for (s, k) in series[1..].iter_mut().zip(keys) {
                s.push(line[k].as_f64().unwrap_or(0.0));
            }
        }
        fs::write(&path, original).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        let mut series = series.into_iter();
        self.rows.push((format!("{what}: save -> report"), series.next().unwrap()));
        for (k, xs) in keys.iter().zip(series) {
            self.rows.push((format!("{what}: {}", k.trim_end_matches("_ms")), xs));
        }
    }

    fn print(&self) {
        let width = self.rows.iter().map(|(n, _)| n.len()).max().unwrap();
        println!("{:width$}  {:>4} {:>9} {:>9} {:>9}  (ms)", "", "n", "median", "p95", "max");
        for (name, xs) in &self.rows {
            let mut ys = xs.clone();
            ys.sort_by(f64::total_cmp);
            let at = |q: f64| ys[((q * (ys.len() - 1) as f64).round() as usize).min(ys.len() - 1)];
            println!("{name:width$}  {:>4} {:>9.3} {:>9.3} {:>9.3}", ys.len(), at(0.5), at(0.95), at(1.0));
        }
    }
}

fn main() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().to_path_buf();
    // The config and the lock its fact table names, side by side as in the repo.
    let config = work.join("config");
    copy_dir(&repo.join("config"), &config);
    fs::copy(repo.join("flake.lock"), work.join("flake.lock")).unwrap();

    let state = work.join("facts-state");
    let out =
        Command::new(STRATA).arg("--config").arg(&config).arg("--state").arg(&state).arg("nix-facts").output().unwrap();
    assert!(out.status.success(), "nix-facts: {}", String::from_utf8_lossy(&out.stderr));
    let table = fs::read_dir(state.join("facts")).unwrap().flatten().next().expect("a cached fact table").path();

    let mut b = Bench { work, config, facts: format!("pkgs={}", table.display()), rows: Vec::new() };
    b.cold();
    b.warm();
    b.edits("zed settings.json (copy)", ".config/zed/settings.json", "zed.cfg", "tab_size: {}", "\"tab_size\": {}", 2);
    b.edits(
        "ghostty config (link)",
        ".config/ghostty/config.ghostty",
        "ghostty.cfg",
        "window-padding-x: {}",
        "window-padding-x = {}",
        5,
    );
    b.print();
}
