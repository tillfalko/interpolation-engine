use crate::model::{Program, ProgramLoadContext, Task};
use anyhow::{anyhow, Result};
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs;

pub fn load_program(ctx: &mut ProgramLoadContext) -> Result<Program> {
    let raw = fs::read_to_string(&ctx.program_path)?;
    let with_lines = add_line_numbers(&raw)?;
    let mut root: Value = json5::from_str(&with_lines)?;

    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow!("Program root must be an object"))?;

    if !obj.contains_key("named_tasks") && obj.contains_key("tasks") {
        let tasks = obj.remove("tasks").unwrap();
        obj.insert("named_tasks".to_string(), tasks);
    }

    let default_state = obj
        .get("default_state")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("Program missing 'default_state' object"))?
        .clone();

    let order = obj
        .get("order")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Program missing 'order' array"))?
        .iter()
        .map(as_task)
        .collect::<Result<Vec<_>>>()?;

    let named_tasks = obj
        .get("named_tasks")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("Program missing 'named_tasks' object"))?
        .iter()
        .map(|(k, v)| Ok((k.clone(), as_task(v)?)))
        .collect::<Result<HashMap<_, _>>>()?;

    let save_states = obj
        .get("save_states")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("Program missing 'save_states' object"))?
        .clone();

    let completion_args = obj
        .get("completion_args")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    Ok(Program {
        default_state,
        order,
        named_tasks,
        save_states,
        completion_args,
    })
}

fn as_task(value: &Value) -> Result<Task> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow!("Task must be an object, got {value:?}"))
}

fn add_line_numbers(input: &str) -> Result<String> {
    let re = Regex::new(
        r#"(?P<key>\bcmd\b|"cmd"|'cmd')\s*:\s*(?P<val>"([^"\\]|\\.)*"|'([^'\\]|\\.)*')(?P<trail>\s*(?:,|\}))"#,
    )?;
    let mut out = String::new();
    for (i, line) in input.lines().enumerate() {
        let line_no = i + 1;
        let replaced = re.replace_all(line, |caps: &regex::Captures| {
            format!(
                "{}:{}{}, line:{}{}",
                &caps["key"],
                &caps["val"],
                "",
                line_no,
                &caps["trail"]
            )
        });
        out.push_str(&replaced);
        out.push('\n');
    }
    Ok(out)
}
