use std::fmt;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use inquire::{Confirm, MultiSelect, Select, Text};
use serde::{Deserialize, Serialize};

use crate::hooks::{shell_quote, tmux_quote};
use crate::i18n::Language;
use crate::plugin;
use crate::tmux;
use crate::{config, pack};

const BLOCK_START: &str = "# >>> tmux-aside >>>";
/// The skills tmux-aside installs for each AI that reads skills, as folder name and text.
pub const SKILLS: [(&str, &str); 2] = [
    (
        "tmux-aside-launch",
        include_str!("../skills/tmux-aside-launch/SKILL.md"),
    ),
    (
        "tmux-aside-qa",
        include_str!("../skills/tmux-aside-qa/SKILL.md"),
    ),
];
const BLOCK_END: &str = "# <<< tmux-aside <<<";
const UNBOUND: &str = "-";

/// An action `tmux-aside setup` can bind to a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Toggle,
    Focus,
    Next,
    Reload,
}

impl Action {
    /// Every action, in the order they are asked for and written.
    pub const ALL: [Action; 4] = [Action::Toggle, Action::Focus, Action::Next, Action::Reload];

    /// The keys offered for the action, as written after `bind-key`. The first is the default.
    pub fn candidates(self) -> &'static [&'static str] {
        match self {
            Action::Toggle => &["B", "S", "e"],
            Action::Focus => &["Tab", "A", "F"],
            Action::Next => &["W", "N", "G"],
            Action::Reload => &["R", "O", "Q"],
        }
    }

    fn description(self) -> &'static str {
        match self {
            Action::Toggle => "Toggle the sidebar",
            Action::Focus => "Focus the sidebar",
            Action::Next => "Jump to the agent waiting (or done) the longest",
            Action::Reload => "Reload the config",
        }
    }

    fn program(self) -> Result<PathBuf> {
        match self {
            Action::Toggle | Action::Focus | Action::Reload => Ok(std::env::current_exe()?),
            Action::Next => plugin::resolve("agents", Language::En),
        }
    }

    fn arguments(self) -> &'static str {
        match self {
            Action::Toggle => "toggle",
            Action::Focus => "focus",
            Action::Next => "next",
            Action::Reload => "reload",
        }
    }
}

/// An AI coding agent whose status hooks `tmux-aside setup` can install, and that
/// `tmux-aside launch` can start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ai {
    Claude,
    Codex,
    Agy,
}

impl Ai {
    /// Every AI with hooks support, in the order they are offered.
    pub const ALL: [Ai; 3] = [Ai::Claude, Ai::Codex, Ai::Agy];

    fn name(self) -> &'static str {
        match self {
            Ai::Claude => "Claude Code",
            Ai::Codex => "Codex",
            Ai::Agy => "Antigravity",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Ai::Claude => "Claude Code: agent status and plan usage",
            Ai::Codex => "Codex: agent status",
            Ai::Agy => "Antigravity: agent status",
        }
    }

    /// The AI whose command is `name`, if tmux-aside supports it.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|ai| ai.program() == name)
    }

    /// The arguments that start the AI's interactive session with `prompt` as its first
    /// message.
    pub fn start_arguments(self, prompt: &str) -> Vec<String> {
        match self {
            Ai::Claude | Ai::Codex => vec![prompt.to_string()],
            Ai::Agy => vec!["--prompt-interactive".to_string(), prompt.to_string()],
        }
    }

    /// The arguments that resume the AI's latest conversation in the current directory.
    pub fn resume_arguments(self) -> Vec<String> {
        match self {
            Ai::Claude | Ai::Agy => vec!["--continue".to_string()],
            Ai::Codex => vec!["resume".to_string(), "--last".to_string()],
        }
    }

    /// The arguments that run the AI headless on `prompt` and exit once it answered.
    pub fn print_arguments(self, prompt: &str) -> Vec<String> {
        let flag = match self {
            Ai::Claude => "-p",
            Ai::Codex => "exec",
            Ai::Agy => "--print",
        };
        vec![flag.to_string(), prompt.to_string()]
    }

    /// The folder the AI reads its skills from, or `None` when tmux-aside installs none for
    /// it: `$CLAUDE_CONFIG_DIR/skills` (`~/.claude/skills`) and `$CODEX_HOME/skills`
    /// (`~/.codex/skills`).
    pub fn skills_dir(self) -> Option<PathBuf> {
        let (variable, fallback) = match self {
            Ai::Claude => ("CLAUDE_CONFIG_DIR", ".claude"),
            Ai::Codex => ("CODEX_HOME", ".codex"),
            Ai::Agy => return None,
        };
        let home = std::env::var_os(variable)
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::home_dir().map(|home| home.join(fallback)))?;
        Some(home.join("skills"))
    }

    /// The command that starts the AI.
    pub fn program(self) -> &'static str {
        match self {
            Ai::Claude => "claude",
            Ai::Codex => "codex",
            Ai::Agy => "agy",
        }
    }

    fn setups(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Ai::Claude => &[("agents", "claude"), ("usage", "claude")],
            Ai::Codex => &[("agents", "codex")],
            Ai::Agy => &[("agents", "agy")],
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
    /// A Pack's path or git URL.
    pub pack: Option<String>,
    /// Whether to install the skills for each chosen AI; asked when unset.
    pub skills: Option<bool>,
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

/// Reads a key written as `bind-key` takes it (`r`, `-n M-w`) or as [`describe_key`] shows it
/// (`prefix + r`, `M-w (no prefix)`, `y in copy-mode-vi`), and returns the `bind-key` form.
pub fn parse_key(input: &str) -> String {
    let input = input.trim();
    if let Some((table, name)) = input.split_once(" + ")
        && table.trim().eq_ignore_ascii_case("prefix")
    {
        return name.trim().to_string();
    }
    if let Some(name) = input.strip_suffix("(no prefix)") {
        return format!("-n {}", name.trim());
    }
    if let Some((name, table)) = input.split_once(" in ")
        && !name.trim().starts_with('-')
    {
        return format!("-T {} {}", table.trim(), name.trim());
    }
    input.to_string()
}

/// Asks for a key per action and the AI hooks to install, then writes the bindings to the
/// tmux config, applies them to the running server and installs the hooks. Nothing changes
/// until every question is answered.
pub fn run(options: Options) -> Result<()> {
    let interactive = !options.defaults && std::io::stdin().is_terminal();
    let live = std::env::var_os("TMUX").is_some();
    let path = match options.config {
        Some(path) => path,
        None => default_tmux_config()?,
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
            .map(|(_, key)| parse_key(key));
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

    let skills: Vec<Ai> = ais
        .iter()
        .copied()
        .filter(|ai| ai.skills_dir().is_some())
        .map(|ai| -> Result<Option<Ai>> {
            let install = match options.skills {
                Some(install) => install,
                None if interactive => Confirm::new(&format!(
                    "Install the /tmux-aside-launch and /tmux-aside-qa skills for {}?",
                    ai.name()
                ))
                .with_default(true)
                .prompt()
                .context("setup canceled; nothing was changed")?,
                None => true,
            };
            Ok(install.then_some(ai))
        })
        .filter_map(Result::transpose)
        .collect::<Result<_>>()?;

    let pack = match options.pack {
        Some(pack) => Some(pack),
        None if interactive => Some(
            Text::new("Pack (path or git URL, empty for none):")
                .with_help_message("a private bundle of Criteria and Templates for your team")
                .prompt()
                .context("setup canceled; nothing was changed")?,
        )
        .filter(|pack| !pack.trim().is_empty()),
        None => None,
    };
    let pack = pack
        .map(|source| pack::install(source.trim(), &pack::packs_dir()?))
        .transpose()?;

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

    for ai in &ais {
        for (plugin, argument) in ai.setups() {
            install_hooks(plugin, argument);
        }
    }

    if options.skills != Some(false) && ais.contains(&Ai::Agy) {
        println!("Antigravity: skills are not installed for it");
    }
    if options.skills == Some(true) && skills.is_empty() {
        println!("No AI chosen, so no skills were installed; choose them with --hooks");
    }
    for ai in skills {
        let dir = ai.skills_dir().context("no skills folder")?;
        for (name, text) in SKILLS {
            let folder = dir.join(name);
            if folder.exists() {
                std::fs::remove_dir_all(&folder)
                    .with_context(|| format!("failed to replace {}", folder.display()))?;
            }
            std::fs::create_dir_all(&folder)?;
            std::fs::write(folder.join("SKILL.md"), text)
                .with_context(|| format!("failed to write {}", folder.display()))?;
        }
        println!("{}: skills installed in {}", ai.name(), dir.display());
    }

    if let Some(dir) = pack {
        let config = config::default_path()?;
        let existing = match std::fs::read_to_string(&config) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", config.display()));
            }
        };
        if let Some(parent) = config.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if !existing.is_empty() {
            let mut backup = config.clone().into_os_string();
            backup.push(".bak");
            std::fs::write(backup, &existing)?;
        }
        std::fs::write(&config, pack::with_pack_key(&existing, Some(&dir)))
            .with_context(|| format!("failed to write {}", config.display()))?;
        println!("pack {} set in {}", dir.display(), config.display());
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
                    "prefix + T, or as after bind-key: T for prefix + T, -n M-t for Alt+t without the prefix",
                )
                .prompt()
                .context("setup canceled; nothing was changed")?;
            Ok(parse_key(&key))
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

fn default_tmux_config() -> Result<PathBuf> {
    Ok(tmux_config_path(
        &std::env::home_dir().ok_or_else(|| anyhow!("cannot locate the home directory"))?,
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .as_deref(),
    ))
}

/// What `tmux-aside uninstall` was given on the command line.
pub struct UninstallOptions {
    /// The tmux config setup wrote to; the one tmux reads when unset.
    pub config: Option<PathBuf>,
    /// Delete a Pack tmux-aside cloned; asked in a terminal when unset, kept otherwise.
    pub delete_pack: Option<bool>,
    pub defaults: bool,
}

/// Undoes what `run` added: the marked block of the tmux config (and its keys, inside tmux),
/// the AI hooks, the skills and the `pack` key, leaving everything else as it is. A Pack
/// cloned by setup is deleted only when the user agrees; one given by path never is. Without
/// a previous setup it changes nothing.
pub fn uninstall(options: UninstallOptions) -> Result<()> {
    let interactive = !options.defaults && std::io::stdin().is_terminal();
    let config_path = config::default_path()?;
    let pack_dir = config::load(&config_path)
        .unwrap_or_else(|error| {
            eprintln!("{error:#}");
            None
        })
        .and_then(|config| config.pack)
        .map(|dir| match config_path.parent() {
            Some(folder) => folder.join(dir),
            None => dir,
        });
    let packs = pack::packs_dir()?.canonicalize().ok();
    let cloned = pack_dir
        .and_then(|dir| dir.canonicalize().ok())
        .filter(|dir| packs.is_some() && dir.parent() == packs.as_deref());
    let delete_pack = match (&cloned, options.delete_pack) {
        (None, _) => false,
        (Some(_), Some(delete)) => delete,
        (Some(dir), None) if interactive => Confirm::new(&format!(
            "Delete the Pack tmux-aside cloned into {}?",
            dir.display()
        ))
        .with_default(false)
        .prompt()
        .context("uninstall canceled; nothing was changed")?,
        (Some(_), None) => false,
    };

    let path = match options.config {
        Some(path) => path,
        None => default_tmux_config()?,
    };
    if let Ok(content) = std::fs::read_to_string(&path) {
        let bindings = block_bindings(&content);
        if content.lines().any(|line| line.trim() == BLOCK_START) {
            if std::env::var_os("TMUX").is_some() {
                for binding in &bindings {
                    let _ = tmux::run(
                        &[&["unbind-key"], key_arguments(&binding.key).as_slice()].concat(),
                    );
                }
            }
            rewrite(&path, &content, &merge_block(&content, ""))?;
            println!("key bindings removed from {}", path.display());
        }
    }

    let mut failed = Vec::new();
    for ai in Ai::ALL {
        for (plugin, argument) in ai.setups() {
            let Ok(program) = plugin::resolve(plugin, Language::En) else {
                continue;
            };
            let removed = Command::new(program)
                .args(["uninstall", argument])
                .status()
                .is_ok_and(|status| status.success());
            if !removed {
                failed.push(format!("tmux-aside-{plugin} uninstall {argument}"));
            }
        }
        let Some(dir) = ai.skills_dir() else {
            continue;
        };
        for (name, _) in SKILLS {
            let folder = dir.join(name);
            if folder.exists() {
                std::fs::remove_dir_all(&folder)
                    .with_context(|| format!("failed to remove {}", folder.display()))?;
                println!("{}: skill {name} removed", ai.name());
            }
        }
    }

    if let Ok(content) = std::fs::read_to_string(&config_path) {
        let without = pack::with_pack_key(&content, None);
        if without != content {
            rewrite(&config_path, &content, &without)?;
            println!("pack removed from {}", config_path.display());
        }
    }
    match (cloned, delete_pack) {
        (Some(dir), true) => {
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("failed to delete {}", dir.display()))?;
            println!("pack {} deleted", dir.display());
        }
        (Some(dir), false) => println!("pack {} kept", dir.display()),
        (None, _) => {}
    }
    if !failed.is_empty() {
        bail!(
            "these hooks may still be installed; run them by hand: {}",
            failed.join(", ")
        );
    }
    Ok(())
}

/// Replaces `content` of `path` with `changed`, without the blank lines it ends with; a file
/// left empty is deleted. It keeps a backup, unless one is already there, such as the one setup
/// left with the file as it was before.
fn rewrite(path: &Path, content: &str, changed: &str) -> Result<()> {
    let mut backup = path.to_path_buf().into_os_string();
    backup.push(".bak");
    if !Path::new(&backup).exists() {
        std::fs::write(backup, content)?;
    }
    let changed = changed.trim_end();
    if changed.is_empty() {
        std::fs::remove_file(path)
    } else {
        std::fs::write(path, format!("{changed}\n"))
    }
    .with_context(|| format!("failed to write {}", path.display()))
}

fn install_hooks(plugin: &str, argument: &str) {
    let Ok(program) = plugin::resolve(plugin, Language::En) else {
        eprintln!("tmux-aside-{plugin} not found; skipped its {argument} hooks");
        return;
    };
    let status = Command::new(program).args(["setup", argument]).status();
    if !status.is_ok_and(|status| status.success()) {
        eprintln!("tmux-aside-{plugin} setup {argument} failed; the rest of the setup is done");
    }
}
