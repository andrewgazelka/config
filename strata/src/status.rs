//! What the last apply did, for `strata status`: the last generation a
//! switch succeeded with, and the error applying is stuck on and since
//! when. The watcher runs where nobody reads its log, so a failure has to
//! be somewhere a person looks.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub last_good: Option<Good>,
    /// Set while applying fails; the next success clears it.
    pub failing: Option<Failing>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Good {
    pub generation: String,
    /// Unix seconds.
    pub at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failing {
    /// The latest error.
    pub error: String,
    /// The first failure since the last success.
    pub since: u64,
    pub last: u64,
    pub attempts: u64,
    /// blake3 of each distinct error since `since`, so each is announced once.
    pub seen: Vec<String>,
}

impl Status {
    pub fn path(state: &Path) -> PathBuf {
        state.join("status.json")
    }

    /// A missing or unreadable file is no history.
    pub fn load(state: &Path) -> Status {
        std::fs::read(Self::path(state)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, state: &Path) -> Result<()> {
        let text = serde_json::to_vec_pretty(self)?;
        crate::store::write_atomic(&Self::path(state), &text, 0o644, false)
    }

    pub fn succeeded(&mut self, generation: &str, now: u64) {
        self.last_good = Some(Good { generation: generation.to_string(), at: now });
        self.failing = None;
    }

    /// Returns whether `error` is new since the last success: worth telling
    /// someone about.
    pub fn failed(&mut self, error: &str, now: u64) -> bool {
        let id = blake3::hash(error.as_bytes()).to_hex()[..16].to_string();
        let f = self.failing.get_or_insert_with(|| Failing {
            error: String::new(),
            since: now,
            last: now,
            attempts: 0,
            seen: Vec::new(),
        });
        f.error = error.to_string();
        f.last = now;
        f.attempts += 1;
        if f.seen.contains(&id) {
            return false;
        }
        f.seen.push(id);
        true
    }

    pub fn render(&self, current: Option<&str>) -> String {
        let mut out = String::new();
        match &self.failing {
            Some(f) => {
                out.push_str(&format!(
                    "failing since {} ({} attempt{}, last {}):\n",
                    local_time(f.since),
                    f.attempts,
                    if f.attempts == 1 { "" } else { "s" },
                    local_time(f.last)
                ));
                for line in f.error.lines() {
                    out.push_str(format!("  {line}").trim_end());
                    out.push('\n');
                }
            }
            None => out.push_str("ok\n"),
        }
        match &self.last_good {
            Some(g) => out.push_str(&format!("last good generation {} at {}\n", g.generation, local_time(g.at))),
            None => out.push_str("no successful apply recorded\n"),
        }
        if let Some(c) = current {
            out.push_str(&format!("current generation {c}\n"));
        }
        out
    }
}

/// A macOS notification, without waiting for it.
pub fn notify(error: &str) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let mut msg: String = error.split_whitespace().collect::<Vec<_>>().join(" ");
    if msg.chars().count() > 240 {
        msg = msg.chars().take(240).collect::<String>() + "…";
    }
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "display notification \"{}\" with title \"strata can't apply\" subtitle \"run strata status\"",
        esc(&msg)
    );
    let child = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match child {
        Ok(mut c) => {
            std::thread::spawn(move || c.wait());
        }
        Err(e) => eprintln!("strata: osascript: {e}"),
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn local_time(t: u64) -> String {
    let secs = t as libc::time_t;
    // SAFETY: an all-zero `tm` is a valid value; localtime_r fills it in.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are to locals that outlive the call.
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return t.to_string();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The status a failing watcher leaves: the last good generation, the
    /// current error, and since when, with each distinct error announced once.
    #[test]
    fn a_failing_apply_records_the_last_good_generation_and_the_error_since_it_began() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Status::default();
        s.succeeded("g1", 100);
        assert!(s.failed("refusing ~/.claude/settings.json", 200), "a new error is announced");
        assert!(!s.failed("refusing ~/.claude/settings.json", 300), "the same error is not announced again");
        assert!(s.failed("1 error\nzed.cfg:3: ...", 400), "a different error is");
        s.save(dir.path()).unwrap();

        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(Status::path(dir.path())).unwrap()).unwrap();
        assert_eq!(json["last_good"], serde_json::json!({ "generation": "g1", "at": 100 }));
        let f = &json["failing"];
        assert_eq!(f["error"], "1 error\nzed.cfg:3: ...");
        assert_eq!((f["since"].as_u64(), f["last"].as_u64(), f["attempts"].as_u64()), (Some(200), Some(400), Some(3)));
        assert_eq!(Status::load(dir.path()), s);
        let shown = s.render(Some("g1"));
        assert!(shown.starts_with("failing since ") && shown.contains("  zed.cfg:3: ...\n"), "{shown}");
        assert!(shown.contains("last good generation g1 at "), "{shown}");

        s.succeeded("g2", 500);
        assert_eq!(s.failing, None);
        assert!(s.failed("refusing ~/.claude/settings.json", 600), "a new streak announces again");
        assert_eq!(s.failing.as_ref().unwrap().since, 600);
    }
}
