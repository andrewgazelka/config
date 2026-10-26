//! Values to the bytes an app reads. Records render with their keys
//! sorted, so equal values always give equal bytes.

use crate::value::Value;

fn ensure_newline(mut s: String) -> String {
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

pub fn json(v: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(v).map(ensure_newline).map_err(|e| e.to_string())
}

pub fn toml(v: &Value) -> Result<String, String> {
    if v.as_record().is_none() {
        return Err(format!("toml needs a record at the top, found {}", v.kind()));
    }
    toml::to_string_pretty(v).map(ensure_newline).map_err(|e| e.to_string())
}

/// `key = value` lines from a list of `[key, value]` pairs or a record.
pub fn kv(v: &Value) -> Result<String, String> {
    let mut out = String::new();
    let mut line = |k: &str, v: &Value| -> Result<(), String> {
        let shown = v.display_scalar().ok_or_else(|| format!("`{k}` must be a scalar, found {}", v.kind()))?;
        out.push_str(&format!("{k} = {shown}\n"));
        Ok(())
    };
    match v {
        Value::Record(map) => {
            for (k, v) in map.iter() {
                line(k, v)?;
            }
        }
        Value::List(items) => {
            for item in items.iter() {
                match item.as_list() {
                    Some([Value::Str(k), v]) => line(k, v)?,
                    _ => return Err(format!("kv expects [key, value] pairs, found {item}")),
                }
            }
        }
        _ => return Err(format!("kv expects a record or a list of pairs, found {}", v.kind())),
    }
    Ok(out)
}

/// git-config syntax: a record of sections; a record inside a section is a subsection; a list repeats a key.
pub fn ini(v: &Value) -> Result<String, String> {
    let sections = v.as_record().ok_or("ini expects a record of sections")?;
    let mut blocks = Vec::new();
    for (name, body) in sections.iter() {
        let body = body.as_record().ok_or_else(|| format!("section `{name}` must be a record"))?;
        let (subs, keys): (Vec<_>, Vec<_>) = body.iter().partition(|(_, v)| v.as_record().is_some());
        if !keys.is_empty() {
            blocks.push((name.clone(), ini_keys(&keys)?));
        }
        for (sub, body) in subs {
            let pairs: Vec<_> = body.as_record().unwrap().iter().collect();
            blocks.push((format!("{name} \"{}\"", escape(sub)), ini_keys(&pairs)?));
        }
    }
    blocks.sort();
    let text: Vec<String> = blocks.into_iter().map(|(name, keys)| format!("[{name}]\n{keys}")).collect();
    Ok(text.join("\n"))
}

fn ini_keys(pairs: &[(&String, &Value)]) -> Result<String, String> {
    let mut out = String::new();
    for (k, v) in pairs {
        let values: Vec<&Value> = match v {
            Value::List(items) => items.iter().collect(),
            v => vec![v],
        };
        for v in values {
            let shown = match v {
                Value::Str(s) => format!("\"{}\"", escape(s)),
                Value::Bool(_) | Value::Int(_) | Value::Float(_) => v.display_scalar().unwrap(),
                _ => return Err(format!("git config value `{k}` must be a scalar, found {}", v.kind())),
            };
            out.push_str(&format!("\t{k} = {shown}\n"));
        }
    }
    Ok(out)
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\t', "\\t")
}
