//! Example third-party NAVI frontend — **another TUI, in its own process**.
//!
//! This binary is not compiled into NAVI. The compose supervisor activates it as
//! a `kind = ui` component; the host spawns it with the terminal inherited and
//! hands it a control socket:
//!
//! * host → plugin: `{"type":"event", …}` (engine events), `{"type":"quiesce"}`
//!   (the supervisor is swapping this generation out), `{"type":"shutdown"}`;
//! * plugin → host: `{"type":"command","text":…}` (a user message, admitted by
//!   the kernel), `{"type":"quit"}`.
//!
//! It renders with crossterm only (no ratatui) to make the point that a frontend
//! is *any* program that owns the terminal and speaks the contract.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute, queue, style};

const MAX_EVENTS: usize = 12;

struct App {
    session: String,
    events: VecDeque<String>,
    input: String,
    status: String,
    accepted: usize,
    denied: usize,
}

impl App {
    fn new(session: String) -> Self {
        Self {
            session,
            events: VecDeque::new(),
            input: String::new(),
            status: "ready".to_string(),
            accepted: 0,
            denied: 0,
        }
    }

    fn push_event(&mut self, text: String) {
        self.events.push_back(text);
        while self.events.len() > MAX_EVENTS {
            self.events.pop_front();
        }
    }
}

/// Restores the terminal on drop (including on panic).
struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn enter() -> std::io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen, cursor::Hide)?;
        Ok(Self { active: true })
    }

    fn restore(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let mut stdout = std::io::stdout();
        let _ = execute!(stdout, cursor::Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(control) = arg_value(&args, "--control") else {
        eprintln!("usage: navi-plugin-tui --control <socket> [--session <id>]");
        std::process::exit(2);
    };
    let session = arg_value(&args, "--session").unwrap_or_default();

    let stream = match UnixStream::connect(&control) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("navi-plugin-tui: cannot connect to {control}: {err}");
            std::process::exit(1);
        }
    };
    let writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(err) => {
            eprintln!("navi-plugin-tui: cannot clone control stream: {err}");
            std::process::exit(1);
        }
    };
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut guard = match TerminalGuard::enter() {
        Ok(guard) => guard,
        Err(err) => {
            eprintln!("navi-plugin-tui: cannot enter raw mode: {err}");
            std::process::exit(1);
        }
    };
    let mut app = App::new(session);
    app.push_event(format!("plugin pid={}", std::process::id()));
    let mut writer = writer;
    let mut dirty = true;

    loop {
        // Control messages from the host.
        loop {
            match rx.try_recv() {
                Ok(line) => {
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                        continue;
                    };
                    match value.get("type").and_then(|kind| kind.as_str()) {
                        Some("event") => {
                            let text = value
                                .get("event")
                                .and_then(|event| event.as_str())
                                .unwrap_or("")
                                .to_string();
                            app.push_event(text);
                            dirty = true;
                        }
                        Some("accepted") => {
                            app.accepted += 1;
                            app.status = format!(
                                "accepted: {}",
                                value.get("text").and_then(|t| t.as_str()).unwrap_or("")
                            );
                            dirty = true;
                        }
                        Some("denied") => {
                            app.denied += 1;
                            app.status = format!(
                                "denied: {}",
                                value.get("reason").and_then(|r| r.as_str()).unwrap_or("")
                            );
                            dirty = true;
                        }
                        Some("quiesce") | Some("shutdown") => {
                            guard.restore();
                            std::process::exit(0);
                        }
                        _ => {}
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    guard.restore();
                    std::process::exit(0);
                }
            }
        }

        // Keyboard.
        while crossterm::event::poll(Duration::from_millis(0)).unwrap_or(false) {
            match crossterm::event::read() {
                Ok(Event::Key(key))
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    match key.code {
                        KeyCode::Esc => {
                            let _ = writeln!(writer, "{}", serde_json::json!({"type": "quit"}));
                            let _ = writer.flush();
                            guard.restore();
                            std::process::exit(0);
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let _ = writeln!(writer, "{}", serde_json::json!({"type": "quit"}));
                            let _ = writer.flush();
                            guard.restore();
                            std::process::exit(0);
                        }
                        KeyCode::Enter => {
                            let text = app.input.trim().to_string();
                            app.input.clear();
                            if !text.is_empty() {
                                let _ = writeln!(
                                    writer,
                                    "{}",
                                    serde_json::json!({"type": "command", "text": text})
                                );
                                let _ = writer.flush();
                                app.status = "submitted…".to_string();
                            }
                            dirty = true;
                        }
                        KeyCode::Backspace => {
                            app.input.pop();
                            dirty = true;
                        }
                        KeyCode::Char(ch) => {
                            app.input.push(ch);
                            dirty = true;
                        }
                        _ => {}
                    }
                }
                Ok(Event::Resize(_, _)) => dirty = true,
                _ => {}
            }
        }

        if dirty {
            if draw(&app).is_err() {
                guard.restore();
                std::process::exit(1);
            }
            dirty = false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn draw(app: &App) -> std::io::Result<()> {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    // A PTY can report 0×0 (e.g. `script` without a sized terminal): never do
    // arithmetic that could underflow on it.
    let cols = cols.max(20);
    let rows = rows.max(8);
    let mut stdout = std::io::stdout();
    queue!(
        stdout,
        cursor::MoveTo(0, 0),
        terminal_clear_line(),
        style::Print(format!(
            "NAVI plugin TUI — third-party frontend, out of process (pid {})",
            std::process::id()
        )),
    )?;
    queue!(stdout, cursor::MoveToNextLine(1), terminal_clear_line())?;
    queue!(
        stdout,
        style::Print(format!(
            "session: {}   accepted: {}   denied: {}",
            if app.session.is_empty() {
                "<new>"
            } else {
                &app.session
            },
            app.accepted,
            app.denied
        ))
    )?;
    queue!(stdout, cursor::MoveToNextLine(1), terminal_clear_line())?;
    queue!(stdout, style::Print("─".repeat(cols.saturating_sub(1) as usize)))?;

    let body_rows = rows.saturating_sub(6) as usize;
    for index in 0..body_rows {
        queue!(stdout, cursor::MoveToNextLine(1), terminal_clear_line())?;
        if let Some(line) = app.events.get(index) {
            let text: String = line.chars().take(cols.saturating_sub(3) as usize).collect();
            queue!(stdout, style::Print(format!(" {text}")))?;
        }
    }
    queue!(stdout, cursor::MoveToNextLine(1), terminal_clear_line())?;
    queue!(stdout, style::Print("─".repeat(cols.saturating_sub(1) as usize)))?;
    queue!(stdout, cursor::MoveToNextLine(1), terminal_clear_line())?;
    queue!(stdout, style::Print(format!("› {}", app.input)))?;
    queue!(stdout, cursor::MoveToNextLine(1), terminal_clear_line())?;
    queue!(stdout, style::Print(format!(
        "{}   [Enter] send   [Esc] quit   (swap me with: navi components reload navi.ui.tui)",
        app.status
    )))?;
    let cursor_col = (3 + app.input.len() as u16).min(cols.saturating_sub(1));
    queue!(stdout, cursor::MoveTo(cursor_col, rows.saturating_sub(3)))?;
    stdout.flush()
}

/// Clear-to-end-of-line without pulling the whole terminal module into scope.
fn terminal_clear_line() -> crossterm::terminal::Clear {
    crossterm::terminal::Clear(crossterm::terminal::ClearType::UntilNewLine)
}
