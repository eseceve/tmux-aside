use std::fmt;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use inquire::{MultiSelect, Select, Text};

use crate::hooks::{shell_quote, tmux_quote};
use crate::plugin;
use crate::tmux;

const BLOCK_START: &str = "# >>> tmux-aside >>>";
const BLOCK_END: &str = "# <<< tmux-aside <<<";
const UNBOUND: &str = "-";

/// An action `tmux-aside setup` can bind to a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Toggle,
    Next,
    Reload,
}

impl Action {
    /// Every action, in the order they are asked for and written.
    pub const ALL: [Action; 3] = [Action::Toggle, Action::Next, Action::Reload];

    /// The keys offered for the action, as written after `bind-key`. The first is the default.
    pub fn candidates(self) -> &'static [&'static str] {
        match self {
            Action::Toggle => &["B", "S", "e"],
            Action::Next => &["W", "N", "G"],
            Action::Reload => &["R", "O", "Q"],
        }
    }

    fn description(self) -> &'static str {
        match self {
            Action::Toggle => "Toggle the sidebar",
            Action::Next => "Jump to the agent waiting the longest",
            Action::Reload => "Reload the config",
        }
    }

    fn program(self) -> Result<PathBuf> {
        match self {
            Action::Toggle | Action::Reload => Ok(std::env::current_exe()?),
            Action::Next => plugin::resolve("agents"),
        }
    }

    fn arguments(self) -> &'static str {
        match self {
            Action::Toggle => "toggle",
            Action::Next => "next",
            Action::Reload => "reload",
        }
    }
}

/// An AI coding agent whose status hooks `tmux-aside setup` can install.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ai {
    Claude,
}

impl Ai {
    /// Every AI with hooks support, in the order they are offered.
    pub const ALL: [Ai; 1] = [Ai::Claude];

    fn description(self) -> &'static str {
        match self {
            Ai::Claude => "Claude Code: agent status and plan usage",
        }
    }

    fn setups(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Ai::Claude => &[("agents", "claude"), ("usage", "claude")],
        }
    }
}

impl fmt::Display for Ai {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(self.description())
    }
}

/// A key and the shell command it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub key: String,
    pub command: String,
}

/// What `tmux-aside setup` was given on the command line. Anything left out is asked for
/// in a terminal, or takes its default otherwise.
pub struct Options {
    pub config: Option<PathBuf>,
    pub keys: Vec<(Action, String)>,
    pub ais: Option<Vec<Ai>>,
    pub defaults: bool,
}

/// The tmux config tmux itself reads first, or `~/.tmux.conf` when there is none.
pub fn tmux_config_path(home: &Path, xdg_config_home: Option<&Path>) -> PathBuf {
    let classic = home.join(".tmux.conf");
    [
        Some(classic.clone()),
        xdg_config_home.map(|dir| dir.join("tmux/tmux.conf")),
        Some(home.join(".config/tmux/tmux.conf")),
    ]
    .into_iter()
    .flatten()
    .find(|path| path.is_file())
    .unwrap_or(classic)
}

/// The marked block holding `bindings`, or an empty string when there are none.
pub fn render_block(bindings: &[Binding]) -> String {
    if bindings.is_empty() {
        return String::new();
    }
    let mut block = format!("{BLOCK_START}\n");
    for binding in bindings {
        block.push_str(&format!(
            "bind-key {} run-shell {}\n",
            binding.key,
            tmux_quote(&binding.command)
        ));
    }
    block.push_str(&format!("{BLOCK_END}\n"));
    block
}

/// Replaces the marked block in `existing` with `block`, or appends it when there is none.
/// An empty `block` removes it.
pub fn merge_block(existing: &str, block: &str) -> String {
    let lines: Vec<&str> = existing.lines().collect();
    let range = lines
        .iter()
        .position(|line| line.trim() == BLOCK_START)
        .and_then(|start| {
            lines[start..]
                .iter()
                .position(|line| line.trim() == BLOCK_END)
                .map(|end| (start, start + end))
        });
    let (before, after) = match range {
        Some((start, end)) => (&lines[..start], &lines[end + 1..]),
        None => (&lines[..], &[][..]),
    };
    let mut merged: String = before.iter().map(|line| format!("{line}\n")).collect();
    if range.is_none() && !block.is_empty() && before.last().is_some_and(|line| !line.is_empty()) {
        merged.push('\n');
    }
    merged.push_str(block);
    merged.extend(after.iter().map(|line| format!("{line}\n")));
    merged
}

/// The bindings inside the marked block of `content`.
pub fn block_bindings(content: &str) -> Vec<Binding> {
    content
        .lines()
        .skip_while(|line| line.trim() != BLOCK_START)
        .take_while(|line| line.trim() != BLOCK_END)
        .filter_map(|line| line.strip_prefix("bind-key "))
        .filter_map(|line| line.split_once(" run-shell "))
        .map(|(key, command)| Binding {
            key: key.to_string(),
            command: command
                .trim()
                .trim_matches('"')
                .replace("\\\"", "\"")
                .replace("\\\\", "\\"),
        })
        .collect()
}

/// How a person reads `key`, given as written after `bind-key`: `prefix + B`, `M-w (no prefix)`.
pub fn describe_key(key: &str) -> String {
    let arguments = key_arguments(key);
    let name = arguments.last().copied().unwrap_or_default();
    match table(&arguments) {
        "prefix" => format!("prefix + {name}"),
        "root" => format!("{name} (no prefix)"),
        table => format!("{name} in {table}"),
    }
}

/// Asks for a key per action and the AI hooks to install, then writes the bindings to the
/// tmux config, applies them to the running server and installs the hooks. Nothing changes
/// until every question is answered.
pub fn run(options: Options) -> Result<()> {
    let interactive = !options.defaults && std::io::stdin().is_terminal();
    let live = std::env::var_os("TMUX").is_some();
    let path = match options.config {
        Some(path) => path,
        None => tmux_config_path(
            &std::env::home_dir().ok_or_else(|| anyhow!("cannot locate the home directory"))?,
            std::env::var_os("XDG_CONFIG_HOME")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from)
                .as_deref(),
        ),
    };
    let existing = match std::fs::read_to_string(&path) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let previous = block_bindings(existing.as_deref().unwrap_or(""));

    let mut bindings = Vec::new();
    for action in Action::ALL {
        let Ok(program) = action.program() else {
            println!(
                "{}: skipped, tmux-aside-agents not found",
                action.description()
            );
            continue;
        };
        let program = program
            .to_str()
            .ok_or_else(|| anyhow!("executable path is not UTF-8"))?;
        let command = format!("{} {}", shell_quote(program), action.arguments());
        let given = options
            .keys
            .iter()
            .find(|(given, _)| *given == action)
            .map(|(_, key)| key.clone());
        let current = previous
            .iter()
            .find(|binding| {
                binding
                    .command
                    .ends_with(&format!(" {}", action.arguments()))
            })
            .map(|binding| binding.key.clone());
        let key = match given {
            Some(key) => key,
            None if interactive => choose_key(action, current)?,
            None => action.candidates()[0].to_string(),
        };
        if key == UNBOUND {
            continue;
        }
        if !interactive && let KeyStatus::Used(existing) = key_status(&key) {
            println!("{} replaces: {existing}", describe_key(&key));
        }
        bindings.push(Binding { key, command });
    }

    let ais = match options.ais {
        Some(ais) => ais,
        None if interactive => MultiSelect::new("AI hooks to install:", Ai::ALL.to_vec())
            .with_all_selected_by_default()
            .prompt()
            .context("setup canceled; nothing was changed")?,
        None => Ai::ALL.to_vec(),
    };

    if live {
        for binding in &bindings {
            tmux::run(
                &[
                    &["bind-key"],
                    key_arguments(&binding.key).as_slice(),
                    &["run-shell", &binding.command],
                ]
                .concat(),
            )?;
        }
        for binding in previous
            .iter()
            .filter(|old| bindings.iter().all(|binding| binding.key != old.key))
        {
            let _ = tmux::run(&[&["unbind-key"], key_arguments(&binding.key).as_slice()].concat());
        }
    }

    let content = merge_block(existing.as_deref().unwrap_or(""), &render_block(&bindings));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Some(existing) = &existing {
        let mut backup = path.clone().into_os_string();
        backup.push(".bak");
        std::fs::write(backup, existing)?;
    }
    std::fs::write(&path, &content)
        .with_context(|| format!("failed to write {}", path.display()))?;
    println!(
        "{} key bindings written to {}",
        bindings.len(),
        path.display()
    );

    for ai in ais {
        for (plugin, argument) in ai.setups() {
            install_hooks(plugin, argument);
        }
    }
    Ok(())
}

enum KeyStatus {
    Free,
    Ours,
    Used(String),
    Unknown,
}

enum Choice {
    Key { key: String, status: KeyStatus },
    Other,
    Unbound,
}

impl fmt::Display for Choice {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Choice::Key { key, status } => {
                let status = match status {
                    KeyStatus::Free => "free".to_string(),
                    KeyStatus::Ours => "tmux-aside".to_string(),
                    KeyStatus::Used(command) => format!("used: {command}"),
                    KeyStatus::Unknown => String::new(),
                };
                write!(formatter, "{:<18}{status}", describe_key(key))
            }
            Choice::Other => formatter.write_str("Other key..."),
            Choice::Unbound => formatter.write_str("Leave unbound"),
        }
    }
}

fn choose_key(action: Action, current: Option<String>) -> Result<String> {
    let keys: Vec<String> = current
        .iter()
        .cloned()
        .chain(
            action
                .candidates()
                .iter()
                .map(|key| key.to_string())
                .filter(|key| Some(key) != current.as_ref()),
        )
        .collect();
    let mut choices: Vec<Choice> = keys
        .into_iter()
        .map(|key| Choice::Key {
            status: key_status(&key),
            key,
        })
        .collect();
    let start = if current.is_some() {
        0
    } else {
        choices
            .iter()
            .position(|choice| {
                matches!(
                    choice,
                    Choice::Key {
                        status: KeyStatus::Free,
                        ..
                    }
                )
            })
            .unwrap_or(0)
    };
    choices.extend([Choice::Other, Choice::Unbound]);
    let choice = Select::new(&format!("{}:", action.description()), choices)
        .with_starting_cursor(start)
        .prompt()
        .context("setup canceled; nothing was changed")?;
    match choice {
        Choice::Key { key, .. } => Ok(key),
        Choice::Unbound => Ok(UNBOUND.to_string()),
        Choice::Other => {
            let key = Text::new("Key:")
                .with_help_message(
                    "as after bind-key: T for prefix + T, -n M-t for Alt+t without the prefix",
                )
                .prompt()
                .context("setup canceled; nothing was changed")?;
            Ok(key.trim().to_string())
        }
    }
}

fn key_arguments(key: &str) -> Vec<&str> {
    key.split_whitespace().collect()
}

fn table<'a>(arguments: &[&'a str]) -> &'a str {
    if arguments.contains(&"-n") {
        "root"
    } else {
        arguments
            .windows(2)
            .find(|pair| pair[0] == "-T")
            .map_or("prefix", |pair| pair[1])
    }
}

fn key_status(key: &str) -> KeyStatus {
    let arguments = key_arguments(key);
    let Some(name) = arguments.last() else {
        return KeyStatus::Unknown;
    };
    match tmux::run(&["list-keys", "-T", table(&arguments), name]) {
        Ok(line) if line.contains("tmux-aside") => KeyStatus::Ours,
        Ok(line) if !line.is_empty() => {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            let command = tokens
                .iter()
                .position(|token| *token == "-T")
                .map(|index| tokens[(index + 3).min(tokens.len())..].join(" "))
                .unwrap_or(line);
            KeyStatus::Used(command)
        }
        Ok(_) => KeyStatus::Free,
        Err(error) if error.to_string().contains("unknown key") => KeyStatus::Free,
        Err(_) => KeyStatus::Unknown,
    }
}

fn install_hooks(plugin: &str, argument: &str) {
    let Ok(program) = plugin::resolve(plugin) else {
        eprintln!("tmux-aside-{plugin} not found; skipped its {argument} hooks");
        return;
    };
    let status = Command::new(program).args(["setup", argument]).status();
    if !status.is_ok_and(|status| status.success()) {
        eprintln!("tmux-aside-{plugin} setup {argument} failed; the rest of the setup is done");
    }
}
