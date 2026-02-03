use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
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
use std::io::{self, Stdout};
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
        respond_to: oneshot::Sender<String>,
    },
    BeginChoice {
        options: Vec<String>,
        description: Option<String>,
        respond_to: oneshot::Sender<usize>,
    },
    CancelInput,
    Shutdown,
}

#[derive(Clone)]
pub struct UiCommandHandle {
    cmd_tx: Sender<UiCommand>,
}

pub fn start_ui() -> (UiCommandHandle, tokio::sync::mpsc::UnboundedReceiver<UiEvent>, JoinHandle<()>) {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = spawn_ui_thread(cmd_rx, event_tx);
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

    pub async fn user_input(&self, prompt: String, default: String) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        let _ = self.cmd_tx.send(UiCommand::BeginInput {
            prompt,
            default,
            respond_to: tx,
        });
        Ok(rx.await?)
    }

    pub async fn select_index(
        &self,
        options: Vec<String>,
        description: Option<String>,
    ) -> Result<usize> {
        let (tx, rx) = oneshot::channel();
        let _ = self.cmd_tx.send(UiCommand::BeginChoice {
            options,
            description,
            respond_to: tx,
        });
        Ok(rx.await?)
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
        respond_to: Option<oneshot::Sender<String>>,
    },
    Choice {
        description: Option<String>,
        options: Vec<String>,
        keys: Vec<String>,
        respond_to: Option<oneshot::Sender<usize>>,
    },
}

struct UiState {
    output: String,
    info: String,
    mode: Mode,
}

fn spawn_ui_thread(cmd_rx: Receiver<UiCommand>, event_tx: UnboundedSender<UiEvent>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut terminal = setup_terminal().ok();
        let mut state = UiState {
            output: String::new(),
            info: String::new(),
            mode: Mode::Idle,
        };

        let mut last_draw = Instant::now();

        loop {
            while let Ok(cmd) = cmd_rx.try_recv() {
                if matches!(cmd, UiCommand::Shutdown) {
                    cleanup_terminal(terminal.take());
                    return;
                }
                handle_command(cmd, &mut state);
            }

            if event::poll(Duration::from_millis(10)).unwrap_or(false) {
                if let Ok(Event::Key(key)) = event::read() {
                    if handle_key(key, &mut state, &event_tx) {
                        cleanup_terminal(terminal.take());
                        return;
                    }
                }
            }

            if last_draw.elapsed() > Duration::from_millis(16) {
                if let Some(term) = terminal.as_mut() {
                    let _ = draw(term, &state);
                }
                last_draw = Instant::now();
            }
        }
    })
}

fn handle_command(cmd: UiCommand, state: &mut UiState) {
    match cmd {
        UiCommand::Write(text) => state.output.push_str(&text),
        UiCommand::Clear => {
            state.output.clear();
        }
        UiCommand::SetOutput(text) => state.output = text,
        UiCommand::BeginInput {
            prompt,
            default,
            respond_to,
        } => {
            let (outline, inline) = split_prompt(&prompt);
            state.info = outline;
            state.mode = Mode::Input {
                prompt_inline: inline,
                buffer: default,
                respond_to: Some(respond_to),
            };
        }
        UiCommand::BeginChoice {
            options,
            description,
            respond_to,
        } => {
            let keys = build_choice_keys(options.len());
            state.mode = Mode::Choice {
                description,
                options,
                keys,
                respond_to: Some(respond_to),
            };
        }
        UiCommand::CancelInput => {
            match &mut state.mode {
                Mode::Input { .. } | Mode::Choice { .. } => {
                    state.mode = Mode::Idle;
                }
                _ => {}
            }
        }
        UiCommand::Shutdown => {}
    }
}

fn handle_key(key: KeyEvent, state: &mut UiState, event_tx: &UnboundedSender<UiEvent>) -> bool {
    if key.code == KeyCode::Esc {
        let _ = event_tx.send(UiEvent::ToggleMenu);
        state.mode = Mode::Idle;
        return false;
    }
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        let _ = event_tx.send(UiEvent::Quit);
        return true;
    }

    match &mut state.mode {
        Mode::Input {
            prompt_inline: _,
            buffer,
            respond_to,
        } => match key.code {
            KeyCode::Enter => {
                let text = buffer.clone();
                if let Some(tx) = respond_to.take() {
                    let _ = tx.send(text);
                }
                state.mode = Mode::Idle;
            }
            KeyCode::Backspace => {
                buffer.pop();
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                buffer.push('\n');
            }
            KeyCode::Char(c) => {
                buffer.push(c);
            }
            _ => {}
        },
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
                return false;
            }
            if let KeyCode::Char(c) = key.code {
                let key_str = c.to_string();
                if let Some(idx) = keys.iter().position(|k| k == &key_str) {
                    if let Some(tx) = respond_to.take() {
                        let _ = tx.send(idx);
                    }
                    state.mode = Mode::Idle;
                } else {
                    if let Some(idx) = options.iter().position(|o| o == &key_str) {
                        if let Some(tx) = respond_to.take() {
                            let _ = tx.send(idx);
                        }
                        state.mode = Mode::Idle;
                    }
                }
            }
        }
        Mode::Idle => {}
    }

    false
}

fn draw(terminal: &mut Terminal<CrosstermBackend<Stdout>>, state: &UiState) -> io::Result<()> {
    terminal
        .draw(|f| {
        let size = f.size();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3), Constraint::Length(3)].as_ref())
            .split(size);

        let output = Paragraph::new(Text::from(state.output.clone()))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::NONE));
        f.render_widget(output, chunks[0]);

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
            _ => String::new(),
        };

        let info = Paragraph::new(info_text)
            .style(Style::default().fg(Color::Yellow))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::NONE));
        f.render_widget(info, chunks[1]);

        let prompt_text = match &state.mode {
            Mode::Input { prompt_inline, buffer, .. } => format!("{prompt_inline}{buffer}"),
            _ => String::new(),
        };
        let prompt = Paragraph::new(prompt_text)
            .style(Style::default().fg(Color::Yellow))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::NONE));
        f.render_widget(prompt, chunks[2]);
    })
    .map(|_| ())
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

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

fn cleanup_terminal(term: Option<Terminal<CrosstermBackend<Stdout>>>) {
    if let Some(mut terminal) = term {
        let _ = disable_raw_mode();
        let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
        let _ = terminal.show_cursor();
    }
}
