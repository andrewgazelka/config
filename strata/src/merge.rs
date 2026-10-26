//! The priority ladder: every contribution is flattened to leaves at
//! (field path) with a level; the highest level wins per path, and two
//! different values at the winning level are a conflict.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::syntax::Span;
use crate::value::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Copied from a prefab.
    Inherited,
    Default,
    Plain,
    Override,
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Level::Inherited => "inherited",
            Level::Default => "default",
            Level::Plain => "plain",
            Level::Override => "override",
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LeafValue {
    Scalar(Value),
    /// "This path is a record", with no fields of its own (`x: {}`).
    Record,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Leaf {
    pub path: Vec<String>,
    pub value: LeafValue,
    pub level: Level,
    pub span: Span,
    /// The prefab chain an inherited leaf came through.
    pub via: Option<Arc<str>>,
}

impl Leaf {
    fn shows(&self) -> String {
        match &self.value {
            LeafValue::Scalar(v) => v.to_string(),
            LeafValue::Record => "{...}".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MergeError {
    // Boxed: errors travel through every merge call and Leaf is large.
    Conflict { path: Vec<String>, a: Box<Leaf>, b: Box<Leaf> },
    Shape { path: Vec<String>, scalar: Box<Leaf>, record: Box<Leaf> },
}

impl MergeError {
    pub fn spans(&self) -> [&Leaf; 2] {
        match self {
            MergeError::Conflict { a, b, .. } => [a, b],
            MergeError::Shape { scalar, record, .. } => [scalar, record],
        }
    }

    pub fn message(&self) -> String {
        match self {
            MergeError::Conflict { path, a, b } => {
                let at = if path.is_empty() { String::new() } else { format!(" at {}", show_path(path)) };
                format!("conflicting {} values{at}: {} vs {}", a.level, a.shows(), b.shows())
            }
            MergeError::Shape { path, scalar, .. } => format!(
                "{} is set both to a value ({}) and to a record at the same level ({})",
                show_path(path),
                scalar.shows(),
                scalar.level
            ),
        }
    }
}

pub fn show_path(path: &[String]) -> String {
    if path.is_empty() {
        return "the value".into();
    }
    path.iter()
        .map(|k| {
            if !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                k.clone()
            } else {
                format!("{k:?}")
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Tells the merge which paths hold sets, whose leaves union instead of conflict.
pub trait Shape {
    fn is_set(&self, path: &[String]) -> bool;
}

pub struct NoSets;

impl Shape for NoSets {
    fn is_set(&self, _: &[String]) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Merged {
    pub value: Value,
    /// The leaves that decided the value, in path order.
    pub winners: Vec<Leaf>,
}

/// Merge leaves into one value. Order-independent: leaves are sorted by path
/// and the winner per path depends only on levels and values.
pub fn merge(mut leaves: Vec<Leaf>, shape: &dyn Shape) -> Result<Merged, MergeError> {
    leaves.sort_by(|a, b| a.path.cmp(&b.path).then(a.span.cmp(&b.span)));
    let mut winners = Vec::new();
    let value = resolve(&leaves, 0, shape, &mut winners)?;
    Ok(Merged { value: value.unwrap_or_else(|| Value::record(BTreeMap::new())), winners })
}

fn resolve(
    leaves: &[Leaf],
    depth: usize,
    shape: &dyn Shape,
    winners: &mut Vec<Leaf>,
) -> Result<Option<Value>, MergeError> {
    if leaves.is_empty() {
        return Ok(None);
    }
    let path = &leaves[0].path[..depth];
    let (exact, deeper): (Vec<&Leaf>, Vec<&Leaf>) = leaves.iter().partition(|l| l.path.len() == depth);
    let scalars: Vec<&Leaf> = exact.iter().copied().filter(|l| matches!(l.value, LeafValue::Scalar(_))).collect();
    let markers: Vec<&Leaf> = exact.iter().copied().filter(|l| l.value == LeafValue::Record).collect();
    let top_scalar = scalars.iter().map(|l| l.level).max();
    let top_record = markers.iter().chain(&deeper).map(|l| l.level).max();

    match (top_scalar, top_record) {
        (Some(s), r) if r.is_none_or(|r| s > r) => {
            let at_top: Vec<&Leaf> = scalars.into_iter().filter(|l| l.level == s).collect();
            if shape.is_set(path) {
                return Ok(Some(union(&at_top, winners)));
            }
            let first = at_top[0];
            let LeafValue::Scalar(v) = &first.value else { unreachable!() };
            if let Some(other) = at_top.iter().find(|l| l.value != first.value) {
                let (a, b) = ordered(first, other);
                return Err(MergeError::Conflict { path: path.to_vec(), a: Box::new(a), b: Box::new(b) });
            }
            winners.push(first.clone());
            Ok(Some(v.clone()))
        }
        (Some(s), Some(r)) if s == r => {
            let scalar = scalars.into_iter().find(|l| l.level == s).unwrap();
            let record = markers.into_iter().chain(deeper).find(|l| l.level == r).unwrap();
            Err(MergeError::Shape {
                path: path.to_vec(),
                scalar: Box::new(scalar.clone()),
                record: Box::new(record.clone()),
            })
        }
        _ => {
            let mut map = BTreeMap::new();
            let mut start = 0;
            let deeper: Vec<Leaf> = deeper.into_iter().cloned().collect();
            while start < deeper.len() {
                let key = &deeper[start].path[depth];
                let end = start + deeper[start..].iter().take_while(|l| &l.path[depth] == key).count();
                if let Some(v) = resolve(&deeper[start..end], depth + 1, shape, winners)? {
                    map.insert(key.clone(), v);
                }
                start = end;
            }
            if map.is_empty()
                && let Some(m) = markers.iter().max_by_key(|l| l.level)
            {
                winners.push((*m).clone());
            }
            Ok(Some(Value::record(map)))
        }
    }
}

fn union(leaves: &[&Leaf], winners: &mut Vec<Leaf>) -> Value {
    let mut items = Vec::new();
    for leaf in leaves {
        match &leaf.value {
            LeafValue::Scalar(Value::List(xs)) => items.extend(xs.iter().cloned()),
            LeafValue::Scalar(v) => items.push(v.clone()),
            LeafValue::Record => unreachable!("markers are not scalars"),
        }
        winners.push((*leaf).clone());
    }
    items.sort();
    items.dedup();
    Value::list(items)
}

/// Report the pair in a stable order so errors don't depend on file order.
fn ordered(a: &Leaf, b: &Leaf) -> (Leaf, Leaf) {
    if a.span <= b.span { (a.clone(), b.clone()) } else { (b.clone(), a.clone()) }
}

/// Flatten a value into leaves under `prefix`.
pub fn flatten(value: &Value, prefix: &mut Vec<String>, level: Level, span: Span, out: &mut Vec<Leaf>) {
    match value {
        Value::Record(map) if !map.is_empty() => {
            for (k, v) in map.iter() {
                prefix.push(k.clone());
                flatten(v, prefix, level, span, out);
                prefix.pop();
            }
        }
        Value::Record(_) => out.push(Leaf { path: prefix.clone(), value: LeafValue::Record, level, span, via: None }),
        v => out.push(Leaf { path: prefix.clone(), value: LeafValue::Scalar(v.clone()), level, span, via: None }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(n: u32) -> Span {
        Span { file: n, lo: 0, hi: 0 }
    }

    fn leaf(path: &str, v: Value, level: Level, file: u32) -> Leaf {
        Leaf {
            path: if path.is_empty() { vec![] } else { path.split('.').map(String::from).collect() },
            value: LeafValue::Scalar(v),
            level,
            span: span(file),
            via: None,
        }
    }

    fn get<'a>(v: &'a Value, path: &str) -> &'a Value {
        path.split('.').fold(v, |v, k| &v.as_record().unwrap()[k])
    }

    #[test]
    fn highest_level_wins_per_field() {
        let leaves = vec![
            leaf("font.size", Value::Int(10), Level::Inherited, 0),
            leaf("font.family", Value::str("Mono"), Level::Inherited, 0),
            leaf("font.size", Value::Int(12), Level::Default, 1),
            leaf("font.size", Value::Int(13), Level::Plain, 2),
        ];
        let m = merge(leaves, &NoSets).unwrap();
        assert_eq!(get(&m.value, "font.size"), &Value::Int(13));
        assert_eq!(get(&m.value, "font.family"), &Value::str("Mono"));
    }

    #[test]
    fn same_level_different_values_conflict_naming_both() {
        let leaves = vec![leaf("size", Value::Int(13), Level::Plain, 2), leaf("size", Value::Int(12), Level::Plain, 1)];
        let err = merge(leaves, &NoSets).unwrap_err();
        let [a, b] = err.spans();
        assert_eq!((a.span.file, b.span.file), (1, 2));
        assert!(matches!(err, MergeError::Conflict { .. }));
    }

    #[test]
    fn same_level_equal_values_agree() {
        let leaves = vec![leaf("x", Value::str("a"), Level::Plain, 1), leaf("x", Value::str("a"), Level::Plain, 2)];
        assert!(merge(leaves, &NoSets).is_ok());
    }

    #[test]
    fn result_is_independent_of_order() {
        let base = vec![
            leaf("a", Value::Int(1), Level::Default, 0),
            leaf("a", Value::Int(2), Level::Plain, 1),
            leaf("b.c", Value::Int(3), Level::Inherited, 2),
            leaf("b.d", Value::Int(4), Level::Override, 3),
            leaf("b.c", Value::Int(5), Level::Plain, 4),
        ];
        let want = merge(base.clone(), &NoSets).unwrap();
        let mut rev = base.clone();
        rev.reverse();
        assert_eq!(merge(rev, &NoSets).unwrap(), want);
        let mut rot = base;
        rot.rotate_left(2);
        assert_eq!(merge(rot, &NoSets).unwrap(), want);
    }

    #[test]
    fn override_replaces_a_whole_record_and_shape_ties_error() {
        let leaves = vec![leaf("a.b", Value::Int(1), Level::Plain, 0), leaf("a", Value::Int(5), Level::Override, 1)];
        let m = merge(leaves, &NoSets).unwrap();
        assert_eq!(get(&m.value, "a"), &Value::Int(5));

        let leaves = vec![leaf("a.b", Value::Int(1), Level::Plain, 0), leaf("a", Value::Int(5), Level::Plain, 1)];
        assert!(matches!(merge(leaves, &NoSets), Err(MergeError::Shape { .. })));

        let leaves = vec![leaf("a.b", Value::Int(1), Level::Plain, 0), leaf("a", Value::Int(5), Level::Default, 1)];
        let m = merge(leaves, &NoSets).unwrap();
        assert_eq!(get(&m.value, "a.b"), &Value::Int(1));
    }

    #[test]
    fn sets_union_at_the_winning_level() {
        struct AllSets;
        impl Shape for AllSets {
            fn is_set(&self, path: &[String]) -> bool {
                path == ["ext"]
            }
        }
        let l = |xs: &[&str], level, file| {
            leaf("ext", Value::list(xs.iter().map(|x| Value::str(*x)).collect()), level, file)
        };
        let leaves = vec![
            l(&["nu", "nix"], Level::Plain, 0),
            l(&["toml", "nix"], Level::Plain, 1),
            l(&["dropped"], Level::Default, 2),
        ];
        let m = merge(leaves, &AllSets).unwrap();
        let want = Value::list(vec![Value::str("nix"), Value::str("nu"), Value::str("toml")]);
        assert_eq!(get(&m.value, "ext"), &want);
    }

    #[test]
    fn empty_record_marker_survives() {
        let leaves = vec![Leaf {
            path: vec!["x".into()],
            value: LeafValue::Record,
            level: Level::Plain,
            span: span(0),
            via: None,
        }];
        let m = merge(leaves, &NoSets).unwrap();
        assert_eq!(get(&m.value, "x"), &Value::record(BTreeMap::new()));
    }
}
