use std::collections::HashMap;
use std::fs::{File, TryLockError};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, anyhow};

use crate::config::{self, EnterAction, SectionConfig};
use crate::folds;
use crate::i18n::Language;
use crate::plugin::{self, Update};
use crate::protocol::{Action, ClientMessage, DaemonMessage, Row};
use crate::title;
use crate::view::{Key, Section, View};

const TICK: Duration = Duration::from_millis(100);
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);
const LOCK_WAIT: Duration = Duration::from_secs(1);
const CONFIG_POLL: Duration = Duration::from_secs(1);

/// Socket of the daemon serving the tmux server named by `$TMUX`.
pub fn default_socket() -> Result<PathBuf> {
    let tmux_socket = tmux_socket()?;
    let mut path = tmux_socket.into_os_string();
    path.push("-aside.sock");
    Ok(path.into())
}

fn tmux_socket() -> Result<PathBuf> {
    let value = std::env::var("TMUX").context("TMUX is not set: run inside tmux")?;
    let path = value
        .split(',')
        .next()
        .filter(|path| !path.is_empty())
        .ok_or_else(|| anyhow!("TMUX has no socket path: {value}"))?;
    Ok(PathBuf::from(path))
}

/// Starts the daemon in the background unless one already answers on `socket`.
pub fn ensure_running(socket: &Path) -> Result<()> {
    if UnixStream::connect(socket).is_ok() {
        return Ok(());
    }
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .arg("_daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command.spawn().context("failed to spawn daemon")?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while UnixStream::connect(socket).is_err() {
        if Instant::now() > deadline {
            return Err(anyhow!("daemon did not start on {}", socket.display()));
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

enum Event {
    Connected(usize, UnixStream),
    Message(usize, ClientMessage),
    Disconnected(usize),
    /// A plugin update, tagged with the generation of plugins that produced it.
    Section(usize, Update),
}

/// The plugins started for one load of the config.
struct Plugins {
    generation: usize,
    stop: Arc<AtomicBool>,
    supervisors: Vec<JoinHandle<()>>,
}

/// The view and plugin settings read from the config.
struct Loaded {
    view: View,
    sections: Vec<SectionConfig>,
    auto_reload: bool,
    enter: EnterAction,
}

/// Serves the shared view on `socket` until no client has been connected for `idle_timeout`
/// or the socket is removed.
///
/// Exits immediately when another daemon already owns `socket`.
pub fn run(socket: &Path, idle_timeout: Duration) -> Result<()> {
    let mut lock_path = socket.as_os_str().to_owned();
    lock_path.push(".lock");
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("failed to open {}", lock_path.display()))?;
    if !acquire(&lock)? {
        return Ok(());
    }

    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)
        .with_context(|| format!("failed to bind {}", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;

    let (events, inbox) = mpsc::channel();
    let loaded = load();
    let mut view = loaded.view;
    let mut configs = loaded.sections.clone();
    let mut auto_reload = loaded.auto_reload;
    let mut enter = loaded.enter;
    let mut plugins = start_plugins(loaded.sections, view.language, &events, 0);
    let mut retired = Vec::new();
    let config_path = config::default_path().ok();
    let mut config_modified = modified(config_path.as_deref());
    let mut config_checked = Instant::now();
    let folds_path = folds::path();
    let mut saved_folds = folds::of(&view, &configs);
    {
        let events = events.clone();
        thread::spawn(move || accept_loop(listener, events));
    }

    let mut clients: HashMap<usize, UnixStream> = HashMap::new();
    let mut idle_since = Some(Instant::now());
    let mut reload_now = false;
    loop {
        match inbox.recv_timeout(TICK) {
            Ok(Event::Connected(id, mut stream)) => {
                if send(&mut stream, &view).is_ok() {
                    clients.insert(id, stream);
                    idle_since = None;
                }
            }
            Ok(Event::Message(id, ClientMessage::Key { key: Key::Activate })) => {
                let window = view.selected_row().and_then(|row| row.window.clone());
                match view.activate() {
                    Some(action) => send_run(clients.get_mut(&id), select(enter, window, action)),
                    None => clients.retain(|_, stream| send(stream, &view).is_ok()),
                }
            }
            Ok(Event::Message(id, ClientMessage::Click { cursor })) => {
                if view.select(cursor) {
                    match view.activate() {
                        Some(action) => send_run(clients.get_mut(&id), action),
                        None => clients.retain(|_, stream| send(stream, &view).is_ok()),
                    }
                }
            }
            Ok(Event::Message(id, ClientMessage::Key { key: Key::Open })) => {
                if let Some(action) = view.selected_action("enter") {
                    send_run(clients.get_mut(&id), action);
                }
            }
            Ok(Event::Message(id, ClientMessage::Action { key })) => {
                if let Some(action) = view.selected_action(&key) {
                    send_run(clients.get_mut(&id), action);
                }
            }
            Ok(Event::Message(_, ClientMessage::Focus { window })) => {
                view.follow_window(&window);
                clients.retain(|_, stream| send(stream, &view).is_ok());
            }
            Ok(Event::Message(_, ClientMessage::Key { key })) => {
                view.apply(key);
                clients.retain(|_, stream| send(stream, &view).is_ok());
            }
            Ok(Event::Disconnected(id)) => {
                clients.remove(&id);
            }
            Ok(Event::Message(_, ClientMessage::Reload)) => {
                reload_now = true;
            }
            Ok(Event::Section(generation, update)) if generation == plugins.generation => {
                view.set_section(update.section, &update.title, update.rows);
                if let Some(section) = view.sections.get_mut(update.section) {
                    section.summary = update.summary;
                }
                clients.retain(|_, stream| send(stream, &view).is_ok());
            }
            Ok(Event::Section(..)) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let current_folds = folds::of(&view, &configs);
        if current_folds != saved_folds {
            if let Some(path) = &folds_path {
                let _ = folds::write(path, &current_folds);
            }
            saved_folds = current_folds;
        }
        if auto_reload && config_checked.elapsed() >= CONFIG_POLL {
            config_checked = Instant::now();
            let current = modified(config_path.as_deref());
            reload_now |= current != config_modified;
        }
        if reload_now {
            reload_now = false;
            config_modified = modified(config_path.as_deref());
            let loaded = load();
            auto_reload = loaded.auto_reload;
            enter = loaded.enter;
            view = merge(view, &configs, loaded.view, &loaded.sections);
            configs = loaded.sections.clone();
            saved_folds = folds::of(&view, &configs);
            let generation = plugins.generation + 1;
            plugins.stop.store(true, Ordering::Relaxed);
            retired.append(&mut plugins.supervisors);
            plugins = start_plugins(loaded.sections, view.language, &events, generation);
            clients.retain(|_, stream| send(stream, &view).is_ok());
        }
        if clients.is_empty() {
            let since = *idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= idle_timeout {
                break;
            }
        }
        if !socket.exists() {
            break;
        }
    }
    plugins.stop.store(true, Ordering::Relaxed);
    for supervisor in plugins.supervisors.into_iter().chain(retired) {
        let _ = supervisor.join();
    }
    let _ = std::fs::remove_file(socket);
    Ok(())
}

/// Asks the daemon serving `socket` to reread the config. Returns false when none is running.
pub fn reload(socket: &Path) -> Result<bool> {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return Ok(false);
    };
    let mut line = serde_json::to_string(&ClientMessage::Reload)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    Ok(true)
}

/// Stops the daemon serving `socket` by removing its socket; it exits on its next tick.
pub fn stop(socket: &Path) {
    let _ = std::fs::remove_file(socket);
}

fn acquire(lock: &File) -> Result<bool> {
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(true),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => thread::sleep(TICK),
            Err(TryLockError::WouldBlock) => return Ok(false),
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

fn accept_loop(listener: UnixListener, events: Sender<Event>) {
    for (id, stream) in listener.incoming().flatten().enumerate() {
        let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
        let Ok(writer) = stream.try_clone() else {
            continue;
        };
        if events.send(Event::Connected(id, writer)).is_err() {
            return;
        }
        let events = events.clone();
        thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                if let Ok(message) = serde_json::from_str(&line) {
                    let _ = events.send(Event::Message(id, message));
                }
            }
            let _ = events.send(Event::Disconnected(id));
        });
    }
}

fn send(stream: &mut UnixStream, view: &View) -> std::io::Result<()> {
    write_message(stream, &DaemonMessage::Frame { view: view.clone() })
}

/// In `select` mode, turns the Enter action of a row in `window` into a jump to that window's
/// sidebar, which runs the original action itself when the window has none.
fn select(enter: EnterAction, window: Option<String>, action: Action) -> Action {
    let (EnterAction::Select, Some(window), Ok(exe)) = (enter, window, std::env::current_exe())
    else {
        return action;
    };
    let mut command = vec![
        exe.to_string_lossy().into_owned(),
        "_select".to_string(),
        "--window".to_string(),
        window,
        "--".to_string(),
    ];
    command.extend(action.command);
    Action { command, ..action }
}

fn send_run(client: Option<&mut UnixStream>, action: Action) {
    if let Some(stream) = client {
        let _ = write_message(
            stream,
            &DaemonMessage::Run {
                command: action.command,
                cwd: action.cwd,
            },
        );
    }
}

fn write_message(stream: &mut UnixStream, message: &DaemonMessage) -> std::io::Result<()> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    stream.write_all(line.as_bytes())
}

fn modified(path: Option<&Path>) -> Option<SystemTime> {
    std::fs::metadata(path?)
        .and_then(|meta| meta.modified())
        .ok()
}

/// Builds the view of a reloaded config, keeping the content, folds and scroll of every
/// section whose settings did not change, and the selection when its line still exists.
fn merge(
    old: View,
    old_configs: &[SectionConfig],
    mut new: View,
    configs: &[SectionConfig],
) -> View {
    let selected = old.selected.clone();
    let mut old_sections = old.sections.into_iter().map(Some).collect::<Vec<_>>();
    for (index, config) in configs.iter().enumerate() {
        if old_configs.get(index) == Some(config)
            && let Some(section) = old_sections.get_mut(index).and_then(Option::take)
        {
            new.sections[index] = section;
        }
    }
    new.restore_selection(selected);
    new
}

fn load() -> Loaded {
    let notice = |text: String, language: Language| Loaded {
        view: View {
            language,
            ..View::new(vec![Section::new("tmux-aside", vec![Row::new(text)], None)])
        },
        sections: vec![],
        auto_reload: false,
        enter: EnterAction::Focus,
    };
    let fallback = Language::from_env();
    let messages = fallback.messages();
    let path = match config::default_path() {
        Ok(path) => path,
        Err(error) => return notice(format!("! {error:#}"), fallback),
    };
    match config::load(&path) {
        Ok(Some(mut config)) if !config.sections.is_empty() => {
            let language = config.language.unwrap_or(fallback);
            config.sections.sort_by_key(|section| section.pin);
            let sections = config
                .sections
                .iter()
                .map(|section| Section {
                    header: section.header,
                    counts: section.counts,
                    group_gap: section.group_gap,
                    density: section.density,
                    collapsed: section.collapsed && section.header,
                    pin: section.pin,
                    ..Section::new(
                        &section
                            .title
                            .as_deref()
                            .map(|title| title::expand(title, language))
                            .unwrap_or_else(|| plugin::display_name(&section.plugin)),
                        vec![Row::new(language.messages().loading)],
                        section.max_height,
                    )
                })
                .collect();
            let mut view = View::new(sections);
            view.theme = config.theme;
            view.language = language;
            if let Some(path) = folds::path() {
                folds::apply(&mut view, &config.sections, &folds::read(&path));
                view.restore_selection(view.selected.clone());
            }
            Loaded {
                view,
                sections: config.sections,
                auto_reload: config.auto_reload,
                enter: config.enter,
            }
        }
        Ok(Some(config)) => {
            let language = config.language.unwrap_or(fallback);
            Loaded {
                auto_reload: config.auto_reload,
                ..notice(
                    (language.messages().add_sections)(&path.display().to_string()),
                    language,
                )
            }
        }
        Ok(None) => notice((messages.create)(&path.display().to_string()), fallback),
        Err(error) => notice(format!("! {error:#}"), fallback),
    }
}

fn start_plugins(
    sections: Vec<SectionConfig>,
    language: Language,
    events: &Sender<Event>,
    generation: usize,
) -> Plugins {
    let stop = Arc::new(AtomicBool::new(false));
    let logs = config::state_dir().and_then(|dir| {
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    });
    let supervisors = sections
        .into_iter()
        .enumerate()
        .map(|(index, section)| {
            let log = match &logs {
                Ok(dir) => dir.join(format!(
                    "{index}-{}.log",
                    plugin::display_name(&section.plugin)
                )),
                Err(_) => PathBuf::from("/dev/null"),
            };
            let events = events.clone();
            plugin::spawn(index, section, log, language, stop.clone(), move |update| {
                let _ = events.send(Event::Section(generation, update));
            })
        })
        .collect();
    Plugins {
        generation,
        stop,
        supervisors,
    }
}
