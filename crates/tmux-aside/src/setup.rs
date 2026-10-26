use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

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

    /// The key offered by default, as written after `bind-key`: `-n` binds it without the prefix.
    pub fn default_key(self) -> &'static str {
        match self {
            Action::Toggle => "b",
            Action::Next => "-n M-w",
            Action::Reload => "R",
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

/// The keys bound inside the marked block of `content`, as written after `bind-key`.
pub fn bound_keys(content: &str) -> Vec<String> {
    content
        .lines()
        .skip_while(|line| line.trim() != BLOCK_START)
        .take_while(|line| line.trim() != BLOCK_END)
        .filter_map(|line| line.strip_prefix("bind-key "))
        .filter_map(|line| line.split_once(" run-shell "))
        .map(|(key, _)| key.to_string())
        .collect()
}

/// Asks for a key per action and the AI hooks to install, writes the bindings to the
/// tmux config and applies them to the running server.
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

    if interactive {
        println!("Key bindings (Enter keeps the default, {UNBOUND} leaves the action unbound)");
    }
    let mut bindings = Vec::new();
    for action in Action::ALL {
        let Ok(program) = action.program() else {
            println!(
                "  {}: skipped, tmux-aside-agents not found",
                action.description()
            );
            continue;
        };
        let given = options
            .keys
            .iter()
            .find(|(given, _)| *given == action)
            .map(|(_, key)| key.clone());
        let key = match given {
            Some(key) => key,
            None if interactive => {
                let key = ask(&format!(
                    "  {} [{}]: ",
                    action.description(),
                    action.default_key()
                ))?;
                if key.is_empty() {
                    action.default_key().to_string()
                } else {
                    key
                }
            }
            None => action.default_key().to_string(),
        };
        if key == UNBOUND {
            continue;
        }
        if live && let Some(existing) = current_binding(&key) {
            println!("  {key} replaces: {existing}");
        }
        let program = program
            .to_str()
            .ok_or_else(|| anyhow!("executable path is not UTF-8"))?;
        bindings.push(Binding {
            key,
            command: format!("{} {}", shell_quote(program), action.arguments()),
        });
    }

    let existing = match std::fs::read_to_string(&path) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
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
        for key in bound_keys(existing.as_deref().unwrap_or(""))
            .into_iter()
            .filter(|key| bindings.iter().all(|binding| &binding.key != key))
        {
            let _ = tmux::run(&[&["unbind-key"], key_arguments(&key).as_slice()].concat());
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

    let ais = match options.ais {
        Some(ais) => ais,
        None if interactive => {
            println!("AI hooks");
            let mut chosen = Vec::new();
            for ai in Ai::ALL {
                let answer = ask(&format!("  {} [Y/n]: ", ai.description()))?;
                if !answer.to_lowercase().starts_with('n') {
                    chosen.push(ai);
                }
            }
            chosen
        }
        None => Ai::ALL.to_vec(),
    };
    for ai in ais {
        for (plugin, argument) in ai.setups() {
            install_hooks(plugin, argument);
        }
    }
    Ok(())
}

fn ask(prompt: &str) -> Result<String> {
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(answer.trim().to_string())
}

fn key_arguments(key: &str) -> Vec<&str> {
    key.split_whitespace().collect()
}

fn current_binding(key: &str) -> Option<String> {
    let arguments = key_arguments(key);
    let table = if arguments.contains(&"-n") {
        "root"
    } else {
        arguments
            .windows(2)
            .find(|pair| pair[0] == "-T")
            .map_or("prefix", |pair| pair[1])
    };
    let existing = tmux::run(&["list-keys", "-T", table, arguments.last()?]).ok()?;
    Some(existing).filter(|existing| !existing.is_empty() && !existing.contains("tmux-aside"))
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
