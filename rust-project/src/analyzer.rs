use crate::interp::extract_insert_keys;
use crate::model::{Program, ProgramLoadContext, Task};
use anyhow::{anyhow, Result};
use serde_json::Value;
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

    let insert_keys = collect_possible_insert_keys(program, ctx);
    let global_labels = collect_labels(program);

    let mut named = HashSet::new();
    for name in program.named_tasks.keys() {
        named.insert(name.clone());
    }

    analyze_task_list(
        &program.order,
        "order",
        &named,
        &insert_keys,
        &global_labels,
        ctx,
        &mut diags,
    );

    for (name, task) in &program.named_tasks {
        analyze_task_list(
            &[task.clone()],
            &format!("named_tasks.{name}"),
            &named,
            &insert_keys,
            &global_labels,
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

fn collect_possible_insert_keys(program: &Program, ctx: &ProgramLoadContext) -> HashSet<String> {
    let mut keys = HashSet::new();
    if let Some(inserts) = program.default_state.get("inserts").and_then(Value::as_object) {
        for k in inserts.keys() {
            keys.insert(k.clone());
        }
    }
    keys.insert("HH:MM".to_string());
    keys.insert("HH:MM:SS".to_string());
    keys.extend(ctx.inserts_dir_keys.iter().cloned());

    let mut stack = Vec::new();
    stack.extend(program.order.iter().cloned());
    stack.extend(program.named_tasks.values().cloned());

    while let Some(task) = stack.pop() {
        if let Some(output_name) = task.get("output_name").and_then(Value::as_str) {
            keys.insert(output_name.to_string());
        }
        if task.get("cmd").and_then(Value::as_str) == Some("for") {
            if let Some(map) = task.get("name_list_map").and_then(Value::as_object) {
                for k in map.keys() {
                    keys.insert(k.clone());
                }
            }
        }
        if let Some(tasks) = task.get("tasks") {
            if let Some(arr) = tasks.as_array() {
                for t in arr {
                    if let Ok(subtask) = super_task(t) {
                        stack.push(subtask);
                    }
                }
            }
        }
        if let Some(item) = task.get("item") {
            if let Ok(item_task) = super_task(item) {
                stack.push(item_task);
            }
        }
    }

    keys
}

fn analyze_task_list(
    tasks: &[Task],
    scope_name: &str,
    named_tasks: &HashSet<String>,
    insert_keys: &HashSet<String>,
    global_labels: &HashSet<String>,
    ctx: &ProgramLoadContext,
    diags: &mut Vec<Diagnostic>,
) {
    for task in tasks {
        validate_task(task, scope_name, named_tasks, insert_keys, global_labels, ctx, diags);
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
                    insert_keys,
                    global_labels,
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
    insert_keys: &HashSet<String>,
    labels: &HashSet<String>,
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
        "print" => require_fields(task, &["text"], diags),
        "clear" => {}
        "sleep" => require_fields(task, &["seconds"], diags),
        "set" => require_fields(task, &["item", "output_name"], diags),
        "unescape" => require_fields(task, &["item", "output_name"], diags),
        "write" => require_fields(task, &["item", "path"], diags),
        "show_inserts" => {}
        "random_choice" => require_fields(task, &["list", "output_name"], diags),
        "list_join" => require_fields(task, &["list", "before", "between", "after", "output_name"], diags),
        "list_concat" => require_fields(task, &["lists", "output_name"], diags),
        "list_append" => require_fields(task, &["list", "item", "output_name"], diags),
        "list_remove" => require_fields(task, &["list", "item", "output_name"], diags),
        "list_index" => require_fields(task, &["list", "index", "output_name"], diags),
        "list_slice" => require_fields(task, &["list", "from_index", "to_index", "output_name"], diags),
        "user_input" => require_fields(task, &["prompt", "output_name"], diags),
        "user_choice" => require_fields(task, &["list", "description", "output_name"], diags),
        "await_insert" => require_fields(task, &["name"], diags),
        "label" => require_fields(task, &["name"], diags),
        "goto" => {
            require_fields(task, &["name"], diags);
            if let Some(target) = task.get("name").and_then(Value::as_str) {
                if target != "CONTINUE" && !labels.contains(target) {
                    diags.push(diag(
                        task,
                        format!("goto target '{target}' not found in {scope_name}"),
                    ));
                }
            }
        }
        "goto_map" => {
            require_fields(task, &["text", "target_maps"], diags);
            if let Some(target_maps) = task.get("target_maps").and_then(Value::as_array) {
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
                    let (_, target_val) = obj.iter().next().unwrap();
                    if let Some(target) = target_val.as_str() {
                        if !target.contains('{') && target != "CONTINUE" && !labels.contains(target) {
                            diags.push(diag(
                                task,
                                format!("goto_map target '{target}' not found in {scope_name}"),
                            ));
                        }
                    }
                }
            }
        }
        "replace_map" => require_fields(task, &["item", "output_name", "wildcard_maps"], diags),
        "for" => require_fields(task, &["name_list_map", "tasks"], diags),
        "serial" | "parallel_wait" | "parallel_race" => require_fields(task, &["tasks"], diags),
        "run_task" => {
            require_fields(task, &["task_name"], diags);
            if let Some(name) = task.get("task_name").and_then(Value::as_str) {
                if !named_tasks.contains(name) {
                    diags.push(diag(task, format!("run_task references unknown task '{name}'")));
                }
            }
        }
        "delete" | "delete_except" => require_fields(task, &["wildcards"], diags),
        "math" => require_fields(task, &["input", "output_name"], diags),
        "chat" => {
            require_fields(task, &["messages", "output_name"], diags);
            validate_voice_path(task, ctx, diags);
        }
        "speak" => {
            require_fields(task, &["text", "voice_path"], diags);
            validate_voice_path(task, ctx, diags);
        }
        _ => diags.push(diag(task, format!("Unknown cmd '{cmd}'"))),
    }

    for (k, value) in task.iter() {
        if k == "tasks" {
            continue;
        }
        if value
            .as_array()
            .is_some_and(|arr| arr.iter().all(|v| v.as_object().is_some()))
        {
            continue;
        }
        if value
            .as_object()
            .is_some_and(|obj| obj.get("cmd").and_then(Value::as_str).is_some())
        {
            continue;
        }
        for key in extract_insert_keys(value) {
            let is_numeric_capture = cmd == "replace_map" && key.chars().all(|c| c.is_ascii_digit());
            if !is_possible_insert(&key, insert_keys) && !key.starts_with("ARG") && !is_numeric_capture {
                diags.push(diag(
                    task,
                    format!("Interpolation key '{key}' will never be defined"),
                ));
            }
        }
    }
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

fn is_possible_insert(key: &str, insert_keys: &HashSet<String>) -> bool {
    if insert_keys.contains(key) {
        return true;
    }
    if key.contains('*') {
        for k in insert_keys {
            if wildcard_match(key, k) || wildcard_match(k, key) {
                return true;
            }
        }
    }
    false
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

fn collect_labels(program: &Program) -> HashSet<String> {
    let mut labels = HashSet::new();
    let mut stack = Vec::new();
    stack.extend(program.order.iter().cloned());
    stack.extend(program.named_tasks.values().cloned());
    while let Some(task) = stack.pop() {
        if task.get("cmd").and_then(Value::as_str) == Some("label") {
            if let Some(name) = task.get("name").and_then(Value::as_str) {
                labels.insert(name.to_string());
            }
        }
        if let Some(tasks) = task.get("tasks").and_then(Value::as_array) {
            for t in tasks {
                if let Ok(subtask) = super_task(t) {
                    stack.push(subtask);
                }
            }
        }
    }
    labels
}
