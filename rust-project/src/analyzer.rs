use crate::interp::{extract_insert_keys, get_interpdata, get_simple_insertkey};
use crate::model::{Program, ProgramLoadContext, Task};
use anyhow::{anyhow, Result};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug)]
pub struct Diagnostic {
    pub message: String,
    pub label: Option<String>,
    pub line: Option<i64>,
}

pub fn analyze_program(program: &Program, ctx: &ProgramLoadContext) -> Result<()> {
    let mut diags = Vec::new();

    let default_inserts = program
        .default_state
        .get("inserts")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut named = HashSet::new();
    for name in program.named_tasks.keys() {
        named.insert(name.clone());
    }

    analyze_task_list(
        &program.order,
        "order",
        &named,
        &default_inserts,
        ctx,
        &mut diags,
    );

    for (name, task) in &program.named_tasks {
        analyze_task_list(
            &[task.clone()],
            &format!("named_tasks.{name}"),
            &named,
            &default_inserts,
            ctx,
            &mut diags,
        );
    }

    if diags.is_empty() {
        Ok(())
    } else {
        let mut msg = String::from("Program validation failed:\n");
        for d in diags {
            let line = d.line.map(|l| format!("line {l}")).unwrap_or_default();
            let label = d.label.unwrap_or_default();
            msg.push_str(&format!(" - {line} {label} {}\n", d.message));
        }
        Err(anyhow!(msg))
    }
}

fn analyze_task_list(
    tasks: &[Task],
    scope_name: &str,
    named_tasks: &HashSet<String>,
    default_inserts: &Map<String, Value>,
    ctx: &ProgramLoadContext,
    diags: &mut Vec<Diagnostic>,
) {
    let labels = collect_labels_for_list(tasks, diags);
    for task in tasks {
        validate_task(
            task,
            scope_name,
            named_tasks,
            &labels,
            default_inserts,
            ctx,
            diags,
        );
        if let Some(subtasks) = task.get("tasks").and_then(Value::as_array) {
            let subtasks = subtasks
                .iter()
                .filter_map(|v| super_task(v).ok())
                .collect::<Vec<_>>();
            if !subtasks.is_empty() {
                analyze_task_list(
                    &subtasks,
                    scope_name,
                    named_tasks,
                    default_inserts,
                    ctx,
                    diags,
                );
            }
        }
    }
}

fn validate_task(
    task: &Task,
    scope_name: &str,
    named_tasks: &HashSet<String>,
    labels: &HashSet<String>,
    default_inserts: &Map<String, Value>,
    ctx: &ProgramLoadContext,
    diags: &mut Vec<Diagnostic>,
) {
    let cmd = match task.get("cmd").and_then(Value::as_str) {
        Some(c) => c,
        None => {
            diags.push(diag(task, "Task missing 'cmd' string".to_string()));
            return;
        }
    };

    match cmd {
        "print" => {
            require_fields(task, &["text"], diags);
            require_string(task, "text", default_inserts, ctx, diags);
        }
        "clear" => {}
        "sleep" => {
            require_fields(task, &["seconds"], diags);
            require_number_or_string(task, "seconds", default_inserts, ctx, diags);
        }
        "set" => {
            require_fields(task, &["item", "output_name"], diags);
            require_string(task, "output_name", default_inserts, ctx, diags);
        }
        "unescape" => {
            require_fields(task, &["item", "output_name"], diags);
            require_string(task, "output_name", default_inserts, ctx, diags);
        }
        "write" => {
            require_fields(task, &["item", "path"], diags);
            require_string(task, "path", default_inserts, ctx, diags);
        }
        "show_inserts" => {}
        "random_choice" => {
            require_fields(task, &["list", "output_name"], diags);
            require_array(task, "list", default_inserts, ctx, diags);
            require_string(task, "output_name", default_inserts, ctx, diags);
            if let Some(list) = get_static_array(task.get("list"), default_inserts, ctx) {
                if list.is_empty() {
                    diags.push(diag(task, "random_choice list is empty".to_string()));
                }
            }
        }
        "list_join" => {
            require_fields(task, &["list", "before", "between", "after", "output_name"], diags);
            require_array(task, "list", default_inserts, ctx, diags);
            require_string(task, "before", default_inserts, ctx, diags);
            require_string(task, "between", default_inserts, ctx, diags);
            require_string(task, "after", default_inserts, ctx, diags);
            require_string(task, "output_name", default_inserts, ctx, diags);
        }
        "list_concat" => {
            require_fields(task, &["lists", "output_name"], diags);
            require_array(task, "lists", default_inserts, ctx, diags);
            require_string(task, "output_name", default_inserts, ctx, diags);
            if let Some(arr) = get_static_array(task.get("lists"), default_inserts, ctx) {
                for item in arr {
                    if item.as_array().is_some() {
                        continue;
                    }
                    if is_simple_interpolation(&item) {
                        continue;
                    }
                    if let Some(resolved) = resolve_simple_value(&item, default_inserts, ctx) {
                        if resolved.as_array().is_some() {
                            continue;
                        }
                    }
                    diags.push(diag(
                        task,
                        "list_concat.lists must contain only arrays or simple interpolations".to_string(),
                    ));
                    break;
                }
            }
        }
        "list_append" => {
            require_fields(task, &["list", "item", "output_name"], diags);
            require_array(task, "list", default_inserts, ctx, diags);
            require_string(task, "output_name", default_inserts, ctx, diags);
        }
        "list_remove" => {
            require_fields(task, &["list", "item", "output_name"], diags);
            require_array(task, "list", default_inserts, ctx, diags);
            require_string(task, "output_name", default_inserts, ctx, diags);
        }
        "list_index" => {
            require_fields(task, &["list", "index", "output_name"], diags);
            require_array(task, "list", default_inserts, ctx, diags);
            require_int_or_string(task, "index", default_inserts, ctx, diags);
            require_string(task, "output_name", default_inserts, ctx, diags);
            if let Some(list) = get_static_array(task.get("list"), default_inserts, ctx) {
                if let Some(idx) = literal_int(task.get("index")) {
                    if idx == 0 {
                        diags.push(diag(task, "list_index index 0 is invalid (1-based)".to_string()));
                    } else if is_index_out_of_bounds(idx, list.len()) {
                        diags.push(diag(task, "list_index index out of bounds".to_string()));
                    }
                }
            }
        }
        "list_slice" => {
            require_fields(task, &["list", "from_index", "to_index", "output_name"], diags);
            require_array(task, "list", default_inserts, ctx, diags);
            require_int_or_string(task, "from_index", default_inserts, ctx, diags);
            require_int_or_string(task, "to_index", default_inserts, ctx, diags);
            require_string(task, "output_name", default_inserts, ctx, diags);
            if let Some(list) = get_static_array(task.get("list"), default_inserts, ctx) {
                if let Some(from_idx) = literal_int(task.get("from_index")) {
                    if from_idx == 0 {
                        diags.push(diag(task, "list_slice from_index 0 is invalid (1-based)".to_string()));
                    } else if is_index_out_of_bounds(from_idx, list.len()) {
                        diags.push(diag(task, "list_slice from_index out of bounds".to_string()));
                    }
                }
                if let Some(to_idx) = literal_int(task.get("to_index")) {
                    if to_idx != 0 && is_index_out_of_bounds(to_idx, list.len()) {
                        diags.push(diag(task, "list_slice to_index out of bounds".to_string()));
                    }
                }
            }
        }
        "user_input" => {
            require_fields(task, &["prompt", "output_name"], diags);
            require_string(task, "prompt", default_inserts, ctx, diags);
            require_string(task, "output_name", default_inserts, ctx, diags);
        }
        "user_choice" => {
            require_fields(task, &["list", "description", "output_name"], diags);
            require_array(task, "list", default_inserts, ctx, diags);
            require_string(task, "description", default_inserts, ctx, diags);
            require_string(task, "output_name", default_inserts, ctx, diags);
        }
        "await_insert" => {
            require_fields(task, &["name"], diags);
            require_string(task, "name", default_inserts, ctx, diags);
        }
        "label" => {
            require_fields(task, &["name"], diags);
            require_string(task, "name", default_inserts, ctx, diags);
        }
        "goto" => {
            require_fields(task, &["name"], diags);
            require_string(task, "name", default_inserts, ctx, diags);
            if let Some(target) = task.get("name").and_then(Value::as_str) {
                if is_literal_no_braces(target) && target != "CONTINUE" && !labels.contains(target) {
                    diags.push(diag(
                        task,
                        format!("goto target '{target}' not found in {scope_name}"),
                    ));
                }
            }
        }
        "goto_map" => {
            require_fields(task, &["text", "target_maps"], diags);
            require_string(task, "text", default_inserts, ctx, diags);
            require_array(task, "target_maps", default_inserts, ctx, diags);
            if let Some(target_maps) = task.get("target_maps").and_then(Value::as_array) {
                if target_maps.is_empty() {
                    diags.push(diag(task, "goto_map.target_maps must not be empty".to_string()));
                }
                if let Some(text) = task.get("text").and_then(Value::as_str) {
                    ensure_balanced_interpolation(task, "text", text, diags);
                }
                let mut literal_keys: Vec<(String, String)> = Vec::new();
                for entry in target_maps {
                    let obj = match entry.as_object() {
                        Some(o) => o,
                        None => {
                            diags.push(diag(task, "target_maps entries must be objects".to_string()));
                            continue;
                        }
                    };
                    if obj.len() != 1 {
                        diags.push(diag(task, "target_maps entries must have 1 key".to_string()));
                        continue;
                    }
                    let (target_key, target_val) = obj.iter().next().unwrap();
                    if target_key.as_str().is_empty() {
                        diags.push(diag(task, "target_maps keys must be non-empty strings".to_string()));
                    }
                    ensure_balanced_interpolation(task, "target_maps key", target_key, diags);
                    if !is_string_or_simple_interpolation(target_val) {
                        diags.push(diag(task, "target_maps values must be strings".to_string()));
                        continue;
                    }
                    if let Some(val) = target_val.as_str() {
                        ensure_balanced_interpolation(task, "target_maps value", val, diags);
                    }
                    if let Some(val_str) = target_val.as_str() {
                        if is_literal_no_braces(target_key.as_str()) && is_literal_no_braces(val_str) {
                            literal_keys.push((target_key.clone(), val_str.to_string()));
                        }
                    }
                }
                if let Some(text) = task.get("text").and_then(Value::as_str) {
                    if is_literal_no_braces(text) && !literal_keys.is_empty() {
                        let mut matched = None;
                        for (key, val) in &literal_keys {
                            if wildcard_match(key, text) {
                                matched = Some(val.clone());
                                break;
                            }
                        }
                        if let Some(target) = matched {
                            if target != "CONTINUE" && !labels.contains(target.as_str()) {
                                diags.push(diag(
                                    task,
                                    format!("goto_map target '{target}' not found in {scope_name}"),
                                ));
                            }
                        } else {
                            diags.push(diag(
                                task,
                                format!("goto_map has no matches for literal text '{text}'"),
                            ));
                        }
                    }
                }
            }
        }
        "replace_map" => {
            require_fields(task, &["item", "output_name", "wildcard_maps"], diags);
            require_string(task, "output_name", default_inserts, ctx, diags);
            require_array(task, "wildcard_maps", default_inserts, ctx, diags);
            if let Some(maps) = task.get("wildcard_maps").and_then(Value::as_array) {
                for entry in maps {
                    let obj = match entry.as_object() {
                        Some(o) => o,
                        None => {
                            diags.push(diag(task, "wildcard_maps entries must be objects".to_string()));
                            continue;
                        }
                    };
                    if obj.len() != 1 {
                        diags.push(diag(task, "wildcard_maps entries must have 1 key".to_string()));
                        continue;
                    }
                    let (k, v) = obj.iter().next().unwrap();
                    ensure_balanced_interpolation(task, "wildcard_maps key", k, diags);
                    if let Some(val) = v.as_str() {
                        ensure_balanced_interpolation(task, "wildcard_maps value", val, diags);
                    } else if !is_simple_interpolation(v) {
                        diags.push(diag(task, "wildcard_maps values must be strings".to_string()));
                    }
                }
            }
        }
        "for" => {
            require_fields(task, &["name_list_map", "tasks"], diags);
            require_object(task, "name_list_map", default_inserts, ctx, diags);
            require_task_array(task, "tasks", default_inserts, ctx, diags);
            if let Some(map) = task.get("name_list_map").and_then(Value::as_object) {
                let mut static_lists = Vec::new();
                for (name, value) in map {
                    if let Some(arr) = get_static_array(Some(value), default_inserts, ctx) {
                        static_lists.push((name.clone(), arr.len()));
                        continue;
                    }
                    if value.as_str().is_some() && !is_simple_interpolation(value) {
                        diags.push(diag(
                            task,
                            format!("for.name_list_map value for '{name}' must be a list or simple interpolation"),
                        ));
                        return;
                    }
                    if !value.is_array() && value.as_str().is_none() {
                        diags.push(diag(
                            task,
                            format!("for.name_list_map value for '{name}' must be a list or simple interpolation"),
                        ));
                        return;
                    }
                }
                if static_lists.len() == map.len() && !static_lists.is_empty() {
                    let expected = static_lists[0].1;
                    if static_lists.iter().any(|(_, len)| *len != expected) {
                        diags.push(diag(task, "for lists have differing lengths".to_string()));
                    }
                }
            }
        }
        "serial" | "parallel_wait" | "parallel_race" => {
            require_fields(task, &["tasks"], diags);
            require_task_array(task, "tasks", default_inserts, ctx, diags);
        }
        "run_task" => {
            require_fields(task, &["task_name"], diags);
            require_string(task, "task_name", default_inserts, ctx, diags);
            if let Some(name) = task.get("task_name").and_then(Value::as_str) {
                if !named_tasks.contains(name) {
                    diags.push(diag(task, format!("run_task references unknown task '{name}'")));
                }
            }
        }
        "delete" | "delete_except" => {
            require_fields(task, &["wildcards"], diags);
            require_array(task, "wildcards", default_inserts, ctx, diags);
        }
        "math" => {
            require_fields(task, &["input", "output_name"], diags);
            require_string(task, "input", default_inserts, ctx, diags);
            require_string(task, "output_name", default_inserts, ctx, diags);
        }
        "chat" => {
            require_fields(task, &["messages", "output_name"], diags);
            require_array(task, "messages", default_inserts, ctx, diags);
            require_string(task, "output_name", default_inserts, ctx, diags);
            validate_voice_path(task, ctx, diags);
            if let Some(msgs) = get_static_array(task.get("messages"), default_inserts, ctx) {
                for msg in msgs {
                    let Some(obj) = msg.as_object() else { continue };
                    if let Some(content) = obj.get("content").and_then(Value::as_str) {
                        ensure_balanced_interpolation(task, "chat.messages.content", content, diags);
                    }
                }
            }
        }
        "speak" => {
            require_fields(task, &["text", "voice_path"], diags);
            require_string(task, "text", default_inserts, ctx, diags);
            require_string(task, "voice_path", default_inserts, ctx, diags);
            validate_voice_path(task, ctx, diags);
        }
        _ => diags.push(diag(task, format!("Unknown cmd '{cmd}'"))),
    }

    if cmd == "goto_map" && has_null_map_entry(task, "target_maps") {
        if let Some(text) = task.get("text").and_then(Value::as_str) {
            ensure_balanced_interpolation(task, "text", text, diags);
        }
    }
    if cmd == "replace_map" && has_null_map_entry(task, "wildcard_maps") {
        if let Some(item) = task.get("item").and_then(Value::as_str) {
            ensure_balanced_interpolation(task, "item", item, diags);
        }
    }
}

fn has_null_map_entry(task: &Task, field: &str) -> bool {
    let Some(arr) = task.get(field).and_then(Value::as_array) else {
        return false;
    };
    for entry in arr {
        let Some(obj) = entry.as_object() else {
            continue;
        };
        if obj.keys().any(|k| k == "NULL") {
            return true;
        }
    }
    false
}

fn validate_voice_path(task: &Task, ctx: &ProgramLoadContext, diags: &mut Vec<Diagnostic>) {
    let path = match task.get("voice_path").and_then(Value::as_str) {
        Some(p) if !p.is_empty() => p,
        _ => return,
    };
    if path.contains('{') || path.contains('}') {
        return;
    }
    let resolved = resolve_path_ctx(ctx, path);
    if !resolved.exists() {
        diags.push(diag(
            task,
            format!("voice_path does not exist: {}", resolved.display()),
        ));
        return;
    }
    if resolved.is_dir() {
        diags.push(diag(
            task,
            format!("voice_path is a directory: {}", resolved.display()),
        ));
    }
}

fn resolve_path_ctx(ctx: &ProgramLoadContext, path: &str) -> PathBuf {
    let expanded = shellexpand::tilde(path).to_string();
    let p = PathBuf::from(expanded);
    if p.is_absolute() {
        p
    } else {
        ctx.program_dir.join(p)
    }
}

fn wildcard_match(pattern: &str, s: &str) -> bool {
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            _ => regex.push_str(&regex::escape(&ch.to_string())),
        }
    }
    regex.push('$');
    regex::RegexBuilder::new(&regex)
        .dot_matches_new_line(true)
        .build()
        .map(|re| re.is_match(s))
        .unwrap_or(false)
}

fn require_fields(task: &Task, fields: &[&str], diags: &mut Vec<Diagnostic>) {
    for f in fields {
        if !task.contains_key(*f) {
            diags.push(diag(task, format!("Missing required field '{f}'")));
        }
    }
}

fn require_string(
    task: &Task,
    field: &str,
    default_inserts: &Map<String, Value>,
    ctx: &ProgramLoadContext,
    diags: &mut Vec<Diagnostic>,
) {
    if let Some(v) = task.get(field) {
        if v.is_string() {
            if let Some(resolved) = resolve_simple_value(v, default_inserts, ctx) {
                if resolved.is_string() {
                    return;
                }
                diags.push(diag(task, format!("Field '{field}' must be a string")));
                return;
            }
            return;
        }
        diags.push(diag(task, format!("Field '{field}' must be a string")));
    }
}

fn require_number_or_string(
    task: &Task,
    field: &str,
    default_inserts: &Map<String, Value>,
    ctx: &ProgramLoadContext,
    diags: &mut Vec<Diagnostic>,
) {
    if let Some(v) = task.get(field) {
        if v.is_string() || v.is_number() {
            return;
        }
        if let Some(resolved) = resolve_simple_value(v, default_inserts, ctx) {
            if resolved.is_string() || resolved.is_number() {
                return;
            }
            diags.push(diag(task, format!("Field '{field}' must be a number or string")));
            return;
        }
        if is_simple_interpolation(v) {
            return;
        }
        diags.push(diag(task, format!("Field '{field}' must be a number or string")));
    }
}

fn require_int_or_string(
    task: &Task,
    field: &str,
    default_inserts: &Map<String, Value>,
    ctx: &ProgramLoadContext,
    diags: &mut Vec<Diagnostic>,
) {
    if let Some(v) = task.get(field) {
        if v.as_i64().is_some() || v.is_string() {
            return;
        }
        if let Some(resolved) = resolve_simple_value(v, default_inserts, ctx) {
            if resolved.as_i64().is_some() || resolved.is_string() {
                return;
            }
            diags.push(diag(task, format!("Field '{field}' must be an int or string")));
            return;
        }
        if is_simple_interpolation(v) {
            return;
        }
        diags.push(diag(task, format!("Field '{field}' must be an int or string")));
    }
}

fn require_array(
    task: &Task,
    field: &str,
    default_inserts: &Map<String, Value>,
    ctx: &ProgramLoadContext,
    diags: &mut Vec<Diagnostic>,
) {
    if let Some(v) = task.get(field) {
        if v.is_array() {
            return;
        }
        if let Some(resolved) = resolve_simple_value(v, default_inserts, ctx) {
            if resolved.is_array() {
                return;
            }
            diags.push(diag(task, format!("Field '{field}' must be an array")));
            return;
        }
        if is_simple_interpolation(v) {
            return;
        }
        diags.push(diag(task, format!("Field '{field}' must be an array")));
    }
}

fn require_object(
    task: &Task,
    field: &str,
    default_inserts: &Map<String, Value>,
    ctx: &ProgramLoadContext,
    diags: &mut Vec<Diagnostic>,
) {
    if let Some(v) = task.get(field) {
        if v.is_object() {
            return;
        }
        if let Some(resolved) = resolve_simple_value(v, default_inserts, ctx) {
            if resolved.is_object() {
                return;
            }
            diags.push(diag(task, format!("Field '{field}' must be an object")));
            return;
        }
        if is_simple_interpolation(v) {
            return;
        }
        diags.push(diag(task, format!("Field '{field}' must be an object")));
    }
}

fn require_task_array(
    task: &Task,
    field: &str,
    default_inserts: &Map<String, Value>,
    ctx: &ProgramLoadContext,
    diags: &mut Vec<Diagnostic>,
) {
    if let Some(v) = task.get(field) {
        if let Some(arr) = v.as_array() {
            if arr.iter().any(|t| t.as_object().is_none()) {
                diags.push(diag(task, format!("Field '{field}' must be an array of objects")));
            }
            return;
        }
        if let Some(resolved) = resolve_simple_value(v, default_inserts, ctx) {
            if let Some(arr) = resolved.as_array() {
                if arr.iter().any(|t| t.as_object().is_none()) {
                    diags.push(diag(task, format!("Field '{field}' must be an array of objects")));
                }
                return;
            }
            diags.push(diag(task, format!("Field '{field}' must be an array of objects")));
            return;
        }
        if is_simple_interpolation(v) {
            return;
        }
        diags.push(diag(task, format!("Field '{field}' must be an array of objects")));
    }
}

fn is_simple_interpolation(value: &Value) -> bool {
    value
        .as_str()
        .and_then(|s| get_simple_insertkey(s))
        .is_some()
}

fn resolve_simple_value(
    value: &Value,
    default_inserts: &Map<String, Value>,
    ctx: &ProgramLoadContext,
) -> Option<Value> {
    let key = value.as_str().and_then(get_simple_insertkey)?;
    if key.starts_with("ARG") {
        return None;
    }
    get_interpdata(default_inserts, &key, ctx).ok()
}

fn collect_labels_for_list(tasks: &[Task], diags: &mut Vec<Diagnostic>) -> HashSet<String> {
    let mut labels = HashSet::new();
    for task in tasks {
        if task.get("cmd").and_then(Value::as_str) != Some("label") {
            continue;
        }
        let Some(name) = task.get("name").and_then(Value::as_str) else {
            diags.push(diag(task, "label.name must be a string".to_string()));
            continue;
        };
        if !labels.insert(name.to_string()) {
            diags.push(diag(task, format!("Label '{name}' is not unique in this task list")));
        }
    }
    labels
}

fn diag(task: &Task, message: String) -> Diagnostic {
    Diagnostic {
        message,
        label: task
            .get("traceback_label")
            .and_then(Value::as_str)
            .map(|s| s.to_string()),
        line: task.get("line").and_then(Value::as_i64),
    }
}

fn super_task(value: &Value) -> Result<Task> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow!("Not a task"))
}

fn scan_braces(s: &str) -> BraceScan {
    let mut depth = 0;
    let mut escaped = false;
    let mut has_unescaped = false;
    let mut balanced = true;
    for ch in s.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '{' {
            has_unescaped = true;
            depth += 1;
            continue;
        }
        if ch == '}' {
            has_unescaped = true;
            if depth == 0 {
                balanced = false;
            } else {
                depth -= 1;
            }
        }
    }
    if depth != 0 {
        balanced = false;
    }
    BraceScan {
        balanced,
        has_unescaped,
    }
}

#[derive(Debug, Clone, Copy)]
struct BraceScan {
    balanced: bool,
    has_unescaped: bool,
}

fn is_literal_no_braces(s: &str) -> bool {
    let scan = scan_braces(s);
    scan.balanced && !scan.has_unescaped
}

fn ensure_balanced_interpolation(task: &Task, field: &str, s: &str, diags: &mut Vec<Diagnostic>) {
    let scan = scan_braces(s);
    if !scan.balanced {
        diags.push(diag(
            task,
            format!("Field '{field}' has malformed interpolation (uneven braces)"),
        ));
    }
    if extract_insert_keys(&Value::String(s.to_string()))
        .iter()
        .any(|k| k.is_empty())
    {
        diags.push(diag(
            task,
            format!("Field '{field}' contains an empty interpolation key"),
        ));
    }
}

fn is_string_or_simple_interpolation(value: &Value) -> bool {
    value.is_string() || is_simple_interpolation(value)
}

fn get_static_array(
    value: Option<&Value>,
    default_inserts: &Map<String, Value>,
    ctx: &ProgramLoadContext,
) -> Option<Vec<Value>> {
    let v = value?;
    if let Some(arr) = v.as_array() {
        return Some(arr.clone());
    }
    if let Some(resolved) = resolve_simple_value(v, default_inserts, ctx) {
        if let Some(arr) = resolved.as_array() {
            return Some(arr.clone());
        }
    }
    None
}

fn literal_int(value: Option<&Value>) -> Option<i64> {
    value?.as_i64()
}

fn is_index_out_of_bounds(idx: i64, len: usize) -> bool {
    let len_i = len as i64;
    if idx > 0 {
        let pos = idx - 1;
        pos < 0 || pos >= len_i
    } else if idx < 0 {
        let pos = len_i + idx;
        pos < 0 || pos >= len_i
    } else {
        true
    }
}
