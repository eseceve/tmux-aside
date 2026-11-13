use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use crate::i18n::Language;
use crate::pack;
pub use crate::view::{Density, Pin, Theme};

const DEFAULT_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// How the daemon runs a section's plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Spawned every `interval`; prints one snapshot and exits.
    Interval,
    /// Spawned once and kept alive; prints a snapshot line whenever its content changes.
    ///
    /// `timeout` only bounds the wait for the plugin's hello. The plugin must exit when its
    /// stdin closes, and it is restarted with backoff whenever it exits.
    Stream,
}

/// What Enter does on a row that belongs to a tmux window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnterAction {
    /// Run the row's action, such as jumping to an agent's pane.
    #[default]
    Focus,
    /// Show the row's window and focus its sidebar, so navigation can go on; `o` runs the
    /// row's action.
    Select,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionConfig {
    pub plugin: String,
    pub args: Vec<String>,
    pub mode: Mode,
    pub interval: Duration,
    pub timeout: Duration,
    pub max_height: Option<usize>,
    pub header: bool,
    /// Replaces the plugin's title; `${today}` expands to the local date, `${today:es}` in Spanish.
    pub title: Option<String>,
    /// Shows how many rows the section and each of its groups hold.
    pub counts: bool,
    /// Leaves a blank line between consecutive groups.
    pub group_gap: bool,
    /// Space between rows: `compact` (none) or `comfortable` (a blank line before each row).
    pub density: Density,
    /// Starts folded unless the user unfolded it before.
    pub collapsed: bool,
    pub pin: Pin,
    /// Runs the plugin without showing the section, for plugins that act rather than show,
    /// such as the watcher's Nudges and cleanup.
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub sections: Vec<SectionConfig>,
    pub theme: Theme,
    /// Reload the config whenever the file changes, without `tmux-aside reload`.
    pub auto_reload: bool,
    pub enter: EnterAction,
    /// Language of the sidebar and the bundled plugins; the locale's when unset.
    pub language: Option<Language>,
    /// Commands replacing the Watcher's default Criteria, per stage.
    pub criteria: Criteria,
    /// The Watcher launches a Group's QA by itself once all its pull requests reached QA.
    pub auto_qa: bool,
    /// The AI launched agents start (`claude`, `codex` or `agy`), unless their Template names
    /// one; Claude Code when unset.
    pub ai: Option<String>,
    /// The folder of the Pack whose Criteria and Templates replace the defaults.
    pub pack: Option<PathBuf>,
    /// Where the Watcher finds pull requests to follow without a Launch.
    pub discover: Vec<DiscoverySource>,
    /// What the Watcher does once pull requests are deployed.
    pub on_deployed: Option<DeployedAction>,
}

/// An AI the Watcher starts with a Template once pull requests are deployed, once for all
/// the pull requests deployed in the same fetch.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeployedAction {
    /// The Template the AI is started with; `deployed` when unset.
    #[serde(default = "default_deployed_template")]
    pub template: String,
    /// Open a window with an interactive agent instead of running the AI headless.
    #[serde(default)]
    pub window: bool,
}

impl Default for DeployedAction {
    fn default() -> Self {
        DeployedAction {
            template: default_deployed_template(),
            window: false,
        }
    }
}

fn default_deployed_template() -> String {
    "deployed".to_string()
}

/// A source of pull requests the Watcher follows by itself.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum DiscoverySource {
    /// The pull requests a GitHub search finds, up to 50.
    Search { search: String },
    /// The pull request URLs a command prints, one per line.
    Command {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

/// The Criterion of each stage of the Lifecycle that replaces the default one, if any.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Criteria {
    /// Whether the pull request meets its merge criteria.
    pub ready_to_merge: Option<CriterionCommand>,
    /// Whether the merged change can be verified in a pre-production environment.
    pub qa: Option<CriterionCommand>,
    /// Whether QA passed.
    pub ready_to_deploy: Option<CriterionCommand>,
    /// Whether the change reached production.
    pub deployed: Option<CriterionCommand>,
}

/// An executable answering whether a pull request reached a stage; see `docs/criteria.md`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CriterionCommand {
    /// Path to the executable; `~` is not expanded. The stage's default Criterion, run with
    /// `args`, when unset.
    pub command: Option<String>,
    /// Arguments passed to it.
    #[serde(default)]
    pub args: Vec<String>,
    /// How long it may run, such as `10s`.
    #[serde(default = "default_criterion_timeout", deserialize_with = "duration")]
    pub timeout: Duration,
}

fn default_criterion_timeout() -> Duration {
    DEFAULT_TIMEOUT
}

fn duration<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
    let value = String::deserialize(deserializer)?;
    parse_duration(&value).map_err(serde::de::Error::custom)
}

/// A config file or a Pack's manifest as written, before defaults: a key that is absent is
/// `None`, so that a config's `false` wins over a Pack's `true`.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfig {
    pub(crate) sections: Option<Vec<RawSection>>,
    pub(crate) theme: Option<RawTheme>,
    pub(crate) auto_reload: Option<bool>,
    pub(crate) enter: Option<EnterAction>,
    pub(crate) language: Option<Language>,
    #[serde(default)]
    pub(crate) criteria: Criteria,
    pub(crate) auto_qa: Option<bool>,
    pub(crate) ai: Option<String>,
    pub(crate) pack: Option<PathBuf>,
    pub(crate) discover: Option<Vec<DiscoverySource>>,
    pub(crate) on_deployed: Option<DeployedAction>,
    pub(crate) templates: Option<String>,
}

impl RawConfig {
    /// Parses a config file or a Pack's manifest.
    pub(crate) fn parse(yaml: &str) -> Result<Self> {
        Ok(serde_saphyr::from_str::<Option<RawConfig>>(yaml)
            .map_err(|error| anyhow!("{error}"))?
            .unwrap_or_default())
    }

    /// This config with what it leaves out taken from `pack`: each key from the first that sets
    /// it, and each color of the theme on its own. Criteria and the deployed action stay
    /// apart, because their commands are resolved inside the Pack.
    fn over(self, pack: RawConfig) -> RawConfig {
        let theme = match (self.theme, pack.theme) {
            (Some(own), Some(pack)) => Some(RawTheme {
                muted: own.muted.or(pack.muted),
                accent: own.accent.or(pack.accent),
                success: own.success.or(pack.success),
                warning: own.warning.or(pack.warning),
                danger: own.danger.or(pack.danger),
                selection: own.selection.or(pack.selection),
                current: own.current.or(pack.current),
            }),
            (own, pack) => own.or(pack),
        };
        RawConfig {
            sections: self.sections.or(pack.sections),
            theme,
            auto_reload: self.auto_reload.or(pack.auto_reload),
            enter: self.enter.or(pack.enter),
            language: self.language.or(pack.language),
            auto_qa: self.auto_qa.or(pack.auto_qa),
            ai: self.ai.or(pack.ai),
            discover: self.discover.or(pack.discover),
            ..self
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawTheme {
    muted: Option<String>,
    accent: Option<String>,
    success: Option<String>,
    warning: Option<String>,
    danger: Option<String>,
    selection: Option<String>,
    current: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSection {
    pub(crate) plugin: String,
    #[serde(default)]
    args: Vec<String>,
    mode: Option<Mode>,
    interval: Option<String>,
    timeout: Option<String>,
    max_height: Option<usize>,
    header: Option<bool>,
    title: Option<String>,
    counts: Option<bool>,
    group_gap: Option<bool>,
    density: Option<Density>,
    collapsed: Option<bool>,
    pin: Option<Pin>,
    hidden: Option<bool>,
}

/// Location of the config file: `$XDG_CONFIG_HOME/tmux-aside/config.yaml`, defaulting to `~/.config`.
pub fn default_path() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("tmux-aside/config.yaml"))
}

/// Directory of the user's own Templates: `templates` next to the config file.
pub fn templates_dir() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("tmux-aside/templates"))
}

/// Directory for data tmux-aside keeps, such as cloned Packs: `$XDG_DATA_HOME/tmux-aside`,
/// defaulting to `~/.local/share`.
pub fn data_dir() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_DATA_HOME", ".local/share")?.join("tmux-aside"))
}

/// Directory for plugin logs: `$XDG_STATE_HOME/tmux-aside`, defaulting to `~/.local/state`.
pub fn state_dir() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_STATE_HOME", ".local/state")?.join("tmux-aside"))
}

/// Directory holding one state file per AI agent, written by the agents' hooks:
/// `$XDG_STATE_HOME/tmux-aside/agents`.
pub fn agents_dir() -> Result<PathBuf> {
    Ok(state_dir()?.join("agents"))
}

/// The state file in `dir` of the agent in tmux pane `pane` (`%3`).
pub fn agent_state_file(dir: &Path, pane: &str) -> PathBuf {
    dir.join(format!("{}.json", pane.trim_start_matches('%')))
}

/// Reads the config at `path`, or `None` when the file does not exist, with what it leaves out
/// taken from its Pack, if any; a relative `pack` is relative to `path`'s folder. A Pack that
/// cannot be read is reported and left out.
pub fn load(path: &Path) -> Result<Option<Config>> {
    let yaml = match std::fs::read_to_string(path) {
        Ok(yaml) => yaml,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let invalid = || format!("invalid config {}", path.display());
    let mut raw = own_config(&yaml).with_context(invalid)?;
    if let Some(dir) = &raw.pack {
        let dir = path.parent().unwrap_or(Path::new("")).join(dir);
        match pack::settings(&dir) {
            Ok(settings) => raw = raw.over(settings),
            Err(error) => eprintln!("{error:#}"),
        }
    }
    build(raw).with_context(invalid).map(Some)
}

/// Parses a YAML config, applying defaults and naming the section of any invalid value.
pub fn parse(yaml: &str) -> Result<Config> {
    build(own_config(yaml)?)
}

fn own_config(yaml: &str) -> Result<RawConfig> {
    let raw = RawConfig::parse(yaml)?;
    if raw.templates.is_some() {
        bail!("templates: only a Pack's manifest names its templates folder");
    }
    Ok(raw)
}

fn build(raw: RawConfig) -> Result<Config> {
    let RawConfig {
        sections: raw_sections,
        theme: raw_theme,
        auto_reload,
        enter,
        language,
        criteria,
        auto_qa,
        ai,
        pack,
        discover,
        on_deployed,
        templates: _,
    } = raw;
    let raw_sections = raw_sections.unwrap_or_default();
    let raw_theme = raw_theme.unwrap_or_default();
    let auto_reload = auto_reload.unwrap_or(false);
    let enter = enter.unwrap_or_default();
    let auto_qa = auto_qa.unwrap_or(false);
    let discover = discover.unwrap_or_default();
    let theme = parse_theme(raw_theme)?;
    let sections = raw_sections
        .into_iter()
        .enumerate()
        .map(|(index, raw)| {
            let name = format!("section {} ({})", index + 1, raw.plugin);
            let duration = |value: Option<String>, default: Duration| {
                value
                    .map(|value| parse_duration(&value))
                    .transpose()
                    .map(|value| value.unwrap_or(default))
                    .map_err(|error| anyhow!("{name}: {error}"))
            };
            Ok(SectionConfig {
                interval: duration(raw.interval, DEFAULT_INTERVAL)?,
                timeout: duration(raw.timeout, DEFAULT_TIMEOUT)?,
                plugin: raw.plugin,
                args: raw.args,
                mode: raw.mode.unwrap_or(Mode::Interval),
                max_height: raw.max_height,
                header: raw.header.unwrap_or(true),
                title: raw.title,
                counts: raw.counts.unwrap_or(true),
                group_gap: raw.group_gap.unwrap_or(false),
                density: raw.density.unwrap_or_default(),
                collapsed: raw.collapsed.unwrap_or(false),
                pin: raw.pin.unwrap_or_default(),
                hidden: raw.hidden.unwrap_or(false),
            })
        })
        .collect::<Result<_>>()?;
    Ok(Config {
        sections,
        theme,
        auto_reload,
        enter,
        language,
        criteria,
        auto_qa,
        ai,
        pack,
        discover,
        on_deployed,
    })
}

fn parse_theme(raw: RawTheme) -> Result<Theme> {
    let default = Theme::default();
    let color = |key: &str, value: Option<String>, default: String| -> Result<String> {
        let Some(value) = value else {
            return Ok(default);
        };
        value
            .parse::<ratatui::style::Color>()
            .map_err(|_| anyhow!("theme.{key}: invalid color '{value}'"))?;
        Ok(value)
    };
    Ok(Theme {
        muted: color("muted", raw.muted, default.muted)?,
        accent: color("accent", raw.accent, default.accent)?,
        success: color("success", raw.success, default.success)?,
        warning: color("warning", raw.warning, default.warning)?,
        danger: color("danger", raw.danger, default.danger)?,
        selection: raw
            .selection
            .map(|value| color("selection", Some(value), String::new()))
            .transpose()?,
        current: raw
            .current
            .map(|value| color("current", Some(value), String::new()))
            .transpose()?,
    })
}

/// Parses a positive duration written as `<n>ms`, `<n>s` or `<n>m`.
pub fn parse_duration(value: &str) -> Result<Duration> {
    let invalid = || anyhow!("invalid duration '{value}': expected <n>ms, <n>s or <n>m");
    let digits = value.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let amount: u64 = digits.parse().map_err(|_| invalid())?;
    let duration = match &value[digits.len()..] {
        "ms" => Duration::from_millis(amount),
        "s" => Duration::from_secs(amount),
        "m" => Duration::from_secs(amount * 60),
        _ => return Err(invalid()),
    };
    if duration.is_zero() {
        bail!("invalid duration '{value}': must be greater than zero");
    }
    Ok(duration)
}

fn xdg_dir(variable: &str, fallback: &str) -> Result<PathBuf> {
    match std::env::var_os(variable).filter(|value| !value.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => std::env::home_dir()
            .map(|home| home.join(fallback))
            .ok_or_else(|| anyhow!("cannot locate the home directory")),
    }
}
