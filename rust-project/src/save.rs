use anyhow::{anyhow, Result};
use serde_json::Value;

pub fn splice_key_into_json5(content: &str, key: &str, new_value: &Value, indent: usize) -> Result<String> {
    let pattern = format!(r#"(['"]?{key}['"]?)\s*:\s*\{{"#);
    let re = regex::Regex::new(&pattern)?;
    let mat = re
        .find(content)
        .ok_or_else(|| anyhow!("Key '{key}' not found or not an object"))?;

    let start_pos = mat.end() - 1;
    let mut brace_level = 1;
    let mut end_pos = None;
    for (i, ch) in content[start_pos + 1..].char_indices() {
        match ch {
            '{' => brace_level += 1,
            '}' => brace_level -= 1,
            _ => {}
        }
        if brace_level == 0 {
            end_pos = Some(start_pos + 1 + i);
            break;
        }
    }
    let end_pos = end_pos.ok_or_else(|| anyhow!("Could not find matching closing brace"))?;

    let line_start = content[..mat.start()].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let key_indent = &content[line_start..mat.start()];

    let dumped = serde_json::to_string_pretty(new_value)?;
    let inner_lines: Vec<&str> = dumped
        .lines()
        .skip(1)
        .take(dumped.lines().count().saturating_sub(2))
        .collect();
    let formatted_inner: Vec<String> = inner_lines
        .into_iter()
        .map(|line| format!("{key_indent}{line}"))
        .collect();
    let replacement = format!("\n{}\n{key_indent}", formatted_inner.join("\n"));

    let mut out = String::new();
    out.push_str(&content[..start_pos + 1]);
    out.push_str(&replacement);
    out.push_str(&content[end_pos..]);
    Ok(out)
}
