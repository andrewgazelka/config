//! Component types: checking merged values, and typing an entity's subtree
//! key by key.

use std::collections::BTreeMap;
use std::fmt;

use crate::syntax::ast::TypeExpr;
use crate::value::Value;

/// Where a value fails its type: `path` names the value in the message,
/// `at` is the value whose source location the error points to.
#[derive(Clone, Debug, PartialEq)]
pub struct Mismatch {
    pub path: Vec<String>,
    pub at: Vec<String>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Any,
    /// A marker component written as a bare name.
    Tag,
    Null,
    Bool,
    Int,
    Number,
    String,
    Lit(String),
    List(Box<Ty>),
    Set(Box<Ty>),
    Map(Box<Ty>),
    /// `rest` types the keys not in `fields`; `None` refuses them.
    Record {
        fields: BTreeMap<String, (Ty, bool)>,
        rest: Option<Box<Ty>>,
    },
    Union(Vec<Ty>),
}

impl Ty {
    /// Resolve names against built-ins and `type` aliases.
    pub fn from_expr(t: &TypeExpr, aliases: &BTreeMap<String, TypeExpr>) -> Result<Ty, String> {
        Self::resolve(t, aliases, 0)
    }

    fn resolve(t: &TypeExpr, aliases: &BTreeMap<String, TypeExpr>, depth: u32) -> Result<Ty, String> {
        if depth > 32 {
            return Err("type aliases refer to themselves".into());
        }
        let go = |t: &TypeExpr| Self::resolve(t, aliases, depth + 1).map(Box::new);
        Ok(match t {
            TypeExpr::Named(n) => match n.name.as_ref() {
                "any" => Ty::Any,
                "tag" => Ty::Tag,
                "null" => Ty::Null,
                "bool" => Ty::Bool,
                "int" => Ty::Int,
                "number" => Ty::Number,
                "string" => Ty::String,
                other => match aliases.get(other) {
                    Some(t) => Self::resolve(t, aliases, depth + 1)?,
                    None => return Err(format!("unknown type `{other}`")),
                },
            },
            TypeExpr::Literal(s) => Ty::Lit(s.clone()),
            TypeExpr::List(t) => Ty::List(go(t)?),
            TypeExpr::Set(t) => Ty::Set(go(t)?),
            TypeExpr::Map(t) => Ty::Map(go(t)?),
            TypeExpr::Record { fields, rest } => Ty::Record {
                fields: fields
                    .iter()
                    .map(|(k, t, opt)| Ok((k.clone(), (Self::resolve(t, aliases, depth + 1)?, *opt))))
                    .collect::<Result<_, String>>()?,
                rest: rest.as_deref().map(go).transpose()?,
            },
            TypeExpr::Union(ts) => {
                Ty::Union(ts.iter().map(|t| Self::resolve(t, aliases, depth + 1)).collect::<Result<_, _>>()?)
            }
        })
    }

    /// Check `v`; the error gives the field path of the offending value (an
    /// unknown key's own path) and what was expected there.
    pub fn check_path(&self, v: &Value) -> Result<(), Mismatch> {
        self.check_at(v, &mut Vec::new())
    }

    fn check_at(&self, v: &Value, path: &mut Vec<String>) -> Result<(), Mismatch> {
        let fail = |path: &Vec<String>, ty: &Ty| {
            Err(Mismatch { path: path.clone(), at: path.clone(), message: format!("expected {ty}, found {}", show(v)) })
        };
        match (self, v) {
            (Ty::Any, _) => Ok(()),
            (Ty::Tag, Value::Bool(true)) => Ok(()),
            (Ty::Null, Value::Null) | (Ty::Bool, Value::Bool(_)) | (Ty::Int, Value::Int(_)) => Ok(()),
            (Ty::Number, Value::Int(_) | Value::Float(_)) | (Ty::String, Value::Str(_)) => Ok(()),
            (Ty::Lit(l), Value::Str(s)) if l.as_str() == s.as_ref() => Ok(()),
            (Ty::List(t) | Ty::Set(t), Value::List(items)) => {
                for (i, item) in items.iter().enumerate() {
                    path.push(i.to_string());
                    t.check_at(item, path)?;
                    path.pop();
                }
                Ok(())
            }
            (Ty::Map(t), Value::Record(map)) => {
                for (k, item) in map.iter() {
                    path.push(k.clone());
                    t.check_at(item, path)?;
                    path.pop();
                }
                Ok(())
            }
            (Ty::Record { fields, rest }, Value::Record(map)) => {
                for (k, item) in map.iter() {
                    match fields.get(k).map(|(t, _)| t).or(rest.as_deref()) {
                        Some(t) => {
                            path.push(k.clone());
                            t.check_at(item, path)?;
                            path.pop();
                        }
                        None => {
                            let known: Vec<&str> = fields.keys().map(String::as_str).collect();
                            let hint = match closest(k, &known) {
                                Some(c) => format!("; did you mean `{c}`?"),
                                None if known.len() <= 12 => format!("; known fields: {}", known.join(", ")),
                                None => String::new(),
                            };
                            let mut at = path.clone();
                            at.push(k.clone());
                            return Err(Mismatch {
                                path: path.clone(),
                                at,
                                message: format!("unknown field `{k}`{hint}"),
                            });
                        }
                    }
                }
                for (k, (_, optional)) in fields {
                    if !optional && !map.contains_key(k) {
                        return Err(Mismatch {
                            path: path.clone(),
                            at: path.clone(),
                            message: format!("missing field `{k}`"),
                        });
                    }
                }
                Ok(())
            }
            (Ty::Union(ts), _) => {
                if ts.iter().any(|t| t.check_at(v, &mut path.clone()).is_ok()) {
                    Ok(())
                } else {
                    fail(path, self)
                }
            }
            _ => fail(path, self),
        }
    }

    /// This type, or the members of a union, flattened.
    pub fn members(&self) -> Vec<&Ty> {
        match self {
            Ty::Union(ts) => ts.iter().flat_map(Ty::members).collect(),
            t => vec![t],
        }
    }

    /// The type of key `k` inside a scope of this type, if it admits one.
    fn field(&self, k: &str) -> Option<&Ty> {
        match self {
            Ty::Any => Some(self),
            Ty::Map(t) => Some(t),
            Ty::Record { fields, rest } => fields.get(k).map(|(t, _)| t).or(rest.as_deref()),
            _ => None,
        }
    }

    /// Descend `path` key by key: the types its value may have, or why no
    /// type admits it (with the index of the key that left the type).
    pub fn descend<'t>(&'t self, path: &[&str]) -> Result<Vec<&'t Ty>, (usize, String)> {
        let mut cur = self.members();
        for (i, k) in path.iter().enumerate() {
            let next: Vec<&Ty> = cur.iter().filter_map(|t| t.field(k)).flat_map(|t| t.members()).collect();
            if next.is_empty() {
                return Err((i, no_field(&cur, k)));
            }
            cur = next;
        }
        Ok(cur)
    }

    /// Whether a scope (an entity) may have this type, and its required keys
    /// are among `present`. Some candidate must pass.
    pub fn check_scope(cands: &[&Ty], present: &dyn Fn(&str) -> bool) -> Result<(), String> {
        let mut missing = None;
        for t in cands {
            match t {
                Ty::Any | Ty::Map(_) => return Ok(()),
                Ty::Record { fields, .. } => match fields.iter().find(|(k, (_, opt))| !opt && !present(k)) {
                    None => return Ok(()),
                    Some((k, _)) => missing = missing.or(Some(format!("missing field `{k}`"))),
                },
                _ => {}
            }
        }
        Err(missing.unwrap_or_else(|| format!("expected {}, found a scope", show_all(cands))))
    }
}

fn show_all(cands: &[&Ty]) -> String {
    cands.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(" | ")
}

fn no_field(cands: &[&Ty], k: &str) -> String {
    let mut known: Vec<&str> = cands
        .iter()
        .filter_map(|t| match t {
            Ty::Record { fields, .. } => Some(fields.keys().map(String::as_str)),
            _ => None,
        })
        .flatten()
        .collect();
    if known.is_empty() && !cands.iter().any(|t| matches!(t, Ty::Record { .. })) {
        return format!("`{k}` is inside a value of type {}, not a scope", show_all(cands));
    }
    known.sort_unstable();
    known.dedup();
    let hint = match closest(k, &known) {
        Some(c) => format!("; did you mean `{c}`?"),
        None if !known.is_empty() && known.len() <= 12 => format!("; known fields: {}", known.join(", ")),
        None => String::new(),
    };
    format!("unknown field `{k}`{hint}")
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Any => f.write_str("any"),
            Ty::Tag => f.write_str("tag"),
            Ty::Null => f.write_str("null"),
            Ty::Bool => f.write_str("bool"),
            Ty::Int => f.write_str("int"),
            Ty::Number => f.write_str("number"),
            Ty::String => f.write_str("string"),
            Ty::Lit(s) => write!(f, "{s:?}"),
            Ty::List(t) => write!(f, "list({t})"),
            Ty::Set(t) => write!(f, "set({t})"),
            Ty::Map(t) => write!(f, "map({t})"),
            Ty::Record { .. } => f.write_str("a record"),
            Ty::Union(ts) => {
                let parts: Vec<String> = ts.iter().map(|t| t.to_string()).collect();
                f.write_str(&parts.join(" | "))
            }
        }
    }
}

fn show(v: &Value) -> String {
    match v {
        Value::Record(_) | Value::List(_) => v.kind().into(),
        _ => v.to_string(),
    }
}

/// The known name nearest to `word`, if it is close enough to be a typo.
pub fn closest<'a>(word: &str, known: &[&'a str]) -> Option<&'a str> {
    known
        .iter()
        .map(|k| (edit_distance(word, k), *k))
        .filter(|(d, k)| *d <= (k.len().max(word.len()) / 3).max(1))
        .min()
        .map(|(_, k)| k)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}
