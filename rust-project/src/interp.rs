use crate::model::ProgramLoadContext;
use anyhow::{anyhow, Result};
use chrono::Local;
use serde_json::{Map, Value};
use std::fs;

pub const INSERT_START: char = '{';
pub const INSERT_STOP: char = '}';
pub const ESCAPE: char = '\\';

pub fn get_simple_insertkey(content: &str) -> Option<String> {
    let mut depth = 0;
    let chars: Vec<char> = content.chars().collect();
    if chars.len() < 2 || chars.first()? != &INSERT_START || chars.last()? != &INSERT_STOP {
        return None;
    }
    for (i, c) in chars.iter().enumerate() {
        if *c == INSERT_STOP {
            depth -= 1;
        }
        if (depth == 0) != (i == 0 || i == chars.len() - 1) {
            return None;
        }
        if *c == INSERT_START {
            depth += 1;
        }
    }
    Some(chars[1..chars.len() - 1].iter().collect())
}

pub fn interpolate_inserts(
    inserts: &Map<String, Value>,
    content: &str,
    ctx: &ProgramLoadContext,
) -> Result<Value> {
    let mut s = content.to_string();

    let escaped_start = format!("{}{}", ESCAPE, INSERT_START);
    let escaped_stop = format!("{}{}", ESCAPE, INSERT_STOP);
    let replaced_start = ".〠".to_string();
    let replaced_stop = "〠.".to_string();
    s = s.replace(&escaped_start, &replaced_start);
    s = s.replace(&escaped_stop, &replaced_stop);

    if let Some(insertkey) = get_simple_insertkey(&s) {
        if let Some(subkey) = get_simple_insertkey(&insertkey) {
            let inner = interpolate_inserts(inserts, &format!("{}{}{}", INSERT_START, subkey, INSERT_STOP), ctx)?;
            return get_interpdata(inserts, &value_to_string(&inner), ctx);
        }
        let inner = interpolate_inserts(inserts, &insertkey, ctx)?;
        return get_interpdata(inserts, &value_to_string(&inner), ctx);
    }

    while s.contains(INSERT_START) {
        let n_starts = s.matches(INSERT_START).count() - s.matches(&escaped_start).count();
        let n_stops = s.matches(INSERT_STOP).count() - s.matches(&escaped_stop).count();
        if n_starts != n_stops {
            return Err(anyhow!(
                "Interpolation error: uneven number of '{{' and '}}' in: {s}"
            ));
        }
        let outer_from = s.rfind(INSERT_START).unwrap();
        let inner_to = s[outer_from + 1..]
            .find(INSERT_STOP)
            .map(|i| i + outer_from + 1)
            .unwrap();
        let inner = s[outer_from + 1..inner_to]
            .replace(&replaced_start, &escaped_start)
            .replace(&replaced_stop, &escaped_stop);
        let insert_value = get_interpdata(inserts, &inner, ctx)?;
        let insert_str = match insert_value {
            Value::String(ref x) => x.clone(),
            Value::Number(ref n) => n.to_string(),
            Value::Array(ref arr) => arr.iter().map(value_to_string).collect::<Vec<_>>().join(""),
            _ => {
                return Err(anyhow!(
                    "Trying to interpolate '{inner}' of unsupported type"
                ))
            }
        };
        s = format!("{}{}{}", &s[..outer_from], insert_str, &s[inner_to + 1..]);
        s = s.replace(&escaped_start, &replaced_start);
        s = s.replace(&escaped_stop, &replaced_stop);
    }

    s = s.replace(&replaced_start, &escaped_start);
    s = s.replace(&replaced_stop, &escaped_stop);
    Ok(Value::String(s))
}

pub fn get_interpdata(
    inserts: &Map<String, Value>,
    insertkey: &str,
    ctx: &ProgramLoadContext,
) -> Result<Value> {
    match insertkey {
        "HH:MM" => {
            let now = Local::now();
            return Ok(Value::String(now.format("%H:%M").to_string()));
        }
        "HH:MM:SS" => {
            let now = Local::now();
            return Ok(Value::String(now.format("%H:%M:%S").to_string()));
        }
        "" => return Err(anyhow!("Tried to interpolate empty string ''")),
        _ => {}
    }

    if insertkey.starts_with("ARG") && insertkey[3..].chars().all(|c| c.is_ascii_digit()) {
        if let Some(v) = inserts.get(insertkey) {
            return Ok(v.clone());
        }
        return Err(anyhow!(
            "Argument interpolation key '{insertkey}' is used but not provided"
        ));
    }

    if let Some(v) = inserts.get(insertkey) {
        return Ok(v.clone());
    }

    if let Some(dir) = ctx.inserts_dir.as_ref() {
        let json5_path = dir.join(format!("{insertkey}.json5"));
        if json5_path.exists() {
            let raw = fs::read_to_string(&json5_path)?;
            let val: Value = json5::from_str(&raw)?;
            return Ok(recursive_escape(val));
        }
        let plain_path = dir.join(insertkey);
        if plain_path.exists() {
            let raw = fs::read_to_string(&plain_path)?;
            return Ok(recursive_escape(Value::String(raw.trim().to_string())));
        }
    }

    Err(anyhow!("Could not find variable '{insertkey}'"))
}

pub fn set_interpdata(inserts: &mut Map<String, Value>, key: &str, value: Value) {
    inserts.insert(key.to_string(), value);
}

pub fn delete_interpdata(inserts: &mut Map<String, Value>, key: &str) {
    inserts.remove(key);
}

pub fn recursive_unescape(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(
            s.replace(&format!("{ESCAPE}{INSERT_START}"), &INSERT_START.to_string())
                .replace(&format!("{ESCAPE}{INSERT_STOP}"), &INSERT_STOP.to_string()),
        ),
        Value::Array(arr) => Value::Array(arr.into_iter().map(recursive_unescape).collect()),
        Value::Object(obj) => Value::Object(
            obj.into_iter()
                .map(|(k, v)| (recursive_unescape(Value::String(k)).as_str().unwrap().to_string(), recursive_unescape(v)))
                .collect(),
        ),
        v => v,
    }
}

pub fn recursive_escape(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(
            s.replace(&INSERT_START.to_string(), &format!("{ESCAPE}{INSERT_START}"))
                .replace(&INSERT_STOP.to_string(), &format!("{ESCAPE}{INSERT_STOP}")),
        ),
        Value::Array(arr) => Value::Array(arr.into_iter().map(recursive_escape).collect()),
        Value::Object(obj) => Value::Object(
            obj.into_iter()
                .map(|(k, v)| (recursive_escape(Value::String(k)).as_str().unwrap().to_string(), recursive_escape(v)))
                .collect(),
        ),
        v => v,
    }
}

pub fn recursive_interpolate(
    inserts: &Map<String, Value>,
    value: Value,
    ctx: &ProgramLoadContext,
) -> Result<Value> {
    if let Value::String(s) = &value {
        if let Some(insertkey) = get_simple_insertkey(s) {
            let inner = match interpolate_inserts(
                inserts,
                &format!("{}{}{}", INSERT_START, insertkey, INSERT_STOP),
                ctx,
            ) {
                Ok(v) => v,
                Err(_) => return Ok(Value::String(s.clone())),
            };
            return Ok(inner);
        }
    }

    match value {
        Value::String(s) => match interpolate_inserts(inserts, &s, ctx) {
            Ok(v) => Ok(v),
            Err(_) => Ok(Value::String(s)),
        },
        Value::Array(arr) => Ok(Value::Array(
            arr.into_iter()
                .map(|v| recursive_interpolate(inserts, v, ctx))
                .collect::<Result<Vec<_>>>()?,
        )),
        Value::Object(obj) => {
            if let Some(cmd) = obj.get("cmd").and_then(Value::as_str) {
                if cmd == "goto_map" || cmd == "replace_map" {
                    return Ok(Value::Object(obj));
                }
                if cmd == "for" || cmd == "serial" || cmd == "parallel_wait" || cmd == "parallel_race" {
                    let mut obj = obj;
                    if let Some(tasks_val) = obj.get_mut("tasks") {
                        if let Some(s) = tasks_val.as_str() {
                            if let Some(insertkey) = get_simple_insertkey(s) {
                                let v = get_interpdata(inserts, &insertkey, ctx)?;
                                *tasks_val = v;
                            }
                        } else if let Some(arr) = tasks_val.as_array_mut() {
                            for i in 0..arr.len() {
                                if let Some(s) = arr[i].as_str() {
                                    if let Some(insertkey) = get_simple_insertkey(s) {
                                        let v = get_interpdata(inserts, &insertkey, ctx)?;
                                        arr[i] = v;
                                    }
                                }
                            }
                        }
                    }
                    return Ok(Value::Object(obj));
                }
            }
            let mut out = Map::new();
            for (k, v) in obj {
                let new_k_val = recursive_interpolate(inserts, Value::String(k), ctx)?;
                let new_k = value_to_string(&new_k_val);
                let new_v = recursive_interpolate(inserts, v, ctx)?;
                out.insert(new_k, new_v);
            }
            Ok(Value::Object(out))
        }
        v => Ok(v),
    }
}

pub fn extract_insert_keys(value: &Value) -> Vec<String> {
    let mut keys = Vec::new();
    match value {
        Value::String(s) => {
            keys.extend(extract_from_str(s));
        }
        Value::Array(arr) => {
            for v in arr {
                keys.extend(extract_insert_keys(v));
            }
        }
        Value::Object(obj) => {
            for (k, v) in obj {
                keys.extend(extract_from_str(k));
                keys.extend(extract_insert_keys(v));
            }
        }
        _ => {}
    }
    keys
}

fn extract_from_str(s: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    let mut in_key = false;
    let mut escaped = false;
    for ch in s.chars() {
        if escaped {
            escaped = false;
            if in_key {
                current.push(ch);
            }
            continue;
        }
        if ch == ESCAPE {
            escaped = true;
            continue;
        }
        if ch == INSERT_START {
            depth += 1;
            if depth == 1 {
                in_key = true;
                current.clear();
                continue;
            }
        }
        if ch == INSERT_STOP {
            if depth == 1 && in_key {
                keys.push(current.clone());
                in_key = false;
                depth -= 1;
                continue;
            }
            if depth > 0 {
                depth -= 1;
            }
        }
        if in_key {
            current.push(ch);
        }
    }
    keys
}

pub fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(arr) => arr.iter().map(value_to_string).collect::<Vec<_>>().join(""),
        Value::Object(_) | Value::Null => serde_json::to_string(value).unwrap_or_default(),
    }
}
