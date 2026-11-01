//! JSON and TOML files an app writes too (`Kind::Merge`). strata owns the
//! keys it declares, down to the leaves: an object is a scope, anything
//! else (a string, a number, a list) one value. The app's other keys stay
//! as they are. A key strata declares that the app changed is drift: it is
//! reported with both values, and strata's wins.

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Json,
    Toml,
}

impl Format {
    pub fn of(target: &str) -> Option<Format> {
        if target.ends_with(".json") {
            Some(Format::Json)
        } else if target.ends_with(".toml") {
            Some(Format::Toml)
        } else {
            None
        }
    }
}

/// A key strata declares whose value on disk is not the one strata last
/// wrote there: the app (or a person) changed it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Drift {
    /// `permissions.defaultMode`; keys with dots or spaces are quoted.
    pub path: String,
    pub app: String,
    pub strata: String,
}

/// A value as nested maps, so one merge serves both formats.
#[derive(Clone, Debug, PartialEq)]
pub enum Node<L> {
    Map(BTreeMap<String, Node<L>>),
    Leaf(L),
}

pub trait Leaf: Clone + PartialEq + fmt::Display + Sized {
    fn split(self) -> Result<BTreeMap<String, Self>, Self>;
    fn table(map: BTreeMap<String, Self>) -> Self;
}

impl Leaf for serde_json::Value {
    fn split(self) -> Result<BTreeMap<String, Self>, Self> {
        match self {
            serde_json::Value::Object(m) => Ok(m.into_iter().collect()),
            v => Err(v),
        }
    }
    fn table(map: BTreeMap<String, Self>) -> Self {
        serde_json::Value::Object(map.into_iter().collect())
    }
}

impl Leaf for toml::Value {
    fn split(self) -> Result<BTreeMap<String, Self>, Self> {
        match self {
            toml::Value::Table(m) => Ok(m.into_iter().collect()),
            v => Err(v),
        }
    }
    fn table(map: BTreeMap<String, Self>) -> Self {
        toml::Value::Table(map.into_iter().collect())
    }
}

impl<L: Leaf> Node<L> {
    fn from_leaf(v: L) -> Self {
        match v.split() {
            Ok(m) => Node::Map(m.into_iter().map(|(k, v)| (k, Node::from_leaf(v))).collect()),
            Err(v) => Node::Leaf(v),
        }
    }
    fn into_leaf(self) -> L {
        match self {
            Node::Map(m) => L::table(m.into_iter().map(|(k, v)| (k, v.into_leaf())).collect()),
            Node::Leaf(v) => v,
        }
    }
    fn show(&self) -> String {
        self.clone().into_leaf().to_string()
    }
}

type Map<L> = BTreeMap<String, Node<L>>;

fn show_path(path: &[String]) -> String {
    let part = |k: &String| {
        if k.is_empty() || k.contains(['.', ' ', '"']) { format!("{k:?}") } else { k.clone() }
    };
    path.iter().map(part).collect::<Vec<_>>().join(".")
}

/// Make `disk` hold `want`'s leaves, taking out what `prev` (what strata
/// declared last time) had and `want` no longer does.
fn merge<L: Leaf>(
    disk: &mut Map<L>,
    want: &Map<L>,
    prev: Option<&Map<L>>,
    path: &mut Vec<String>,
    drift: &mut Vec<Drift>,
) {
    if let Some(prev) = prev {
        for (k, p) in prev {
            if !want.contains_key(k) {
                unmerge(disk, k, p);
            }
        }
    }
    for (k, w) in want {
        path.push(k.clone());
        let p = prev.and_then(|p| p.get(k));
        let d = disk.get(k);
        match w {
            Node::Map(wm) => {
                if !matches!(d, Some(Node::Map(_))) {
                    if let Some(d) = d
                        && p != Some(d)
                    {
                        drift.push(Drift { path: show_path(path), app: d.show(), strata: w.show() });
                    }
                    disk.insert(k.clone(), Node::Map(BTreeMap::new()));
                }
                let Some(Node::Map(dm)) = disk.get_mut(k) else { unreachable!("just made a map") };
                let pm = match p {
                    Some(Node::Map(m)) => Some(m),
                    _ => None,
                };
                merge(dm, wm, pm, path, drift);
            }
            Node::Leaf(_) if d == Some(w) => {}
            Node::Leaf(_) => {
                // A value strata wrote and the config since changed is an
                // update; any other value on disk was the app's.
                if let Some(d) = d
                    && p != Some(d)
                {
                    drift.push(Drift { path: show_path(path), app: d.show(), strata: w.show() });
                }
                disk.insert(k.clone(), w.clone());
            }
        }
        path.pop();
    }
}

/// Take strata's key `k` (declared as `p`) out of `disk`; a map left empty goes too.
fn unmerge<L: Leaf>(disk: &mut Map<L>, k: &str, p: &Node<L>) {
    match (p, disk.get_mut(k)) {
        (Node::Map(pm), Some(Node::Map(dm))) => {
            for (k2, p2) in pm {
                unmerge(dm, k2, p2);
            }
            if dm.is_empty() {
                disk.remove(k);
            }
        }
        (_, Some(_)) => {
            disk.remove(k);
        }
        (_, None) => {}
    }
}

/// A whole file: an object (JSON) or a table (TOML) at the top.
#[derive(Clone, Debug, PartialEq)]
pub enum Doc {
    Json(Map<serde_json::Value>),
    Toml(Map<toml::Value>),
}

impl Doc {
    pub fn empty(format: Format) -> Doc {
        match format {
            Format::Json => Doc::Json(BTreeMap::new()),
            Format::Toml => Doc::Toml(BTreeMap::new()),
        }
    }

    pub fn parse(format: Format, bytes: &[u8]) -> Result<Doc, String> {
        match format {
            Format::Json => {
                let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("not valid JSON: {e}"))?;
                match Node::from_leaf(v) {
                    Node::Map(m) => Ok(Doc::Json(m)),
                    Node::Leaf(_) => Err("not a JSON object at the top".into()),
                }
            }
            Format::Toml => {
                let text = std::str::from_utf8(bytes).map_err(|e| format!("not UTF-8: {e}"))?;
                let t: toml::Table = text.parse().map_err(|e| format!("not valid TOML: {e}"))?;
                match Node::from_leaf(toml::Value::Table(t)) {
                    Node::Map(m) => Ok(Doc::Toml(m)),
                    Node::Leaf(_) => unreachable!("a table is a map"),
                }
            }
        }
    }

    /// The bytes strata's own `json()` and `toml()` would give: keys
    /// sorted, so a file holding only strata's keys is byte for byte the
    /// one strata renders.
    pub fn render(&self) -> Vec<u8> {
        let mut text = match self {
            Doc::Json(m) => {
                serde_json::to_string_pretty(&Node::Map(m.clone()).into_leaf()).expect("a JSON value serializes")
            }
            Doc::Toml(m) => match Node::Map(m.clone()).into_leaf() {
                toml::Value::Table(t) => toml::to_string_pretty(&t).expect("a TOML table serializes"),
                _ => unreachable!("a map is a table"),
            },
        };
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.into_bytes()
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Doc::Json(m) => m.is_empty(),
            Doc::Toml(m) => m.is_empty(),
        }
    }

    /// Merge strata's keys (`want`) in; `prev` is what strata declared in
    /// the generation before, if it declared this file.
    pub fn merge(&mut self, want: &Doc, prev: Option<&Doc>) -> Vec<Drift> {
        let mut drift = Vec::new();
        match (self, want) {
            (Doc::Json(d), Doc::Json(w)) => {
                let p = match prev {
                    Some(Doc::Json(p)) => Some(p),
                    _ => None,
                };
                merge(d, w, p, &mut Vec::new(), &mut drift);
            }
            (Doc::Toml(d), Doc::Toml(w)) => {
                let p = match prev {
                    Some(Doc::Toml(p)) => Some(p),
                    _ => None,
                };
                merge(d, w, p, &mut Vec::new(), &mut drift);
            }
            _ => unreachable!("one target has one format"),
        }
        drift
    }

    /// Take out every key `prev` declared.
    pub fn unmerge(&mut self, prev: &Doc) {
        match (self, prev) {
            (Doc::Json(d), Doc::Json(p)) => p.iter().for_each(|(k, v)| unmerge(d, k, v)),
            (Doc::Toml(d), Doc::Toml(p)) => p.iter().for_each(|(k, v)| unmerge(d, k, v)),
            _ => unreachable!("one target has one format"),
        }
    }
}
