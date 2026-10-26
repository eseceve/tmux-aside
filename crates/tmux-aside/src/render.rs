use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use ratatui::crossterm::event::{
    self, DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture,
    Event as TermEvent, KeyCode, KeyEventKind, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::layout::{LayoutLine, LayoutOptions, LineKind, has_spinner, help, layout};
use crate::protocol::{ClientMessage, DaemonMessage, Tone};
use crate::view::{Cursor, Key, Theme, View};
use crate::{daemon, sidebar, tmux};

const MIN_BACKOFF: Duration = Duration::from_millis(50);
const MAX_BACKOFF: Duration = Duration::from_secs(2);
const SPINNER_TICK: Duration = Duration::from_millis(100);
const RESIZE_SETTLE: Duration = Duration::from_millis(300);
const RIGHT_PADDING: usize = 1;

enum Event {
    Frame(View),
    Run(Vec<String>, Option<String>),
    Disconnected,
    Input(TermEvent),
}

type Writer = Arc<Mutex<Option<UnixStream>>>;

/// Runs a sidebar client: draws the daemon's shared view and forwards navigation keys to it.
pub fn run(socket: Option<PathBuf>) -> Result<()> {
    let socket = match socket {
        Some(socket) => socket,
        None => daemon::default_socket()?,
    };
    let (events, inbox) = mpsc::channel();
    let writer: Writer = Arc::new(Mutex::new(None));
    {
        let events = events.clone();
        let writer = writer.clone();
        thread::spawn(move || connection_loop(&socket, &writer, &events));
    }
    thread::spawn(move || {
        while let Ok(input) = event::read() {
            if events.send(Event::Input(input)).is_err() {
                return;
            }
        }
    });

    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableFocusChange, EnableMouseCapture);
    let mut view: Option<View> = None;
    let mut focused = false;
    let mut showing_help = false;
    let mut targets: Vec<Option<Cursor>> = Vec::new();
    let host = host_size();
    let current_window = host.as_ref().map(|size| size.window_id.clone());
    let mut window_width = host.map(|size| size.window_width);
    let mut resize_deadline: Option<Instant> = None;
    let result = (|| -> Result<()> {
        loop {
            terminal.draw(|frame| {
                let area = frame.area();
                let options = LayoutOptions {
                    width: usize::from(area.width).saturating_sub(RIGHT_PADDING),
                    height: area.height.into(),
                    spinner_frame: spinner_frame(),
                    current_window: current_window.clone(),
                };
                targets.clear();
                let lines = match &view {
                    Some(view) if showing_help => help(view, options.width)
                        .into_iter()
                        .map(|line| styled(line, &view.theme))
                        .collect(),
                    Some(view) => layout(view, &options)
                        .into_iter()
                        .inspect(|line| targets.push(line.target.clone()))
                        .map(|line| LayoutLine {
                            selected: line.selected && focused,
                            ..line
                        })
                        .map(|line| styled(line, &view.theme))
                        .collect(),
                    None => vec![Line::from("connecting...")],
                };
                frame.render_widget(Paragraph::new(lines), area);
            })?;
            let spinner_wait = view
                .as_ref()
                .is_some_and(has_spinner)
                .then_some(SPINNER_TICK);
            let resize_wait =
                resize_deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
            let event = match spinner_wait.into_iter().chain(resize_wait).min() {
                Some(wait) => match inbox.recv_timeout(wait) {
                    Ok(event) => Some(event),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(error) => return Err(error.into()),
                },
                None => Some(inbox.recv()?),
            };
            if resize_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                resize_deadline = None;
                window_width = settle_resize(window_width);
            }
            let Some(event) = event else { continue };
            match event {
                Event::Input(TermEvent::Resize(..)) => {
                    resize_deadline = Some(Instant::now() + RESIZE_SETTLE);
                }
                Event::Frame(frame) => view = Some(frame),
                Event::Run(command, cwd) => run_action(&command, cwd.as_deref()),
                Event::Disconnected => view = None,
                Event::Input(TermEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                    if showing_help {
                        showing_help = false;
                    } else if key.code == KeyCode::Char('?') {
                        showing_help = true;
                    } else if let Some(message) = key_message(key.code) {
                        send_message(&writer, &message);
                    }
                }
                Event::Input(TermEvent::Mouse(mouse)) => {
                    let message = match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => targets
                            .get(usize::from(mouse.row))
                            .cloned()
                            .flatten()
                            .map(|cursor| ClientMessage::Click { cursor }),
                        MouseEventKind::ScrollDown => Some(ClientMessage::Key { key: Key::Down }),
                        MouseEventKind::ScrollUp => Some(ClientMessage::Key { key: Key::Up }),
                        _ => None,
                    };
                    if let Some(message) = message {
                        showing_help = false;
                        send_message(&writer, &message);
                    }
                }
                Event::Input(TermEvent::FocusGained) => {
                    focused = true;
                    if let Some(window) = &current_window {
                        send_message(
                            &writer,
                            &ClientMessage::Focus {
                                window: window.clone(),
                            },
                        );
                    }
                }
                Event::Input(TermEvent::FocusLost) => focused = false,
                Event::Input(_) => {}
            }
        }
    })();
    let _ = execute!(std::io::stdout(), DisableFocusChange, DisableMouseCapture);
    ratatui::restore();
    result
}

struct HostSize {
    window_id: String,
    window_width: u16,
    pane_width: u16,
}

fn host_size() -> Option<HostSize> {
    let pane = std::env::var("TMUX_PANE").ok()?;
    let output = tmux::run(&[
        "display-message",
        "-p",
        "-t",
        &pane,
        "#{window_id} #{window_width} #{pane_width}",
    ])
    .ok()?;
    let mut fields = output.split(' ');
    Some(HostSize {
        window_id: fields.next()?.to_string(),
        window_width: fields.next()?.parse().ok()?,
        pane_width: fields.next()?.parse().ok()?,
    })
}

/// Treats a pane resize that left the window width unchanged as the user dragging the
/// sidebar, and returns the window width to compare the next resize against.
fn settle_resize(previous_window_width: Option<u16>) -> Option<u16> {
    let Some(size) = host_size() else {
        return previous_window_width;
    };
    let dragged = previous_window_width == Some(size.window_width)
        && sidebar::expected_width(size.window_width)
            .is_ok_and(|expected| expected != size.pane_width);
    if dragged {
        let _ = sidebar::adopt_manual_resize(&size.window_id, size.pane_width);
    }
    Some(size.window_width)
}

fn spinner_frame() -> usize {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    (millis / SPINNER_TICK.as_millis()) as usize
}

fn run_action(command: &[String], cwd: Option<&str>) {
    let Some((program, args)) = command.split_first() else {
        return;
    };
    let mut process = std::process::Command::new(program);
    if let Some(cwd) = cwd {
        process.current_dir(cwd);
    }
    let _ = process
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut child| thread::spawn(move || child.wait()));
}

fn connection_loop(socket: &Path, writer: &Writer, events: &Sender<Event>) {
    let jitter = Duration::from_millis(u64::from(std::process::id() % 50));
    let mut backoff = MIN_BACKOFF;
    loop {
        let stream = UnixStream::connect(socket).or_else(|_| {
            daemon::ensure_running(socket).map_err(std::io::Error::other)?;
            UnixStream::connect(socket)
        });
        if let Ok(stream) = stream {
            backoff = MIN_BACKOFF;
            *writer.lock().unwrap() = stream.try_clone().ok();
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                let event = match serde_json::from_str(&line) {
                    Ok(DaemonMessage::Frame { view }) => Event::Frame(view),
                    Ok(DaemonMessage::Run { command, cwd }) => Event::Run(command, cwd),
                    Err(_) => continue,
                };
                if events.send(event).is_err() {
                    return;
                }
            }
            *writer.lock().unwrap() = None;
            if events.send(Event::Disconnected).is_err() {
                return;
            }
        }
        thread::sleep(backoff + jitter);
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

fn send_message(writer: &Writer, message: &ClientMessage) {
    if let Some(stream) = writer.lock().unwrap().as_mut()
        && let Ok(mut line) = serde_json::to_string(message)
    {
        line.push('\n');
        let _ = stream.write_all(line.as_bytes());
    }
}

/// Keys the sidebar keeps for itself; any other character runs the selected row's action.
const RESERVED: [char; 7] = ['j', 'k', 'h', 'l', 'o', '?', ' '];

fn key_message(code: KeyCode) -> Option<ClientMessage> {
    if let Some(key) = navigation_key(code) {
        return Some(ClientMessage::Key { key });
    }
    match code {
        KeyCode::Char(c) if !RESERVED.contains(&c) => {
            Some(ClientMessage::Action { key: c.to_string() })
        }
        _ => None,
    }
}

fn navigation_key(code: KeyCode) -> Option<Key> {
    match code {
        KeyCode::Char('j') | KeyCode::Down => Some(Key::Down),
        KeyCode::Char('k') | KeyCode::Up => Some(Key::Up),
        KeyCode::Char('h') | KeyCode::Left => Some(Key::Collapse),
        KeyCode::Char('l') | KeyCode::Right => Some(Key::Expand),
        KeyCode::Enter => Some(Key::Activate),
        KeyCode::Char('o') => Some(Key::Open),
        KeyCode::Char(' ') => Some(Key::Toggle),
        _ => None,
    }
}

fn styled(line: LayoutLine, theme: &Theme) -> Line<'static> {
    let mut style = match line.kind {
        LineKind::Separator => {
            Style::new().fg(color(theme, Tone::Muted).unwrap_or(Color::DarkGray))
        }
        LineKind::Header | LineKind::Group | LineKind::Row | LineKind::Blank => Style::new(),
    };
    if line.current {
        style = style.add_modifier(Modifier::BOLD);
        if let Some(current) = parse_color(theme.current.as_deref()) {
            style = style.fg(current);
        }
    }
    if line.selected {
        style = match parse_color(theme.selection.as_deref()) {
            Some(selection) => style.bg(selection),
            None => style.add_modifier(Modifier::REVERSED),
        };
    }
    let spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|span| match color(theme, span.tone) {
            Some(color) => Span::styled(span.text, Style::new().fg(color)),
            None => Span::raw(span.text),
        })
        .collect();
    Line::from(spans).style(style)
}

fn color(theme: &Theme, tone: Tone) -> Option<Color> {
    parse_color(theme.color(tone))
}

fn parse_color(color: Option<&str>) -> Option<Color> {
    color.and_then(|color| color.parse().ok())
}
