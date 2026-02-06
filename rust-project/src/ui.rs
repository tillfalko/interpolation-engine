use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEventKind, DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::Text,
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal,
};
use std::io::{self, Stdout, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

#[derive(Debug)]
pub enum UiEvent {
    ToggleMenu,
    Quit,
}

#[derive(Debug)]
pub enum UiCommand {
    Write(String),
    Clear,
    SetOutput(String),
    BeginInput {
        prompt: String,
        default: String,
        allow_menu_toggle: bool,
        respond_to: oneshot::Sender<String>,
    },
    BeginChoice {
        options: Vec<String>,
        description: Option<String>,
        allow_menu_toggle: bool,
        respond_to: oneshot::Sender<usize>,
    },
    CancelInput,
    Shutdown,
}

#[derive(Clone)]
pub struct UiCommandHandle {
    cmd_tx: Sender<UiCommand>,
}

pub fn start_ui(history_path: Option<PathBuf>) -> (UiCommandHandle, tokio::sync::mpsc::UnboundedReceiver<UiEvent>, JoinHandle<()>) {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = spawn_ui_thread(cmd_rx, event_tx, history_path);
    (UiCommandHandle { cmd_tx }, event_rx, handle)
}

impl UiCommandHandle {

    pub fn write(&self, text: String) {
        let _ = self.cmd_tx.send(UiCommand::Write(text));
    }

    pub fn clear(&self) {
        let _ = self.cmd_tx.send(UiCommand::Clear);
    }

    pub fn set_output(&self, text: String) {
        let _ = self.cmd_tx.send(UiCommand::SetOutput(text));
    }

    pub async fn user_input(&self, prompt: String, default: String, allow_menu_toggle: bool) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        let _ = self.cmd_tx.send(UiCommand::BeginInput {
            prompt,
            default,
            allow_menu_toggle,
            respond_to: tx,
        });
        match rx.await {
            Ok(value) => Ok(value),
            Err(_) => Err(anyhow::anyhow!("cancelled")),
        }
    }

    pub async fn select_index(
        &self,
        options: Vec<String>,
        description: Option<String>,
        allow_menu_toggle: bool,
    ) -> Result<usize> {
        let (tx, rx) = oneshot::channel();
        let _ = self.cmd_tx.send(UiCommand::BeginChoice {
            options,
            description,
            allow_menu_toggle,
            respond_to: tx,
        });
        match rx.await {
            Ok(value) => Ok(value),
            Err(_) => Err(anyhow::anyhow!("cancelled")),
        }
    }

    pub fn cancel_input(&self) {
        let _ = self.cmd_tx.send(UiCommand::CancelInput);
    }

    pub fn shutdown(&self) {
        let _ = self.cmd_tx.send(UiCommand::Shutdown);
    }
}

#[derive(Debug)]
enum Mode {
    Idle,
    Input {
        prompt_inline: String,
        buffer: String,
        cursor: usize,
        allow_menu_toggle: bool,
        respond_to: Option<oneshot::Sender<String>>,
    },
    Search {
        prompt_inline: String,
        buffer: String,
        allow_menu_toggle: bool,
        respond_to: Option<oneshot::Sender<String>>,
        query: String,
        original: String,
        match_index: Option<usize>,
    },
    Choice {
        description: Option<String>,
        options: Vec<String>,
        keys: Vec<String>,
        allow_menu_toggle: bool,
        respond_to: Option<oneshot::Sender<usize>>,
    },
}

struct UiState {
    output: String,
    info: String,
    mode: Mode,
    history_path: Option<PathBuf>,
    history: Vec<String>,
    history_cursor: Option<usize>,
    history_stash: Option<String>,
    output_scroll: usize,
    auto_scroll: bool,
    last_layout: Option<LayoutInfo>,
    dirty: bool,
}

#[derive(Debug, Clone, Copy)]
struct LayoutInfo {
    output_height: usize,
    max_scroll: usize,
}

fn spawn_ui_thread(
    cmd_rx: Receiver<UiCommand>,
    event_tx: UnboundedSender<UiEvent>,
    history_path: Option<PathBuf>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut terminal = setup_terminal().ok();
        let mut state = UiState {
            output: String::new(),
            info: String::new(),
            mode: Mode::Idle,
            history_path,
            history: Vec::new(),
            history_cursor: None,
            history_stash: None,
            output_scroll: 0,
            auto_scroll: true,
            last_layout: None,
            dirty: true,
        };
        if let Some(path) = &state.history_path {
            state.history = load_history(path);
        }

        let mut last_draw = Instant::now();

        loop {
            while let Ok(cmd) = cmd_rx.try_recv() {
                if matches!(cmd, UiCommand::Shutdown) {
                    cleanup_terminal(terminal.take());
                    return;
                }
                if handle_command(cmd, &mut state) {
                    state.dirty = true;
                }
            }

            if event::poll(Duration::from_millis(10)).unwrap_or(false) {
                if let Ok(event) = event::read() {
                    let (quit, changed) = match event {
                        Event::Key(key) => handle_key(key, &mut state, &event_tx),
                        Event::Mouse(mouse) => (false, handle_mouse(mouse.kind, &mut state)),
                        _ => (false, false),
                    };
                    if changed {
                        state.dirty = true;
                    }
                    if quit {
                        cleanup_terminal(terminal.take());
                        return;
                    }
                }
            }

            if state.dirty || last_draw.elapsed() > Duration::from_millis(200) {
                if let Some(term) = terminal.as_mut() {
                    let _ = draw(term, &mut state);
                }
                last_draw = Instant::now();
                state.dirty = false;
            }
        }
    })
}

fn handle_command(cmd: UiCommand, state: &mut UiState) -> bool {
    match cmd {
        UiCommand::Write(text) => {
            state.output.push_str(&text);
            if state.auto_scroll {
                if let Some(layout) = state.last_layout {
                    state.output_scroll = layout.max_scroll;
                }
            }
            true
        }
        UiCommand::Clear => {
            state.output.clear();
            state.output_scroll = 0;
            state.auto_scroll = true;
            true
        }
        UiCommand::SetOutput(text) => {
            state.output = text;
            if state.auto_scroll {
                if let Some(layout) = state.last_layout {
                    state.output_scroll = layout.max_scroll;
                }
            }
            true
        }
        UiCommand::BeginInput {
            prompt,
            default,
            allow_menu_toggle,
            respond_to,
        } => {
            let (outline, inline) = split_prompt(&prompt);
            let cursor = default.len();
            state.info = outline;
            state.mode = Mode::Input {
                prompt_inline: inline,
                buffer: default,
                cursor,
                allow_menu_toggle,
                respond_to: Some(respond_to),
            };
            state.history_cursor = None;
            state.history_stash = None;
            true
        }
        UiCommand::BeginChoice {
            options,
            description,
            allow_menu_toggle,
            respond_to,
        } => {
            let keys = build_choice_keys(options.len());
            state.mode = Mode::Choice {
                description,
                options,
                keys,
                allow_menu_toggle,
                respond_to: Some(respond_to),
            };
            true
        }
        UiCommand::CancelInput => {
            match &mut state.mode {
                Mode::Input { .. } | Mode::Search { .. } | Mode::Choice { .. } => {
                    state.mode = Mode::Idle;
                    true
                }
                _ => false,
            }
        }
        UiCommand::Shutdown => false,
    }
}

fn handle_key(key: KeyEvent, state: &mut UiState, event_tx: &UnboundedSender<UiEvent>) -> (bool, bool) {
    if key.code == KeyCode::Esc {
        match &state.mode {
            Mode::Input { allow_menu_toggle: false, .. }
            | Mode::Choice { allow_menu_toggle: false, .. } => {
                state.mode = Mode::Idle;
                return (false, true);
            }
            _ => {
                let _ = event_tx.send(UiEvent::ToggleMenu);
                state.mode = Mode::Idle;
                return (false, true);
            }
        }
    }
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        let _ = event_tx.send(UiEvent::Quit);
        return (true, true);
    }

    let mut changed = false;
    match &mut state.mode {
        Mode::Input {
            prompt_inline: _,
            buffer,
            cursor,
            respond_to,
            ..
        } => match key.code {
            KeyCode::Enter => {
                let text = buffer.clone();
                if let Some(path) = &state.history_path {
                    let _ = append_history(path, &text);
                }
                if let Some(tx) = respond_to.take() {
                    let _ = tx.send(text);
                }
                state.mode = Mode::Idle;
                changed = true;
            }
            KeyCode::Backspace => {
                let new_cursor = prev_char_index(buffer, *cursor);
                if new_cursor < *cursor {
                    buffer.replace_range(new_cursor..*cursor, "");
                    *cursor = new_cursor;
                }
                state.history_cursor = None;
                changed = true;
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                buffer.insert(*cursor, '\n');
                *cursor += 1;
                state.history_cursor = None;
                changed = true;
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let original = buffer.clone();
                let (prompt_inline, allow_menu_toggle, respond_to) = match std::mem::replace(&mut state.mode, Mode::Idle) {
                    Mode::Input { prompt_inline, allow_menu_toggle, respond_to, .. } => {
                        (prompt_inline, allow_menu_toggle, respond_to)
                    }
                    other => {
                        state.mode = other;
                        return (false, false);
                    }
                };
                let match_index = find_history_match(&state.history, "", None);
                let buffer = match_index.and_then(|i| state.history.get(i).cloned()).unwrap_or_else(|| original.clone());
                state.mode = Mode::Search {
                    prompt_inline,
                    buffer,
                    allow_menu_toggle,
                    respond_to,
                    query: String::new(),
                    original,
                    match_index,
                };
                changed = true;
            }
            KeyCode::Up => {
                if state.history.is_empty() {
                    return (false, false);
                }
                let idx = match state.history_cursor {
                    None => {
                        state.history_stash = Some(buffer.clone());
                        state.history.len().saturating_sub(1)
                    }
                    Some(i) => i.saturating_sub(1),
                };
                if let Some(entry) = state.history.get(idx).cloned() {
                    *buffer = entry;
                    *cursor = buffer.len();
                    state.history_cursor = Some(idx);
                }
                changed = true;
            }
            KeyCode::Down => {
                if state.history.is_empty() {
                    return (false, false);
                }
                if let Some(i) = state.history_cursor {
                    if i + 1 < state.history.len() {
                        let idx = i + 1;
                        if let Some(entry) = state.history.get(idx).cloned() {
                            *buffer = entry;
                            *cursor = buffer.len();
                            state.history_cursor = Some(idx);
                        }
                    } else {
                        state.history_cursor = None;
                        if let Some(stash) = state.history_stash.take() {
                            *buffer = stash;
                            *cursor = buffer.len();
                        }
                    }
                }
                changed = true;
            }
            KeyCode::Left => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    *cursor = prev_word_index(buffer, *cursor);
                } else {
                    *cursor = prev_char_index(buffer, *cursor);
                }
                state.history_cursor = None;
                changed = true;
            }
            KeyCode::Right => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    *cursor = next_word_index(buffer, *cursor);
                } else {
                    *cursor = next_char_index(buffer, *cursor);
                }
                state.history_cursor = None;
                changed = true;
            }
            KeyCode::Home => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    changed = scroll_output_key(key.code, state);
                } else {
                    *cursor = 0;
                    state.history_cursor = None;
                    changed = true;
                }
            }
            KeyCode::End => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    changed = scroll_output_key(key.code, state);
                } else {
                    *cursor = buffer.len();
                    state.history_cursor = None;
                    changed = true;
                }
            }
            KeyCode::Delete => {
                let next = next_char_index(buffer, *cursor);
                if next > *cursor {
                    buffer.replace_range(*cursor..next, "");
                    state.history_cursor = None;
                    changed = true;
                }
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                *cursor = 0;
                state.history_cursor = None;
                changed = true;
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                *cursor = buffer.len();
                state.history_cursor = None;
                changed = true;
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let new_cursor = prev_word_index(buffer, *cursor);
                if new_cursor < *cursor {
                    buffer.replace_range(new_cursor..*cursor, "");
                    *cursor = new_cursor;
                }
                state.history_cursor = None;
                changed = true;
            }
            KeyCode::Char(c) => {
                buffer.insert(*cursor, c);
                *cursor += c.len_utf8();
                state.history_cursor = None;
                changed = true;
            }
            KeyCode::PageUp | KeyCode::PageDown
                if key.modifiers.contains(KeyModifiers::CONTROL) || key.code == KeyCode::PageUp || key.code == KeyCode::PageDown =>
            {
                changed = scroll_output_key(key.code, state);
            }
            _ => {}
        },
        Mode::Search { .. } => {
            let mode = std::mem::replace(&mut state.mode, Mode::Idle);
            let mut m = match mode {
                Mode::Search {
                    prompt_inline,
                    buffer,
                    allow_menu_toggle,
                    respond_to,
                    query,
                    original,
                    match_index,
                } => (prompt_inline, buffer, allow_menu_toggle, respond_to, query, original, match_index),
                other => {
                    state.mode = other;
                    return (false, false);
                }
            };
            match key.code {
                KeyCode::Esc => {
                    let cursor = m.5.len();
                    state.mode = Mode::Input {
                        prompt_inline: m.0,
                        buffer: m.5,
                        cursor,
                        allow_menu_toggle: m.2,
                        respond_to: m.3,
                    };
                    changed = true;
                }
                KeyCode::Enter => {
                    state.mode = Mode::Input {
                        prompt_inline: m.0,
                        buffer: m.1.clone(),
                        cursor: m.1.len(),
                        allow_menu_toggle: m.2,
                        respond_to: m.3,
                    };
                    changed = true;
                }
                KeyCode::Backspace => {
                    m.4.pop();
                    m.6 = find_history_match(&state.history, &m.4, None);
                    if let Some(i) = m.6 {
                        if let Some(entry) = state.history.get(i).cloned() {
                            m.1 = entry;
                        }
                    } else {
                        m.1 = m.5.clone();
                    }
                    state.mode = Mode::Search {
                        prompt_inline: m.0,
                        buffer: m.1,
                        allow_menu_toggle: m.2,
                        respond_to: m.3,
                        query: m.4,
                        original: m.5,
                        match_index: m.6,
                    };
                    changed = true;
                }
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let start = m.6.and_then(|i| i.checked_sub(1));
                    m.6 = find_history_match(&state.history, &m.4, start);
                    if let Some(i) = m.6 {
                        if let Some(entry) = state.history.get(i).cloned() {
                            m.1 = entry;
                        }
                    }
                    state.mode = Mode::Search {
                        prompt_inline: m.0,
                        buffer: m.1,
                        allow_menu_toggle: m.2,
                        respond_to: m.3,
                        query: m.4,
                        original: m.5,
                        match_index: m.6,
                    };
                    changed = true;
                }
                KeyCode::Char(c) => {
                    m.4.push(c);
                    m.6 = find_history_match(&state.history, &m.4, None);
                    if let Some(i) = m.6 {
                        if let Some(entry) = state.history.get(i).cloned() {
                            m.1 = entry;
                        }
                    } else {
                        m.1 = m.5.clone();
                    }
                    state.mode = Mode::Search {
                        prompt_inline: m.0,
                        buffer: m.1,
                        allow_menu_toggle: m.2,
                        respond_to: m.3,
                        query: m.4,
                        original: m.5,
                        match_index: m.6,
                    };
                    changed = true;
                }
                KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End => {
                    changed = scroll_output_key(key.code, state);
                }
                _ => {
                    state.mode = Mode::Search {
                        prompt_inline: m.0,
                        buffer: m.1,
                        allow_menu_toggle: m.2,
                        respond_to: m.3,
                        query: m.4,
                        original: m.5,
                        match_index: m.6,
                    };
                }
            }
        }
        Mode::Choice {
            options,
            keys,
            respond_to,
            ..
        } => {
            if options.is_empty() {
                if let Some(tx) = respond_to.take() {
                    let _ = tx.send(0);
                }
                state.mode = Mode::Idle;
                return (false, true);
            }
            match key.code {
                KeyCode::Char(c) => {
                    let key_str = c.to_string();
                    if let Some(idx) = keys.iter().position(|k| k == &key_str) {
                        if let Some(tx) = respond_to.take() {
                            let _ = tx.send(idx);
                        }
                        state.mode = Mode::Idle;
                        changed = true;
                    } else {
                        if let Some(idx) = options.iter().position(|o| o == &key_str) {
                            if let Some(tx) = respond_to.take() {
                                let _ = tx.send(idx);
                            }
                            state.mode = Mode::Idle;
                            changed = true;
                        }
                    }
                }
                KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End => {
                    changed = scroll_output_key(key.code, state);
                }
                _ => {}
            }
        }
        Mode::Idle => {
            match key.code {
                KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End => {
                    changed = scroll_output_key(key.code, state);
                }
                KeyCode::Up | KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    changed = scroll_output_key(key.code, state);
                }
                _ => {}
            }
        }
    }

    (false, changed)
}

fn handle_mouse(kind: MouseEventKind, state: &mut UiState) -> bool {
    match kind {
        MouseEventKind::ScrollUp => scroll_output_lines(state, 3),
        MouseEventKind::ScrollDown => scroll_output_lines(state, -3),
        _ => false,
    }
}

fn scroll_output_key(code: KeyCode, state: &mut UiState) -> bool {
    match code {
        KeyCode::PageUp => scroll_output_page(state, -1),
        KeyCode::PageDown => scroll_output_page(state, 1),
        KeyCode::Home => scroll_output_home_end(state, true),
        KeyCode::End => scroll_output_home_end(state, false),
        KeyCode::Up => scroll_output_lines(state, 1),
        KeyCode::Down => scroll_output_lines(state, -1),
        _ => false,
    }
}

fn scroll_output_page(state: &mut UiState, pages: i32) -> bool {
    let Some(layout) = state.last_layout else { return false };
    if layout.output_height == 0 {
        return false;
    }
    let delta = layout.output_height as i32 * pages;
    scroll_output_delta(state, delta)
}

fn scroll_output_lines(state: &mut UiState, lines: i32) -> bool {
    scroll_output_delta(state, -lines)
}

fn scroll_output_home_end(state: &mut UiState, home: bool) -> bool {
    let Some(layout) = state.last_layout else { return false };
    let new_scroll = if home { 0 } else { layout.max_scroll };
    if new_scroll == state.output_scroll && (!home || state.auto_scroll) {
        return false;
    }
    state.output_scroll = new_scroll;
    state.auto_scroll = state.output_scroll == layout.max_scroll;
    true
}

fn scroll_output_delta(state: &mut UiState, delta: i32) -> bool {
    let Some(layout) = state.last_layout else { return false };
    if layout.max_scroll == 0 {
        state.output_scroll = 0;
        state.auto_scroll = true;
        return false;
    }
    let current = state.output_scroll as i32;
    let max_scroll = layout.max_scroll as i32;
    let mut next = current + delta;
    if next < 0 {
        next = 0;
    } else if next > max_scroll {
        next = max_scroll;
    }
    let next_usize = next as usize;
    if next_usize == state.output_scroll {
        return false;
    }
    state.output_scroll = next_usize;
    state.auto_scroll = state.output_scroll == layout.max_scroll;
    true
}

fn append_history(path: &PathBuf, text: &str) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(text.as_bytes())?;
    file.write_all(b"\n")?;
    file.write_all(&[HISTORY_RS])?;
    file.write_all(b"\n")?;
    Ok(())
}

const HISTORY_RS: u8 = 0x1e;

fn load_history(path: &PathBuf) -> Vec<String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    if raw.as_bytes().contains(&HISTORY_RS) {
        raw.split(HISTORY_RS as char)
            .map(|s| s.trim_matches('\n').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        raw.lines()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

fn find_history_match(history: &[String], query: &str, start_from: Option<usize>) -> Option<usize> {
    if history.is_empty() {
        return None;
    }
    let mut idx = start_from.unwrap_or_else(|| history.len().saturating_sub(1));
    loop {
        if history[idx].contains(query) {
            return Some(idx);
        }
        if idx == 0 {
            break;
        }
        idx -= 1;
    }
    None
}

fn draw(terminal: &mut Terminal<CrosstermBackend<Stdout>>, state: &mut UiState) -> io::Result<()> {
    terminal.draw(|f| {
        let size = f.size();
        let info_text = match &state.mode {
            Mode::Choice { description, options, keys, .. } => {
                let mut lines = Vec::new();
                if let Some(desc) = description {
                    lines.push(desc.clone());
                }
                for (i, opt) in options.iter().enumerate() {
                    if let Some(k) = keys.get(i) {
                        lines.push(format!("({}) {}", k, opt));
                    }
                }
                lines.join("\n")
            }
            Mode::Input { .. } => state.info.clone(),
            Mode::Search { query, .. } => format!("reverse-i-search: {query}"),
            _ => String::new(),
        };

        let (prompt_text, cursor_text) = match &state.mode {
            Mode::Input { prompt_inline, buffer, cursor, .. } => {
                let c = (*cursor).min(buffer.len());
                let cursor_slice = &buffer[..c];
                (
                    format!("{prompt_inline}{buffer}"),
                    Some(format!("{prompt_inline}{cursor_slice}")),
                )
            }
            Mode::Search { prompt_inline, buffer, .. } => (format!("{prompt_inline}{buffer}"), None),
            _ => (String::new(), None),
        };

        let width = size.width as usize;
        let height = size.height as usize;
        let prompt_height = wrapped_line_count(&prompt_text, width).min(height);
        let info_pref = wrapped_line_count(&info_text, width).min(height);

        let (mut output_height, info_height) = match &state.mode {
            Mode::Choice { .. } | Mode::Input { .. } | Mode::Search { .. } => {
                let available = height.saturating_sub(prompt_height);
                let info_height = info_pref.min(available);
                let output_height = available.saturating_sub(info_height);
                (output_height, info_height)
            }
            Mode::Idle => {
                let available = height.saturating_sub(prompt_height);
                (available, 0)
            }
        };

        let used = output_height + info_height + prompt_height;
        if used < height {
            output_height += height - used;
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(output_height as u16),
                Constraint::Length(info_height as u16),
                Constraint::Length(prompt_height as u16),
            ])
            .split(size);

        let total_output_lines = wrapped_line_count(&state.output, width);
        let max_scroll = total_output_lines.saturating_sub(output_height);
        state.last_layout = Some(LayoutInfo {
            output_height,
            max_scroll,
        });

        let scroll_offset = if state.auto_scroll {
            max_scroll
        } else {
            state.output_scroll.min(max_scroll)
        };

        let output = Paragraph::new(Text::from(state.output.clone()))
            .wrap(Wrap { trim: false })
            .scroll((scroll_offset.min(u16::MAX as usize) as u16, 0))
            .block(Block::default().borders(Borders::NONE));
        f.render_widget(output, chunks[0]);

        let info = Paragraph::new(info_text.clone())
            .style(Style::default().fg(Color::Yellow))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::NONE));
        f.render_widget(info, chunks[1]);
        let prompt = Paragraph::new(prompt_text.clone())
            .style(Style::default().fg(Color::Yellow))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::NONE));
        f.render_widget(prompt, chunks[2]);

        match &state.mode {
            Mode::Input { .. } => {
                if width > 0 && prompt_height > 0 {
                    let cursor_ref = cursor_text.as_deref().unwrap_or(&prompt_text);
                    let (row, col) = cursor_offset(cursor_ref, width);
                    let x = chunks[2].x.saturating_add(col as u16);
                    let y = chunks[2].y.saturating_add(row as u16);
                    f.set_cursor(x, y);
                }
            }
            Mode::Search { .. } => {
                if width > 0 && info_height > 0 {
                    let (row, col) = cursor_offset(&info_text, width);
                    let x = chunks[1].x.saturating_add(col as u16);
                    let y = chunks[1].y.saturating_add(row as u16);
                    f.set_cursor(x, y);
                }
            }
            _ => {}
        }
    }).map(|_| ())
}

fn split_prompt(prompt: &str) -> (String, String) {
    if let Some((left, right)) = prompt.rsplit_once('\n') {
        (left.to_string(), right.to_string())
    } else {
        (String::new(), prompt.to_string())
    }
}

fn build_choice_keys(n: usize) -> Vec<String> {
    let mut keys = Vec::new();
    if n <= 9 {
        for i in 1..=n {
            keys.push(i.to_string());
        }
    } else {
        for i in 0..n {
            keys.push(((b'a' + i as u8) as char).to_string());
        }
    }
    keys
}

fn wrapped_line_count(text: &str, width: usize) -> usize {
    if text.is_empty() || width == 0 {
        return 0;
    }
    let mut count = 0;
    for line in text.split('\n') {
        let len = line.chars().count();
        let lines = if len == 0 { 1 } else { (len - 1) / width + 1 };
        count += lines;
    }
    count
}

fn cursor_offset(text: &str, width: usize) -> (usize, usize) {
    let mut row = 0;
    let mut col = 0;
    for ch in text.chars() {
        if ch == '\n' {
            row += 1;
            col = 0;
            continue;
        }
        col += 1;
        if col >= width {
            row += 1;
            col = 0;
        }
    }
    (row, col)
}

fn prev_char_index(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut prev = 0;
    for (i, _) in text.char_indices() {
        if i >= cursor {
            break;
        }
        prev = i;
    }
    prev
}

fn next_char_index(text: &str, cursor: usize) -> usize {
    for (i, _) in text.char_indices() {
        if i > cursor {
            return i;
        }
    }
    text.len()
}

fn prev_word_index(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut i = cursor;
    while i > 0 {
        let p = prev_char_index(text, i);
        if !char_at(text, p).is_whitespace() {
            break;
        }
        i = p;
    }
    if i == 0 {
        return 0;
    }
    let p = prev_char_index(text, i);
    let word = is_word_char(char_at(text, p));
    while i > 0 {
        let prev = prev_char_index(text, i);
        let ch = char_at(text, prev);
        if word && !is_word_char(ch) {
            break;
        }
        if !word && ch.is_whitespace() {
            break;
        }
        i = prev;
    }
    i
}

fn next_word_index(text: &str, cursor: usize) -> usize {
    let mut i = cursor;
    if i >= text.len() {
        return text.len();
    }
    while i < text.len() {
        let ch = char_at(text, i);
        if !ch.is_whitespace() {
            break;
        }
        i = next_char_index(text, i);
    }
    if i >= text.len() {
        return text.len();
    }
    let word = is_word_char(char_at(text, i));
    while i < text.len() {
        let ch = char_at(text, i);
        if word && !is_word_char(ch) {
            break;
        }
        if !word && ch.is_whitespace() {
            break;
        }
        i = next_char_index(text, i);
    }
    i
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn char_at(text: &str, idx: usize) -> char {
    text[idx..].chars().next().unwrap_or('\0')
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

fn cleanup_terminal(term: Option<Terminal<CrosstermBackend<Stdout>>>) {
    if let Some(mut terminal) = term {
        let _ = disable_raw_mode();
        let _ = execute!(terminal.backend_mut(), DisableMouseCapture, LeaveAlternateScreen);
        let _ = terminal.show_cursor();
    }
}
