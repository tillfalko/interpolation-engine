use crate::filter::{InvertedFilter, OutputFilter};
use anyhow::{anyhow, Result};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::{json, Map, Value};

#[derive(Debug)]
pub struct ChatArgs {
    pub messages: Vec<Map<String, Value>>,
    pub completion_args: Map<String, Value>,
    pub start_str: String,
    pub stop_str: String,
    pub hide_start_str: String,
    pub hide_stop_str: String,
    pub n_outputs: i64,
    pub shown: bool,
    pub choices_list: Option<Vec<String>>,
    pub extra_body: Map<String, Value>,
    pub api_url: String,
    pub api_key: String,
}

pub async fn run_chat(
    args: ChatArgs,
    mut on_text: Option<&mut dyn FnMut(&str) -> Result<()>>,
) -> Result<(Vec<String>, String)> {
    if (!args.start_str.is_empty()) ^ (!args.stop_str.is_empty()) {
        return Err(anyhow!(
            "You can either set both start_str and stop_str or none."
        ));
    }
    if args.choices_list.is_some() {
        if !args.start_str.is_empty() {
            return Err(anyhow!("Filtering is not supported when using choices."));
        }
        if args.n_outputs != 1 {
            return Err(anyhow!("Multiple outputs not supported when using choices."));
        }
    }

    let mut request = args.completion_args.clone();
    request.insert("messages".to_string(), Value::Array(args.messages.iter().cloned().map(Value::Object).collect()));
    request.insert("stream".to_string(), Value::Bool(true));

    if !args.extra_body.is_empty() {
        request.insert("extra_body".to_string(), Value::Object(args.extra_body.clone()));
    }

    if request.contains_key("max_completion_tokens") {
        if let Some(v) = request.remove("max_completion_tokens") {
            request.insert("max_tokens".to_string(), v);
        }
    }

    if let Some(choices) = &args.choices_list {
        let schema = json!({
            "type": "object",
            "properties": { "choice": { "enum": choices } },
            "required": ["choice"],
            "additionalProperties": false
        });
        let prompt = format!(
            "Respond only with a valid JSON object conforming to this schema: {}. Do not add any additional text.",
            schema
        );
        let mut msgs = args.messages.clone();
        msgs.push(map_message("user", &prompt));
        request.insert(
            "messages".to_string(),
            Value::Array(msgs.into_iter().map(Value::Object).collect()),
        );
        request.insert(
            "response_format".to_string(),
            json!({"type":"json_schema","json_schema":schema}),
        );
    }

    let url = normalize_api_url(&args.api_url);
    let client = reqwest::Client::new();
    let res = client
        .post(url)
        .bearer_auth(&args.api_key)
        .json(&request)
        .send()
        .await?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(anyhow!("Chat request failed: {status} {body}"));
    }

    let mut output_filter = OutputFilter::new(&args.start_str, &args.stop_str, args.n_outputs > 1);
    let mut hide_filter = InvertedFilter::new(&args.hide_start_str, &args.hide_stop_str);
    let mut raw = String::new();
    let mut visual_output = String::new();
    let mut ran_out_of_context = false;

    let mut stream = res.bytes_stream().eventsource();
    while let Some(event) = stream.next().await {
        let event = event?;
        if event.data == "[DONE]" {
            break;
        }
        let chunk: Value = serde_json::from_str(&event.data)?;
        let delta = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("delta"))
            .and_then(|v| v.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let finish_reason = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("finish_reason"))
            .and_then(Value::as_str);
        if finish_reason == Some("length") {
            ran_out_of_context = true;
        }
        if !delta.is_empty() {
            raw.push_str(delta);
            let fragment = output_filter.update(delta);
            let visual_fragment = hide_filter.update(&fragment);
            if args.shown && !visual_fragment.is_empty() {
                if let Some(cb) = on_text.as_mut() {
                    cb(&visual_fragment)?;
                }
                visual_output.push_str(&visual_fragment);
            }
        }
    }

    if ran_out_of_context {
        return Err(anyhow!("Generation exceeded context length."));
    }

    if let Some(_) = args.choices_list {
        let parsed: Value = serde_json::from_str(&raw)?;
        let choice = parsed
            .get("choice")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Choice schema response missing 'choice'"))?;
        return Ok((vec![choice.to_string()], visual_output));
    }

    let outputs = output_filter.outputs().into_iter().map(|o| o.trim().to_string()).collect();
    Ok((outputs, visual_output))
}

fn normalize_api_url(api_url: &str) -> String {
    let base = api_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}

fn map_message(role: &str, content: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("role".to_string(), Value::String(role.to_string()));
    m.insert("content".to_string(), Value::String(content.to_string()));
    m
}
