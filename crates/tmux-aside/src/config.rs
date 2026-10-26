use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub sections: Vec<SectionConfig>,
    pub theme: Theme,
    /// Reload the config whenever the file changes, without `tmux-aside reload`.
    pub auto_reload: bool,
    pub enter: EnterAction,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    sections: Vec<RawSection>,
    theme: Option<RawTheme>,
    #[serde(default)]
    auto_reload: bool,
    #[serde(default)]
    enter: EnterAction,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawTheme {
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
struct RawSection {
    plugin: String,
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
}

/// Location of the config file: `$XDG_CONFIG_HOME/tmux-aside/config.yaml`, defaulting to `~/.config`.
pub fn default_path() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("tmux-aside/config.yaml"))
}

/// Directory for plugin logs: `$XDG_STATE_HOME/tmux-aside`, defaulting to `~/.local/state`.
pub fn state_dir() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_STATE_HOME", ".local/state")?.join("tmux-aside"))
}

/// Reads the config at `path`, or `None` when the file does not exist.
pub fn load(path: &Path) -> Result<Option<Config>> {
    match std::fs::read_to_string(path) {
        Ok(yaml) => parse(&yaml)
            .with_context(|| format!("invalid config {}", path.display()))
            .map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Parses a YAML config, applying defaults and naming the section of any invalid value.
pub fn parse(yaml: &str) -> Result<Config> {
    let raw: Option<RawConfig> =
        serde_saphyr::from_str(yaml).map_err(|error| anyhow!("{error}"))?;
    let (raw_sections, raw_theme, auto_reload, enter) = raw
        .map(|raw| {
            (
                raw.sections,
                raw.theme.unwrap_or_default(),
                raw.auto_reload,
                raw.enter,
            )
        })
        .unwrap_or_default();
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
            })
        })
        .collect::<Result<_>>()?;
    Ok(Config {
        sections,
        theme,
        auto_reload,
        enter,
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
