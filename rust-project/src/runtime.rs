use crate::chat::{run_chat, ChatArgs};
use async_recursion::async_recursion;
use crate::interp::{
    delete_interpdata, get_interpdata, get_simple_insertkey, interpolate_inserts, recursive_interpolate,
    recursive_unescape, set_interpdata, value_to_string, ESCAPE, INSERT_START, INSERT_STOP,
};
use crate::math::eval_math;
use crate::model::{Program, ProgramLoadContext, Task};
use crate::save::splice_key_into_json5;
use crate::audio_web;
use crate::ui::{start_ui, UiCommandHandle, UiEvent};
use anyhow::{anyhow, Result};
use rand::random;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use futures::stream::{FuturesUnordered, StreamExt};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct RuntimeOptions {
    pub agent_mode: bool,
    pub agent_input: PathBuf,
    pub agent_output: PathBuf,
    pub log_path: Option<PathBuf>,
    pub history_path: Option<PathBuf>,
    pub audio_web: bool,
    pub audio_port: u16,
}

#[derive(Clone)]
struct State {
    data: Map<String, Value>,
}

impl State {
    fn from_default(default_state: &Map<String, Value>) -> Self {
        let mut data = default_state.clone();
        if !data.contains_key("output") {
            data.insert("output".to_string(), Value::String(String::new()));
        }
        Self { data }
    }

    fn inserts(&self) -> &Map<String, Value> {
        self.data
            .get("inserts")
            .and_then(Value::as_object)
            .expect("state.inserts must be an object")
    }

    fn inserts_mut(&mut self) -> &mut Map<String, Value> {
        self.data
            .get_mut("inserts")
            .and_then(Value::as_object_mut)
            .expect("state.inserts must be an object")
    }

    fn get_output(&self) -> String {
        self.data
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    fn set_output(&mut self, text: String) {
        self.data.insert("output".to_string(), Value::String(text));
    }

    fn get_i64(&self, key: &str) -> i64 {
        self.data
            .get(key)
            .and_then(Value::as_i64)
            .unwrap_or(1)
    }

    fn set_i64(&mut self, key: &str, value: i64) {
        self.data.insert(key.to_string(), Value::Number(value.into()));
    }
}

pub async fn run_program(
    program: &mut Program,
    ctx: &ProgramLoadContext,
    args: &[String],
    options: RuntimeOptions,
) -> Result<()> {
    audio_web::init_config(audio_web::AudioWebConfig {
        enabled: options.audio_web,
        port: options.audio_port,
    });
    let state = Arc::new(Mutex::new(State::from_default(&program.default_state)));

    {
        let mut st = state.lock().await;
        let inserts = st.inserts_mut();
        for (i, arg) in args.iter().enumerate() {
            let key = format!("ARG{}", i + 1);
            let escaped = arg
                .replace(INSERT_START, &format!("{ESCAPE}{INSERT_START}"))
                .replace(INSERT_STOP, &format!("{ESCAPE}{INSERT_STOP}"));
            inserts.insert(key, Value::String(escaped));
        }
    }

    let mut completion_args = program.completion_args.clone();
    let named_tasks = program.named_tasks.clone();
    let ctx = Arc::new(ctx.clone());

        let (ui_cmd, mut ui_events, ui_join) = if options.agent_mode {
        (None, None, None)
    } else {
        let (cmd, events, join) = start_ui(options.history_path.clone());
        (Some(cmd), Some(events), Some(join))
    };

    let io = if options.agent_mode {
        Io::Agent(Arc::new(Mutex::new(AgentIo::new(
            options.agent_input.clone(),
            options.agent_output.clone(),
        ))))
    } else {
        Io::Ui(ui_cmd.clone().unwrap())
    };

    let run_result = async {
        if !program.order.is_empty() {
            io.set_output(state.lock().await.get_output()).await;
        }

        let mut menu_open = false;
        let mut kill = false;

        while {
            let st = state.lock().await;
            st.get_i64("order_index") <= program.order.len() as i64
        } {
            if kill {
                break;
            }

            if menu_open {
                if let (Io::Ui(ui), Some(_events)) = (&io, &mut ui_events) {
                    let action = main_menu(
                        program,
                        &state,
                        &mut completion_args,
                        &named_tasks,
                        &options,
                        ui,
                        &ctx,
                    )
                    .await?;
                    match action {
                        MenuAction::Close => menu_open = false,
                        MenuAction::Quit => {
                            kill = true;
                            break;
                        }
                    }
                    continue;
                } else {
                    menu_open = false;
                }
            }

            let task_index = state.lock().await.get_i64("order_index") - 1;
            let task = program.order.get(task_index as usize).cloned().unwrap();
            io.clear().await;
            io.write(state.lock().await.get_output()).await;

            let token = CancellationToken::new();
            let completion_snapshot = Arc::new(completion_args.clone());
            let named_snapshot = Arc::new(named_tasks.clone());
            let exec_fut = execute_task(
                state.clone(),
                task,
                completion_snapshot,
                named_snapshot,
                ctx.clone(),
                io.clone(),
                token.child_token(),
                "root".to_string(),
            );
            let mut exec_fut = Box::pin(exec_fut);

            if let (Io::Ui(ui), Some(events)) = (&io, &mut ui_events) {
                loop {
                    tokio::select! {
                        res = &mut exec_fut => {
                            match res {
                                Ok(TaskOutcome::None) => {
                                    state.lock().await.set_i64("order_index", task_index as i64 + 2);
                                    break;
                                }
                                Ok(TaskOutcome::Goto(target)) => {
                                    let idx = find_label_index(&program.order, &target)?;
                                    state.lock().await.set_i64("order_index", (idx + 2) as i64);
                                    break;
                                }
                                Err(e) => {
                                    if is_cancelled(&e) || token.is_cancelled() {
                                        let mut saw_event = false;
                                        while let Ok(ev) = events.try_recv() {
                                            match ev {
                                                UiEvent::ToggleMenu => {
                                                    menu_open = true;
                                                    saw_event = true;
                                                }
                                                UiEvent::Quit => {
                                                    kill = true;
                                                    saw_event = true;
                                                }
                                            }
                                        }
                                        if !saw_event {
                                            menu_open = true;
                                        }
                                        break;
                                    }
                                    return Err(e);
                                }
                            }
                        }
                        ev = events.recv() => {
                            match ev {
                                Some(UiEvent::ToggleMenu) => {
                                    token.cancel();
                                    ui.cancel_input();
                                    menu_open = true;
                                    break;
                                }
                                Some(UiEvent::Quit) => {
                                    token.cancel();
                                    ui.cancel_input();
                                    kill = true;
                                    break;
                                }
                                None => {}
                            }
                        }
                    }
                    if menu_open || kill {
                        break;
                    }
                }
            } else {
                let outcome = exec_fut.await?;
                match outcome {
                    TaskOutcome::None => {
                        state.lock().await.set_i64("order_index", task_index as i64 + 2);
                    }
                    TaskOutcome::Goto(target) => {
                        let idx = find_label_index(&program.order, &target)?;
                        state.lock().await.set_i64("order_index", (idx + 2) as i64);
                    }
                }
            }
        }

        Ok::<(), anyhow::Error>(())
    }
    .await;

    if options.audio_web {
        audio_web::wait_for_idle(
            Duration::from_millis(300),
            Duration::from_secs(10),
            Duration::from_millis(1200),
        )
        .await;
    }

    if let (Io::Ui(ui), Some(join)) = (&io, ui_join) {
        ui.shutdown();
        let _ = join.join();
    }

    let output = state.lock().await.get_output();
    println!("{}", output.trim());
    run_result
}

#[derive(Debug)]
enum TaskOutcome {
    None,
    Goto(String),
}

fn task_label(task: &Task, fallback_index: usize) -> String {
    let cmd = task
        .get("cmd")
        .and_then(Value::as_str)
        .unwrap_or("task");
    match task.get("line").and_then(Value::as_i64) {
        Some(line) => format!("{cmd}:{line}"),
        None => format!("{cmd}:{fallback_index}"),
    }
}

#[async_recursion(?Send)]
async fn execute_task(
    state: Arc<Mutex<State>>,
    task: Task,
    completion_args: Arc<Map<String, Value>>,
    named_tasks: Arc<HashMap<String, Task>>,
    ctx: Arc<ProgramLoadContext>,
    io: Io,
    token: CancellationToken,
    runtime_label: String,
) -> Result<TaskOutcome> {
    if token.is_cancelled() {
        return Err(anyhow!("cancelled"));
    }

    let inserts_snapshot = state.lock().await.inserts().clone();
    let interpolated = recursive_interpolate(&inserts_snapshot, Value::Object(task), &ctx)?;
    let task = interpolated
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow!("Task must be object after interpolation"))?;
    let cmd = task
        .get("cmd")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Task missing cmd"))?;

    match cmd {
        "list_join" => {
            let list = as_array(&task, "list")?;
            let before = as_string(&task, "before")?;
            let between = as_string(&task, "between")?;
            let after = as_string(&task, "after")?;
            let output_name = as_string(&task, "output_name")?;
            let joined = format!(
                "{}{}{}",
                before,
                list.iter().map(value_to_string).collect::<Vec<_>>().join(&between),
                after
            );
            with_inserts(state, |ins| set_interpdata(ins, &output_name, Value::String(joined))).await;
        }
        "list_concat" => {
            let lists = as_array(&task, "lists")?;
            let output_name = as_string(&task, "output_name")?;
            let mut out = Vec::new();
            for list in lists {
                if let Some(arr) = list.as_array() {
                    out.extend(arr.clone());
                } else {
                    return Err(anyhow!("list_concat expects lists of arrays"));
                }
            }
            with_inserts(state, |ins| set_interpdata(ins, &output_name, Value::Array(out))).await;
        }
        "list_append" => {
            let list = as_array(&task, "list")?;
            let item = task.get("item").cloned().unwrap_or(Value::Null);
            let output_name = as_string(&task, "output_name")?;
            let mut new_list = list.clone();
            new_list.push(item);
            with_inserts(state, |ins| set_interpdata(ins, &output_name, Value::Array(new_list))).await;
        }
        "list_remove" => {
            let list = as_array(&task, "list")?;
            let item = task.get("item").cloned().unwrap_or(Value::Null);
            let output_name = as_string(&task, "output_name")?;
            let mut new_list = list.clone();
            if let Some(pos) = new_list.iter().position(|v| *v == item) {
                new_list.remove(pos);
            }
            with_inserts(state, |ins| set_interpdata(ins, &output_name, Value::Array(new_list))).await;
        }
        "list_index" => {
            let list = as_array(&task, "list")?;
            let index_val = task.get("index").cloned().unwrap_or(Value::Null);
            let index = eval_index(&index_val, &inserts_snapshot, &ctx, list.len())?;
            let output_name = as_string(&task, "output_name")?;
            let item = list
                .get(index)
                .ok_or_else(|| anyhow!("Index out of bounds"))?
                .clone();
            with_inserts(state, |ins| set_interpdata(ins, &output_name, item)).await;
        }
        "list_slice" => {
            let list = as_array(&task, "list")?;
            let from_val = task.get("from_index").cloned().unwrap_or(Value::Null);
            let to_val = task.get("to_index").cloned().unwrap_or(Value::Null);
            let from = eval_math_index(&from_val, &inserts_snapshot, &ctx)?;
            let to = eval_math_index(&to_val, &inserts_snapshot, &ctx)?;
            if to == 0 {
                let output_name = as_string(&task, "output_name")?;
                with_inserts(state, |ins| set_interpdata(ins, &output_name, Value::Array(Vec::new()))).await;
                return Ok(TaskOutcome::None);
            }
            let (start, end) = slice_indices(from, to, list.len())?;
            if end < start {
                let output_name = as_string(&task, "output_name")?;
                with_inserts(state, |ins| set_interpdata(ins, &output_name, Value::Array(Vec::new()))).await;
                return Ok(TaskOutcome::None);
            }
            let slice = list[start..=end].to_vec();
            let output_name = as_string(&task, "output_name")?;
            with_inserts(state, |ins| set_interpdata(ins, &output_name, Value::Array(slice))).await;
        }
        "user_choice" => {
            let list = as_array(&task, "list")?;
            let description = as_string(&task, "description")?;
            let output_name = as_string(&task, "output_name")?;
            if list.is_empty() {
                let _ = io.select_index(Vec::new(), Some(description), true).await?;
                with_inserts(state, |ins| set_interpdata(ins, &output_name, Value::Null)).await;
            } else {
                let options = list.iter().map(value_to_string).collect::<Vec<_>>();
                let choice_index = io.select_index(options, Some(description), true).await?;
                let choice = list
                    .get(choice_index)
                    .ok_or_else(|| anyhow!("Choice index out of bounds"))?
                    .clone();
                with_inserts(state, |ins| set_interpdata(ins, &output_name, choice)).await;
            }
        }
        "user_input" => {
            let prompt = as_string(&task, "prompt")?;
            let output_name = as_string(&task, "output_name")?;
            let input = io.user_input(prompt, String::new(), true).await?;
            let escaped = input
                .replace(INSERT_START, &format!("{ESCAPE}{INSERT_START}"))
                .replace(INSERT_STOP, &format!("{ESCAPE}{INSERT_STOP}"));
            with_inserts(state, |ins| {
                set_interpdata(ins, &output_name, Value::String(escaped))
            })
            .await;
        }
        "await_insert" => {
            let name = as_string(&task, "name")?;
            loop {
                if token.is_cancelled() {
                    return Err(anyhow!("cancelled"));
                }
                if state.lock().await.inserts().contains_key(&name) {
                    break;
                }
                sleep(Duration::from_millis(50)).await;
            }
        }
        "run_task" => {
            let name = as_string(&task, "task_name")?;
            let subtask = named_tasks
                .get(&name)
                .cloned()
                .ok_or_else(|| anyhow!("Unknown task '{name}'"))?;
            return execute_task(
                state,
                subtask,
                completion_args.clone(),
                named_tasks.clone(),
                ctx.clone(),
                io.clone(),
                token,
                format!("{runtime_label}/{name}"),
            )
            .await;
        }
        "parallel_wait" => {
            let tasks = as_task_array(&task, "tasks")?;
            let futures = tasks.into_iter().enumerate().map(|(index, t)| {
                let child_label = format!("{}/{}", runtime_label, task_label(&t, index + 1));
                execute_task(
                    state.clone(),
                    t,
                    completion_args.clone(),
                    named_tasks.clone(),
                    ctx.clone(),
                    io.clone(),
                    token.child_token(),
                    child_label,
                )
            });
            let results = futures::future::join_all(futures).await;
            for res in results {
                res?;
            }
        }
        "parallel_race" => {
            let tasks = as_task_array(&task, "tasks")?;
            let group = token.child_token();
            let mut futures = FuturesUnordered::new();
            for (index, t) in tasks.into_iter().enumerate() {
                let child_label = format!("{}/{}", runtime_label, task_label(&t, index + 1));
                futures.push(execute_task(
                    state.clone(),
                    t,
                    completion_args.clone(),
                    named_tasks.clone(),
                    ctx.clone(),
                    io.clone(),
                    group.child_token(),
                    child_label,
                ));
            }
            if let Some(res) = futures.next().await {
                group.cancel();
                res?;
            }
            while let Some(res) = futures.next().await {
                let _ = res;
            }
        }
        "serial" => {
            let tasks = as_task_array(&task, "tasks")?;
            let sub_index_label = format!("order_index/{runtime_label}");
            let mut sub_index = state.lock().await.get_i64(&sub_index_label);
            while sub_index <= tasks.len() as i64 {
                if token.is_cancelled() {
                    return Err(anyhow!("cancelled"));
                }
                let subtask = tasks.get((sub_index - 1) as usize).cloned().unwrap();
                let child_label =
                    format!("{}/{}", runtime_label, task_label(&subtask, sub_index as usize));
                let result = execute_task(
                    state.clone(),
                    subtask,
                    completion_args.clone(),
                    named_tasks.clone(),
                    ctx.clone(),
                    io.clone(),
                    token.child_token(),
                    child_label,
                )
                .await?;
                match result {
                    TaskOutcome::None => sub_index += 1,
                    TaskOutcome::Goto(target) => {
                        let idx = find_label_index(&tasks, &target)?;
                        sub_index = idx as i64 + 2;
                    }
                }
                state.lock().await.set_i64(&sub_index_label, sub_index);
            }
            state.lock().await.data.remove(&sub_index_label);
        }
        "for" => {
            let name_list_map = task
                .get("name_list_map")
                .and_then(Value::as_object)
                .ok_or_else(|| anyhow!("for.name_list_map must be object"))?
                .clone();
            let tasks = as_task_array(&task, "tasks")?;
            let mut lists = Vec::new();
            let mut item_names = Vec::new();
            for (name, list_val) in name_list_map {
                let list_value = recursive_interpolate(&inserts_snapshot, list_val, &ctx)?;
                let list = list_value
                    .as_array()
                    .ok_or_else(|| anyhow!("for expects list values"))?
                    .clone();
                lists.push(list);
                item_names.push(name);
            }
            let len = lists.first().map(|l| l.len()).unwrap_or(0);
            if lists.iter().any(|l| l.len() != len) {
                return Err(anyhow!("Lists have differing lengths"));
            }
            let counter_label = format!("order_index/{runtime_label}/counter");
            let mut counter = state.lock().await.get_i64(&counter_label);
            while counter <= len as i64 {
                if token.is_cancelled() {
                    return Err(anyhow!("cancelled"));
                }
                for (name, list) in item_names.iter().zip(lists.iter()) {
                    let value = list[(counter - 1) as usize].clone();
                    with_inserts(state.clone(), |ins| set_interpdata(ins, name, value)).await;
                }
                let sub_index_label = format!("order_index/{runtime_label}");
                let mut sub_index = state.lock().await.get_i64(&sub_index_label);
                while sub_index <= tasks.len() as i64 {
                    let subtask = tasks.get((sub_index - 1) as usize).cloned().unwrap();
                    let child_label = format!(
                        "{}/{}",
                        runtime_label,
                        task_label(&subtask, sub_index as usize)
                    );
                    let result = execute_task(
                        state.clone(),
                        subtask,
                        completion_args.clone(),
                        named_tasks.clone(),
                        ctx.clone(),
                        io.clone(),
                        token.child_token(),
                        child_label,
                    )
                    .await?;
                    match result {
                        TaskOutcome::None => sub_index += 1,
                        TaskOutcome::Goto(target) => {
                            let idx = find_label_index(&tasks, &target)?;
                            sub_index = idx as i64 + 2;
                        }
                    }
                    state.lock().await.set_i64(&sub_index_label, sub_index);
                }
                counter += 1;
                state.lock().await.data.remove(&sub_index_label);
                state.lock().await.set_i64(&counter_label, counter);
            }
            state.lock().await.data.remove(&counter_label);
        }
        "label" => {}
        "set" => {
            let item = task.get("item").cloned().unwrap_or(Value::Null);
            let output_name = as_string(&task, "output_name")?;
            with_inserts(state, |ins| set_interpdata(ins, &output_name, item)).await;
        }
        "unescape" => {
            let item = task.get("item").cloned().unwrap_or(Value::Null);
            let output_name = as_string(&task, "output_name")?;
            let unescaped = recursive_unescape(item);
            let interpolated = recursive_interpolate(&inserts_snapshot, unescaped, &ctx)?;
            with_inserts(state, |ins| set_interpdata(ins, &output_name, interpolated)).await;
        }
        "print" => {
            let text = as_string(&task, "text")?;
            let text = text
                .replace(&format!("{ESCAPE}{INSERT_START}"), &INSERT_START.to_string())
                .replace(&format!("{ESCAPE}{INSERT_STOP}"), &INSERT_STOP.to_string());
            let mut st = state.lock().await;
            let mut output = st.get_output();
            output.push_str(&text);
            st.set_output(output.clone());
            io.write(output_tail(&text)).await;
        }
        "sleep" => {
            let seconds_val = task.get("seconds").cloned().unwrap_or(Value::Null);
            let seconds = if seconds_val.is_string() {
                eval_math(&inserts_snapshot, seconds_val.as_str().unwrap(), &ctx)? as f64
            } else {
                seconds_val.as_f64().unwrap_or(0.0)
            };
            tokio::select! {
                _ = sleep(Duration::from_millis((seconds * 1000.0) as u64)) => {}
                _ = token.cancelled() => return Err(anyhow!("cancelled")),
            }
        }
        "clear" => {
            state.lock().await.set_output(String::new());
            io.clear().await;
        }
        "goto" => {
            let target = as_string(&task, "name")?;
            if target != "CONTINUE" {
                return Ok(TaskOutcome::Goto(target));
            }
        }
        "goto_map" => {
            let value_text = as_string(&task, "text")?;
            let target_maps = task
                .get("target_maps")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("goto_map.target_maps must be array"))?;

            let mut interp_error = false;
            let value_text = match interpolate_inserts(&inserts_snapshot, &value_text, &ctx) {
                Ok(v) => value_to_string(&v),
                Err(_) => {
                    interp_error = true;
                    "NULL".to_string()
                }
            };

            let mut target = None;
            if interp_error {
                for entry in target_maps {
                    let obj = entry.as_object().ok_or_else(|| anyhow!("target_maps entry must be object"))?;
                    let (k, v) = obj.iter().next().ok_or_else(|| anyhow!("target_maps entry empty"))?;
                    let key = value_to_string(&interpolate_inserts(&inserts_snapshot, k, &ctx)?);
                    if key == "NULL" {
                        target = Some(value_to_string(&interpolate_inserts(
                            &inserts_snapshot,
                            v.as_str().unwrap_or(""),
                            &ctx,
                        )?));
                        break;
                    }
                }
                if target.is_none() {
                    return Err(anyhow!(
                        "goto_map value could not be resolved but 'NULL' is not a key in target_maps"
                    ));
                }
            } else {
                for entry in target_maps {
                    let obj = entry.as_object().ok_or_else(|| anyhow!("target_maps entry must be object"))?;
                    let (k, v) = obj.iter().next().ok_or_else(|| anyhow!("target_maps entry empty"))?;
                    let key = value_to_string(&interpolate_inserts(&inserts_snapshot, k, &ctx)?);
                    let val = value_to_string(&interpolate_inserts(&inserts_snapshot, v.as_str().unwrap_or(""), &ctx)?);
                    if wildcard_match(&key, &value_text) {
                        target = Some(val);
                        break;
                    }
                }
            }
            let target = target.ok_or_else(|| anyhow!("goto_map has no matches for '{value_text}'"))?;
            if target != "CONTINUE" {
                return Ok(TaskOutcome::Goto(target));
            }
        }
        "replace_map" => {
            let item = task.get("item").cloned().unwrap_or(Value::Null);
            let output_name = as_string(&task, "output_name")?;
            let maps = task
                .get("wildcard_maps")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("replace_map.wildcard_maps must be array"))?
                .clone();
            let repeat_until_done = task
                .get("repeat_until_done")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let result = replace_map(item, &maps, &inserts_snapshot, &ctx, repeat_until_done)?;
            with_inserts(state, |ins| set_interpdata(ins, &output_name, result)).await;
        }
        "show_inserts" => {
            let inserts = state.lock().await.inserts().clone();
            let text = serde_json::to_string_pretty(&Value::Object(inserts))?;
            let _ = io.select_index(vec!["Dismiss".to_string()], Some(text), true).await?;
        }
        "random_choice" => {
            let list = as_array(&task, "list")?;
            let output_name = as_string(&task, "output_name")?;
            if list.is_empty() {
                return Err(anyhow!("random_choice list is empty"));
            }
            let idx = random::<usize>() % list.len();
            let item = list.get(idx).cloned().unwrap_or(Value::Null);
            with_inserts(state, |ins| set_interpdata(ins, &output_name, item)).await;
        }
        "delete" => {
            let wildcards = as_array(&task, "wildcards")?;
            with_inserts(state, |ins| {
                let keys: Vec<String> = ins.keys().cloned().collect();
                for k in keys {
                    if wildcards.iter().any(|w| wildcard_match(&value_to_string(w), &k)) {
                        delete_interpdata(ins, &k);
                    }
                }
            })
            .await;
        }
        "delete_except" => {
            let wildcards = as_array(&task, "wildcards")?;
            with_inserts(state, |ins| {
                let keys: Vec<String> = ins.keys().cloned().collect();
                for k in keys {
                    if !wildcards.iter().any(|w| wildcard_match(&value_to_string(w), &k)) {
                        delete_interpdata(ins, &k);
                    }
                }
            })
            .await;
        }
        "math" => {
            let input = as_string(&task, "input")?;
            let output_name = as_string(&task, "output_name")?;
            let result = eval_math(&inserts_snapshot, &input, &ctx)?;
            with_inserts(state, |ins| {
                set_interpdata(ins, &output_name, Value::Number(result.into()))
            })
            .await;
        }
        "write" => {
            let item = task.get("item").cloned().unwrap_or(Value::Null);
            let path = as_string(&task, "path")?;
            let resolved = resolve_path(&ctx, &path);
            let parent = resolved.parent().unwrap_or_else(|| std::path::Path::new("."));
            if !parent.is_dir() {
                return Err(anyhow!("write path '{}' does not exist", resolved.display()));
            }
            if resolved.is_dir() {
                return Err(anyhow!("write path '{}' is a directory", resolved.display()));
            }
            let content = match recursive_unescape(item) {
                Value::String(s) => s,
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                v => serde_json::to_string(&v)?,
            };
            fs::write(&resolved, content)?;
        }
        "speak" => {
            let text = as_string(&task, "text")?;
            let voice_path = as_string(&task, "voice_path")?;
            let voice_path = resolve_path(&ctx, &voice_path);
            let voice_path_str = voice_path.to_string_lossy().to_string();
            if text.is_empty() {
                io.stop_tts().await?;
            } else {
                io.speak(&text, &voice_path_str, task.get("voice_speaker").and_then(Value::as_i64)).await?;
            }
        }
        "chat" => {
            let messages = task.get("messages").cloned().unwrap_or(Value::Null);
            let output_name = as_string(&task, "output_name")?;

            let mut completion = (*completion_args).clone();
            if let Some(extra) = task.get("extra_body").and_then(Value::as_object) {
                let mut combined = completion
                    .get("extra_body")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                for (k, v) in extra {
                    combined.insert(k.clone(), v.clone());
                }
                completion.insert("extra_body".to_string(), Value::Object(combined));
            }
            for (k, v) in task.iter() {
                if k == "cmd" || k == "messages" || k == "output_name" {
                    continue;
                }
                completion.insert(k.clone(), v.clone());
            }

            let start_str = completion
                .remove("start_str")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let stop_str = completion
                .remove("stop_str")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let hide_start_str = completion
                .remove("hide_start_str")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let hide_stop_str = completion
                .remove("hide_stop_str")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let n_outputs = match completion.remove("n_outputs") {
                Some(Value::Number(n)) => n.as_i64().unwrap_or(1),
                Some(Value::String(s)) => s.parse::<i64>().unwrap_or(1),
                _ => 1,
            };
            let shown = match completion.remove("shown") {
                Some(Value::Bool(b)) => b,
                Some(Value::String(s)) if s == "true" => true,
                Some(Value::String(s)) if s == "false" => false,
                _ => true,
            };
            let choices_list = completion
                .remove("choices_list")
                .and_then(|v| v.as_array().cloned())
                .map(|arr| arr.iter().map(value_to_string).collect::<Vec<_>>());
            let voice_path = completion
                .remove("voice_path")
                .and_then(|v| v.as_str().map(|s| s.to_string()));
            let voice_speaker = completion
                .remove("voice_speaker")
                .and_then(|v| v.as_i64());
            let api_url = completion
                .remove("api_url")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "http://localhost:8080".to_string());
            let api_key = completion
                .remove("api_key")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "unused".to_string());
            let extra_body = completion
                .remove("extra_body")
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();

            let messages = interpolate_messages(messages, &inserts_snapshot, &ctx)?;

            completion.remove("line");
            completion.remove("traceback_label");

            let tts_writer = if let Some(path) = voice_path.clone() {
                let resolved = resolve_path(&ctx, &path);
                if !resolved.exists() {
                    return Err(anyhow!("voice_path does not exist: {}", resolved.display()));
                }
                if resolved.is_dir() {
                    return Err(anyhow!("voice_path is a directory, expected a file: {}", resolved.display()));
                }
                Some(Arc::new(std::sync::Mutex::new(
                    io.start_tts_stream(&resolved.to_string_lossy(), voice_speaker).await?,
                )))
            } else {
                None
            };
            let io_clone = io.clone();
            let tts_clone = tts_writer.clone();
            let mut on_text = move |text: &str| -> Result<()> {
                let io2 = io_clone.clone();
                let text_owned = text.to_string();
                tokio::spawn(async move {
                    io2.write(text_owned).await;
                });
                if let Some(writer) = tts_clone.as_ref() {
                    let mut guard = writer.lock().map_err(|_| anyhow!("TTS writer lock poisoned"))?;
                    guard.write(text)?;
                }
                Ok(())
            };

            let (outputs, visual_output) = loop {
                let (outputs, visual_output) = run_chat(
                    ChatArgs {
                        messages: messages.clone(),
                        completion_args: completion.clone(),
                        start_str: start_str.clone(),
                        stop_str: stop_str.clone(),
                        hide_start_str: hide_start_str.clone(),
                        hide_stop_str: hide_stop_str.clone(),
                        n_outputs,
                        shown,
                        choices_list: choices_list.clone(),
                        extra_body: extra_body.clone(),
                        api_url: api_url.clone(),
                        api_key: api_key.clone(),
                    },
                    Some(&mut on_text),
                )
                .await?;
                if outputs.len() < n_outputs as usize {
                    io.write(format!(
                        "\n(Expected {n_outputs} outputs, got {}. Retrying.)\n",
                        outputs.len()
                    ))
                    .await;
                    sleep(Duration::from_secs(2)).await;
                    continue;
                }
                break (outputs, visual_output);
            };

            if let Some(writer) = tts_writer.as_ref() {
                let mut guard = writer.lock().map_err(|_| anyhow!("TTS writer lock poisoned"))?;
                guard.finish()?;
            }

            if outputs.len() == 1 {
                with_inserts(state.clone(), |ins| {
                    set_interpdata(ins, &output_name, Value::String(outputs[0].clone()))
                })
                .await;
            } else {
                with_inserts(state.clone(), |ins| {
                    set_interpdata(ins, &output_name, Value::Array(outputs.into_iter().map(Value::String).collect()))
                })
                .await;
            }

            if !visual_output.is_empty() {
                let mut st = state.lock().await;
                let mut out = st.get_output();
                out.push_str(&visual_output);
                st.set_output(out);
            }
        }
        _ => return Err(anyhow!("Unknown cmd '{cmd}'")),
    }

    Ok(TaskOutcome::None)
}

async fn with_inserts<F>(state: Arc<Mutex<State>>, f: F)
where
    F: FnOnce(&mut Map<String, Value>),
{
    let mut st = state.lock().await;
    let inserts = st.inserts_mut();
    f(inserts);
}

fn as_string(task: &Task, key: &str) -> Result<String> {
    task.get(key)
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("Field '{key}' must be a string"))
}

fn as_array(task: &Task, key: &str) -> Result<Vec<Value>> {
    task.get(key)
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| anyhow!("Field '{key}' must be an array"))
}

fn as_task_array(task: &Task, key: &str) -> Result<Vec<Task>> {
    let arr = task
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Field '{key}' must be an array"))?;
    let mut out = Vec::new();
    for v in arr {
        if let Some(obj) = v.as_object() {
            out.push(obj.clone());
        } else {
            return Err(anyhow!("Tasks must be objects"));
        }
    }
    Ok(out)
}

fn eval_index(value: &Value, inserts: &Map<String, Value>, ctx: &ProgramLoadContext, len: usize) -> Result<usize> {
    let idx = if let Some(s) = value.as_str() {
        eval_math(inserts, s, ctx)? as i64
    } else {
        value.as_i64().ok_or_else(|| anyhow!("Index must be int"))?
    };
    if idx > 0 {
        let pos = idx - 1;
        if pos < 0 || pos >= len as i64 {
            return Err(anyhow!("Index out of bounds"));
        }
        Ok(pos as usize)
    } else if idx < 0 {
        let pos = len as i64 + idx;
        if pos < 0 || pos >= len as i64 {
            return Err(anyhow!("Index out of bounds"));
        }
        Ok(pos as usize)
    } else {
        Err(anyhow!("Index 0 is invalid (1-based indexing)"))
    }
}

fn eval_math_index(value: &Value, inserts: &Map<String, Value>, ctx: &ProgramLoadContext) -> Result<i64> {
    if let Some(s) = value.as_str() {
        Ok(eval_math(inserts, s, ctx)?)
    } else {
        value.as_i64().ok_or_else(|| anyhow!("Index must be int"))
    }
}

fn slice_indices(from: i64, to: i64, len: usize) -> Result<(usize, usize)> {
    let len_i = len as i64;
    let mut start = if from > 0 { from - 1 } else { len_i + from };
    let mut end = if to > 0 { to - 1 } else { len_i + to };
    if from == 0 {
        return Err(anyhow!("Lower slice index cannot be 0 (1-based)"));
    }
    if start < 0 || end < 0 || start >= len_i || end >= len_i {
        return Err(anyhow!("Slice indices out of bounds"));
    }
    Ok((start as usize, end as usize))
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

fn replace_map(
    item: Value,
    maps: &[Value],
    inserts: &Map<String, Value>,
    ctx: &ProgramLoadContext,
    repeat_until_done: bool,
) -> Result<Value> {
    let null_value = maps.iter().find_map(|m| {
        m.as_object().and_then(|o| {
            o.get("NULL").map(|v| v.clone())
        })
    });

    fn replace_str(
        mut text: String,
        maps: &[Value],
        inserts: &Map<String, Value>,
        ctx: &ProgramLoadContext,
        repeat_until_done: bool,
    ) -> Result<String> {
        loop {
            let mut current = match interpolate_inserts(inserts, &text, ctx) {
                Ok(v) => value_to_string(&v),
                Err(e) => return Err(e),
            };
            let mut replaced = None;
            for map in maps {
                let obj = map.as_object().ok_or_else(|| anyhow!("replace_map expects object"))?;
                let (k, v) = obj.iter().next().ok_or_else(|| anyhow!("replace_map entry empty"))?;
                let key = value_to_string(&interpolate_inserts(inserts, k, ctx)?);
                if wildcard_match(&key, &current) {
                    let captures = wildcard_captures(&key, &current);
                    let mut extra = inserts.clone();
                    for (i, cap) in captures.iter().enumerate() {
                        extra.insert((i + 1).to_string(), Value::String(cap.clone()));
                    }
                    let val = value_to_string(&interpolate_inserts(&extra, v.as_str().unwrap_or(""), ctx)?);
                    replaced = Some(val);
                    break;
                }
            }
            let new_text = replaced.unwrap_or(current.clone());
            if !repeat_until_done || new_text == text {
                return Ok(new_text);
            }
            text = new_text;
        }
    }

    let result: Result<Value, anyhow::Error> = match item {
        Value::String(s) => Ok(Value::String(replace_str(s, maps, inserts, ctx, repeat_until_done)?)),
        Value::Array(arr) => Ok(Value::Array(
            arr.into_iter()
                .map(|v| replace_map(v, maps, inserts, ctx, repeat_until_done))
                .collect::<Result<Vec<_>>>()?,
        )),
        Value::Object(obj) => {
            let mut out = Map::new();
            for (k, v) in obj {
                let new_k = replace_str(k, maps, inserts, ctx, repeat_until_done)?;
                let new_v = replace_map(v, maps, inserts, ctx, repeat_until_done)?;
                out.insert(new_k, new_v);
            }
            Ok(Value::Object(out))
        }
        v => Ok(v),
    };

    match result {
        Ok(v) => Ok(v),
        Err(_) => {
            if let Some(v) = null_value {
                Ok(v)
            } else {
                Err(anyhow!("replace_map interpolation error without NULL handler"))
            }
        }
    }
}

fn wildcard_captures(pattern: &str, text: &str) -> Vec<String> {
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str("(.*)"),
            _ => regex.push_str(&regex::escape(&ch.to_string())),
        }
    }
    regex.push('$');
    let re = regex::RegexBuilder::new(&regex)
        .dot_matches_new_line(true)
        .build()
        .unwrap();
    if let Some(caps) = re.captures(text) {
        caps.iter()
            .skip(1)
            .filter_map(|c| c.map(|m| m.as_str().to_string()))
            .collect()
    } else {
        Vec::new()
    }
}

fn find_label_index(tasks: &[Task], target: &str) -> Result<usize> {
    for (i, t) in tasks.iter().enumerate() {
        if t.get("cmd").and_then(Value::as_str) == Some("label")
            && t.get("name").and_then(Value::as_str) == Some(target)
        {
            return Ok(i);
        }
    }
    Err(anyhow!("Label '{target}' not found"))
}

fn output_tail(text: &str) -> String {
    text.to_string()
}

fn resolve_path(ctx: &ProgramLoadContext, path: &str) -> PathBuf {
    let expanded = shellexpand::tilde(path).to_string();
    let p = PathBuf::from(expanded);
    if p.is_absolute() {
        p
    } else {
        ctx.program_dir.join(p)
    }
}

async fn main_menu(
    program: &mut Program,
    state: &Arc<Mutex<State>>,
    completion_args: &mut Map<String, Value>,
    named_tasks: &HashMap<String, Task>,
    options: &RuntimeOptions,
    ui: &UiCommandHandle,
    ctx: &ProgramLoadContext,
) -> Result<MenuAction> {
    let mut status = String::new();
    loop {
        let choice = match ui
            .select_index(
                vec![
                    "Save State".to_string(),
                    "Load State".to_string(),
                    "Reload and Restart".to_string(),
                    "Quit".to_string(),
                ],
                if status.is_empty() { None } else { Some(status.clone()) },
                false,
            )
            .await
        {
            Ok(value) => value,
            Err(e) => {
                if is_cancelled(&e) {
                    return Ok(MenuAction::Close);
                }
                return Err(e);
            }
        };
        match choice {
            0 => {
                let slots = collect_slots(&program.save_states);
                let labels = slots.iter().map(|s| s.label.clone()).collect::<Vec<_>>();
                let idx = match ui.select_index(labels, None, false).await {
                    Ok(value) => value,
                    Err(e) => {
                        if is_cancelled(&e) {
                            return Ok(MenuAction::Close);
                        }
                        return Err(e);
                    }
                };
                let default_label = slots[idx].label.clone();
                let label = match ui
                    .user_input(
                        "What do you want to call this save state?\n> ".to_string(),
                        if default_label == "(Empty Slot)" { "".to_string() } else { default_label },
                        false,
                    )
                    .await
                {
                    Ok(value) => value,
                    Err(e) => {
                        if is_cancelled(&e) {
                            return Ok(MenuAction::Close);
                        }
                        return Err(e);
                    }
                };
                let mut st = state.lock().await;
                let mut saved = st.data.clone();
                saved.insert("label".to_string(), Value::String(label.clone()));
                program
                    .save_states
                    .insert((idx + 1).to_string(), Value::Object(saved));
                save_program(program, ctx)?;
                status = format!("Saved '{label}' to slot {}.", idx + 1);
                continue;
            }
            1 => {
                let slots = collect_slots(&program.save_states);
                let labels = slots.iter().map(|s| s.label.clone()).collect::<Vec<_>>();
                let idx = match ui.select_index(labels, None, false).await {
                    Ok(value) => value,
                    Err(e) => {
                        if is_cancelled(&e) {
                            return Ok(MenuAction::Close);
                        }
                        return Err(e);
                    }
                };
                if slots[idx].is_empty {
                    status = "Cannot load empty slot.".to_string();
                    continue;
                }
                let mut st = state.lock().await;
                st.data = slots[idx].data.clone();
                if !st.data.contains_key("output") {
                    st.data.insert("output".to_string(), Value::String(String::new()));
                }
                let output = st.get_output();
                ui.set_output(output);
                status = format!("Loaded '{}'.", slots[idx].label);
                continue;
            }
            2 => {
                let mut load_ctx = ProgramLoadContext::new(ctx.program_path.clone(), ctx.inserts_dir.clone())?;
                let mut new_program = crate::parser::load_program(&mut load_ctx)?;
                crate::analyzer::analyze_program(&new_program, &load_ctx)?;
                let mut st = state.lock().await;
                let args: HashMap<String, Value> = st
                    .inserts()
                    .iter()
                    .filter(|(k, _)| k.starts_with("ARG") && k[3..].chars().all(|c| c.is_ascii_digit()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                st.data = new_program.default_state.clone();
                if !st.data.contains_key("output") {
                    st.data.insert("output".to_string(), Value::String(String::new()));
                }
                for (k, v) in args {
                    st.inserts_mut().insert(k, v);
                }
                program.order = new_program.order;
                program.named_tasks = new_program.named_tasks;
                program.save_states = new_program.save_states;
                program.completion_args = new_program.completion_args;
                completion_args.clear();
                completion_args.extend(program.completion_args.clone());
                status = "Restarted program after reloading.".to_string();
                continue;
            }
            3 => return Ok(MenuAction::Quit),
            _ => {}
        }
        return Ok(MenuAction::Close);
    }
}

fn is_cancelled(err: &anyhow::Error) -> bool {
    err.to_string() == "cancelled"
}

fn save_program(program: &Program, ctx: &ProgramLoadContext) -> Result<()> {
    let raw = fs::read_to_string(&ctx.program_path)?;
    let new_content = splice_key_into_json5(&raw, "save_states", &Value::Object(program.save_states.clone()), 4)?;
    fs::write(&ctx.program_path, new_content)?;
    Ok(())
}

struct Slot {
    label: String,
    data: Map<String, Value>,
    is_empty: bool,
}

fn collect_slots(save_states: &Map<String, Value>) -> Vec<Slot> {
    let mut slots = Vec::new();
    for i in 1..=9 {
        if let Some(val) = save_states.get(&i.to_string()).and_then(Value::as_object) {
            let label = val
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or("(Unlabelled Slot)")
                .to_string();
            slots.push(Slot {
                label,
                data: val.clone(),
                is_empty: false,
            });
        } else {
            slots.push(Slot {
                label: "(Empty Slot)".to_string(),
                data: Map::new(),
                is_empty: true,
            });
        }
    }
    slots
}

enum MenuAction {
    Close,
    Quit,
}

fn interpolate_messages(
    messages: Value,
    inserts: &Map<String, Value>,
    ctx: &ProgramLoadContext,
) -> Result<Vec<Map<String, Value>>> {
    if let Some(s) = messages.as_str() {
        if let Some(key) = get_simple_insertkey(s) {
            let val = get_interpdata(inserts, &key, ctx)?;
            return interpolate_messages(val, inserts, ctx);
        }
    }
    let arr = messages
        .as_array()
        .ok_or_else(|| anyhow!("chat.messages must be array or interpolated array"))?;
    let mut out = Vec::new();
    for msg in arr {
        if let Some(obj) = msg.as_object() {
            let role = obj.get("role").and_then(Value::as_str).unwrap_or("user");
            let content = obj.get("content").and_then(Value::as_str).unwrap_or("");
            let content_val = interpolate_inserts(inserts, content, ctx)?;
            let mut m = Map::new();
            m.insert("role".to_string(), Value::String(role.to_string()));
            m.insert("content".to_string(), Value::String(value_to_string(&content_val).trim().to_string()));
            out.push(m);
        }
    }
    Ok(out)
}

#[derive(Clone)]
enum Io {
    Ui(UiCommandHandle),
    Agent(Arc<Mutex<AgentIo>>),
}

impl Io {
    async fn write(&self, text: String) {
        match self {
            Io::Ui(ui) => ui.write(text),
            Io::Agent(agent) => {
                agent.lock().await.write(text);
            }
        }
    }
    async fn clear(&self) {
        match self {
            Io::Ui(ui) => ui.clear(),
            Io::Agent(agent) => {
                agent.lock().await.clear();
            }
        }
    }
    async fn set_output(&self, text: String) {
        match self {
            Io::Ui(ui) => ui.set_output(text),
            Io::Agent(agent) => {
                agent.lock().await.set_output(text);
            }
        }
    }
    async fn user_input(&self, prompt: String, default: String, allow_menu_toggle: bool) -> Result<String> {
        match self {
            Io::Ui(ui) => ui.user_input(prompt, default, allow_menu_toggle).await,
            Io::Agent(agent) => agent.lock().await.user_input(prompt).await,
        }
    }
    async fn select_index(&self, options: Vec<String>, description: Option<String>, allow_menu_toggle: bool) -> Result<usize> {
        match self {
            Io::Ui(ui) => ui.select_index(options, description, allow_menu_toggle).await,
            Io::Agent(agent) => agent.lock().await.select_index(options, description).await,
        }
    }
    async fn start_tts_stream(&self, voice_path: &str, voice_speaker: Option<i64>) -> Result<TtsWriter> {
        match self {
            Io::Ui(_) => TtsWriter::start(voice_path, voice_speaker),
            Io::Agent(_) => Ok(TtsWriter::noop()),
        }
    }
    async fn stop_tts(&self) -> Result<()> {
        Ok(())
    }
    async fn speak(&self, text: &str, voice_path: &str, voice_speaker: Option<i64>) -> Result<()> {
        let mut writer = TtsWriter::start(voice_path, voice_speaker)?;
        writer.write(text)?;
        Ok(())
    }
}

struct AgentIo {
    output: String,
    input_path: PathBuf,
    output_path: PathBuf,
}

impl AgentIo {
    fn new(input: PathBuf, output: PathBuf) -> Self {
        Self {
            output: String::new(),
            input_path: input,
            output_path: output,
        }
    }
    fn write(&mut self, text: String) {
        self.output.push_str(&text);
    }
    fn clear(&mut self) {
        self.output.clear();
    }
    fn set_output(&mut self, text: String) {
        self.output = text;
    }
    async fn user_input(&mut self, prompt: String) -> Result<String> {
        let payload = json!({
            "type": "user_input",
            "output": self.output,
            "prompt": prompt,
        });
        let _ = fs::remove_file(&self.input_path);
        fs::write(&self.output_path, serde_json::to_string_pretty(&payload)?)?;
        loop {
            if self.input_path.exists() {
                let data = fs::read_to_string(&self.input_path)?;
                let _ = fs::remove_file(&self.input_path);
                return Ok(data.trim_end_matches('\n').to_string());
            }
            sleep(Duration::from_millis(100)).await;
        }
    }
    async fn select_index(&mut self, options: Vec<String>, description: Option<String>) -> Result<usize> {
        if options.is_empty() {
            let payload = json!({
                "type": "user_choice",
                "output": self.output,
                "prompt": description,
                "choices": HashMap::<String, String>::new(),
            });
            let _ = fs::remove_file(&self.input_path);
            fs::write(&self.output_path, serde_json::to_string_pretty(&payload)?)?;
            return Ok(0);
        }
        let keys = if options.len() <= 9 {
            (1..=options.len()).map(|i| i.to_string()).collect::<Vec<_>>()
        } else {
            (0..options.len()).map(|i| ((b'a' + i as u8) as char).to_string()).collect()
        };
        let choice_map: HashMap<String, usize> = keys.iter().enumerate().map(|(i, k)| (k.clone(), i)).collect();
        let payload = json!({
            "type": "user_choice",
            "output": self.output,
            "prompt": description,
            "choices": keys.iter().enumerate().map(|(i,k)| (k.clone(), options[i].clone())).collect::<HashMap<String,String>>(),
        });
        let _ = fs::remove_file(&self.input_path);
        fs::write(&self.output_path, serde_json::to_string_pretty(&payload)?)?;
        loop {
            if self.input_path.exists() {
                let data = fs::read_to_string(&self.input_path)?;
                let _ = fs::remove_file(&self.input_path);
                let text = data.trim();
                if let Some(idx) = choice_map.get(text) {
                    return Ok(*idx);
                }
                if let Some(idx) = options.iter().position(|o| o == text) {
                    return Ok(idx);
                }
                return Err(anyhow!("Invalid agent choice '{text}'"));
            }
            sleep(Duration::from_millis(100)).await;
        }
    }
}

struct TtsWriter {
    child: Option<std::process::Child>,
    buffer: String,
    web: Option<audio_web::AudioBroadcaster>,
    _reader: Option<std::thread::JoinHandle<()>>,
}

impl TtsWriter {
    fn start(voice_path: &str, voice_speaker: Option<i64>) -> Result<Self> {
        if !which::which("piper").is_ok() {
            return Err(anyhow!("voice_path was set but 'piper' was not found on PATH."));
        }
        if !which::which("pw-play").is_ok() {
            if !audio_web::config().enabled {
                return Err(anyhow!("voice_path was set but 'pw-play' was not found on PATH."));
            }
        }
        if !std::path::Path::new(voice_path).exists() {
            return Err(anyhow!("voice_path does not exist: {voice_path}"));
        }
        if std::path::Path::new(voice_path).is_dir() {
            return Err(anyhow!("voice_path is a directory, expected a file: {voice_path}"));
        }
        let mut rate = 22050;
        let mut channels = 1;
        let config_path = if voice_path.ends_with(".onnx") && std::path::Path::new(&format!("{voice_path}.json")).exists() {
            Some(format!("{voice_path}.json"))
        } else if std::path::Path::new(&format!("{voice_path}.onnx.json")).exists() {
            Some(format!("{voice_path}.onnx.json"))
        } else {
            None
        };
        if let Some(cfg_path) = config_path {
            if let Ok(raw) = fs::read_to_string(cfg_path) {
                if let Ok(cfg) = serde_json::from_str::<Value>(&raw) {
                    if let Some(audio) = cfg.get("audio").and_then(Value::as_object) {
                        if let Some(sr) = audio.get("sample_rate").and_then(Value::as_i64) {
                            rate = sr as i32;
                        }
                        if let Some(ch) = audio.get("channels").and_then(Value::as_i64) {
                            channels = ch as i32;
                        }
                    } else {
                        if let Some(sr) = cfg.get("sample_rate").and_then(Value::as_i64) {
                            rate = sr as i32;
                        }
                        if let Some(ch) = cfg.get("channels").and_then(Value::as_i64) {
                            channels = ch as i32;
                        }
                    }
                }
            }
        }
        let mut cmd = std::process::Command::new("piper");
        cmd.arg("--model").arg(voice_path).arg("--output-raw");
        if let Some(speaker) = voice_speaker {
            cmd.arg("--speaker").arg(speaker.to_string());
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        let mut child = cmd.spawn()?;
        let mut web = None;
        let mut reader = None;
        if audio_web::config().enabled {
            let broadcaster = audio_web::get_or_start(rate as u32, channels as u16)?;
            if let Some(stdout) = child.stdout.take() {
                let tx = broadcaster.clone();
                reader = Some(std::thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    let mut rdr = std::io::BufReader::new(stdout);
                    loop {
                        match std::io::Read::read(&mut rdr, &mut buf) {
                            Ok(0) => break,
                            Ok(n) => tx.send(buf[..n].to_vec()),
                            Err(_) => break,
                        }
                    }
                }));
                web = Some(broadcaster);
            }
        } else {
            let piper_out = child
                .stdout
                .take()
                .ok_or_else(|| anyhow!("Failed to open Piper stdout"))?;
            let mut pw = std::process::Command::new("pw-play");
            pw.arg("-a")
                .arg("--rate")
                .arg(rate.to_string())
                .arg("--channels")
                .arg(channels.to_string())
                .arg("--format")
                .arg("s16")
                .arg("-")
                .stdin(piper_out);
            let _ = pw.spawn();
        }
        Ok(Self {
            child: Some(child),
            buffer: String::new(),
            web,
            _reader: reader,
        })
    }

    fn noop() -> Self {
        Self {
            child: None,
            buffer: String::new(),
            web: None,
            _reader: None,
        }
    }

    fn write(&mut self, text: &str) -> Result<()> {
        self.buffer.push_str(text);
        self.flush_buffer(false)?;
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        self.flush_buffer(true)
    }

    fn flush_buffer(&mut self, force: bool) -> Result<()> {
        if let Some(child) = &mut self.child {
            if let Some(stdin) = &mut child.stdin {
                use std::io::Write;
                while let Some(idx) = self.buffer.find('\n') {
                    let line = self.buffer[..idx].trim();
                    if !line.is_empty() {
                        stdin.write_all(line.as_bytes())?;
                        stdin.write_all(b"\n")?;
                        stdin.flush()?;
                    }
                    self.buffer = self.buffer[idx + 1..].to_string();
                }

                if force {
                    let line = self.buffer.trim();
                    if !line.is_empty() {
                        stdin.write_all(line.as_bytes())?;
                        stdin.write_all(b"\n")?;
                        stdin.flush()?;
                    }
                    self.buffer.clear();
                    return Ok(());
                }

                if let Some(idx) = last_sentence_end(&self.buffer) {
                    let line = self.buffer[..idx].trim();
                    if !line.is_empty() {
                        stdin.write_all(line.as_bytes())?;
                        stdin.write_all(b"\n")?;
                        stdin.flush()?;
                    }
                    self.buffer = self.buffer[idx..].to_string();
                }
            }
        }
        Ok(())
    }
}

fn last_sentence_end(text: &str) -> Option<usize> {
    let mut last = None;
    for (i, ch) in text.char_indices() {
        if ch == '.' || ch == '!' || ch == '?' {
            last = Some(i + ch.len_utf8());
        }
    }
    last
}
