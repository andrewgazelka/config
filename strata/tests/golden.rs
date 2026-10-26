//! The golden configs render to the recorded bytes, and nothing else.
//! `expected/*.json` maps each file under `~` to its text.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn golden() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for e in fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            files(root, &p, out);
        } else {
            out.push(p.strip_prefix(root).unwrap().to_string_lossy().into_owned());
        }
    }
}

fn case(name: &str, layers: &[&str]) {
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_strata"));
    for l in layers {
        cmd.arg("--config").arg(golden().join(l));
    }
    cmd.arg("--state").arg(work.path().join("state")).arg("--home").arg(work.path().join("home"));
    let res = cmd.arg("build").arg("--out").arg(&out).output().unwrap();
    assert!(res.status.success(), "{name}: build failed:\n{}", String::from_utf8_lossy(&res.stderr));

    let expected: BTreeMap<String, String> =
        serde_json::from_str(&fs::read_to_string(golden().join(format!("expected/{name}.json"))).unwrap()).unwrap();
    let mut bad = Vec::new();
    for (rel, text) in &expected {
        // The recorded text may lack the final newline every file gets.
        let want = if text.ends_with('\n') { text.clone() } else { format!("{text}\n") };
        match fs::read_to_string(out.join(rel)) {
            Err(_) => bad.push(format!("missing {rel}")),
            Ok(got) if got != want => bad.push(format!("differs {rel}")),
            Ok(_) => {}
        }
    }
    let mut produced = Vec::new();
    files(&out, &out, &mut produced);
    bad.extend(produced.into_iter().filter(|p| !expected.contains_key(p)).map(|p| format!("extra {p}")));
    assert!(bad.is_empty(), "{name}:\n  {}", bad.join("\n  "));
}

#[test]
fn base() {
    case("base", &["common"]);
}

#[test]
fn with_host_layer() {
    case("main", &["common", "laptop"]);
}
