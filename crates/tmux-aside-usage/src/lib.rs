use serde_json::Value;
use tmux_aside::i18n::Language;
use tmux_aside::protocol::{Row, Tone};

const STATUSLINE_MARKER: &str = " statusline ";

/// Text the usage section shows, in one language.
pub struct Messages {
    pub title: &'static str,
    pub five_hours: &'static str,
    pub weekly: &'static str,
    pub days: fn(u64) -> String,
    pub hours: fn(u64) -> String,
    pub no_data: &'static str,
}

const EN: Messages = Messages {
    title: "Usage",
    five_hours: "5 hours",
    weekly: "Weekly",
    days: |days| format!("{days} days"),
    hours: |hours| format!("{hours} hours"),
    no_data: "no data",
};

const ES: Messages = Messages {
    title: "Uso",
    five_hours: "5 horas",
    weekly: "Semanal",
    days: |days| format!("{days} días"),
    hours: |hours| format!("{hours} horas"),
    no_data: "sin datos",
};

/// The usage section's text in `language`.
pub fn messages(language: Language) -> &'static Messages {
    match language {
        Language::En => &EN,
        Language::Es => &ES,
    }
}

/// One usage window of a plan, such as the 5-hour or the weekly limit.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub label: String,
    pub used_percent: f64,
    /// Unix time in seconds when the window resets.
    pub resets_at: u64,
}

/// Windows from the `rate_limits` object Claude Code passes to its statusline command,
/// skipping missing windows and windows that already reset.
pub fn claude_windows(rate_limits: &Value, now: u64, language: Language) -> Vec<Window> {
    let messages = messages(language);
    [
        ("five_hour", messages.five_hours),
        ("seven_day", messages.weekly),
    ]
    .into_iter()
    .filter_map(|(key, label)| {
        let window = rate_limits.get(key)?;
        Some(Window {
            label: label.to_string(),
            used_percent: window.get("used_percentage")?.as_f64()?,
            resets_at: window.get("resets_at")?.as_u64()?,
        })
    })
    .filter(|window| window.resets_at > now)
    .collect()
}

/// Windows from the result of the Codex app-server `account/rateLimits/read` method, named by
/// their duration because a plan may have only a weekly window in the primary slot.
pub fn codex_windows(result: &Value, now: u64, language: Language) -> Vec<Window> {
    ["primary", "secondary"]
        .into_iter()
        .filter_map(|slot| {
            let window = result.pointer(&format!("/rateLimits/{slot}"))?;
            let minutes = window.get("windowDurationMins")?.as_u64()?;
            Some(Window {
                label: window_label(minutes, language),
                used_percent: window.get("usedPercent")?.as_f64()?,
                resets_at: window.get("resetsAt")?.as_u64()?,
            })
        })
        .filter(|window| window.resets_at > now)
        .collect()
}

/// One row per window grouped by tool, with a meter of the percent used, colored by how close
/// the window is to its limit, and the muted time left until it resets. Labels and details are
/// padded to a common width so the meters line up. Tools without windows show a muted
/// "no data" row.
pub fn rows(tools: &[(String, Vec<Window>)], now: u64, language: Language) -> Vec<Row> {
    let label_width = tools
        .iter()
        .flat_map(|(_, windows)| windows)
        .map(|window| window.label.chars().count())
        .max()
        .unwrap_or(0);
    let mut rows = Vec::new();
    for (tool, windows) in tools {
        if windows.is_empty() {
            let mut row = Row::new(messages(language).no_data);
            row.tone = Tone::Muted;
            row.group = Some(tool.clone());
            rows.push(row);
        }
        for window in windows {
            let meter_tone = match window.used_percent {
                used if used >= 90.0 => Tone::Danger,
                used if used >= 70.0 => Tone::Warning,
                _ => Tone::Success,
            };
            rows.push(Row {
                group: Some(tool.clone()),
                detail_tone: Tone::Muted,
                meter_tone,
                meter: Some(window.used_percent.clamp(0.0, 100.0).round() as u8),
                ..Row::with_detail(
                    format!("{:<label_width$}", window.label),
                    format!(
                        "{:.0}% · {}",
                        window.used_percent,
                        format_remaining(window.resets_at.saturating_sub(now))
                    ),
                )
            });
        }
    }
    let detail_width = rows
        .iter()
        .filter_map(|row| row.detail.as_ref())
        .map(|detail| detail.chars().count())
        .max()
        .unwrap_or(0);
    for detail in rows.iter_mut().filter_map(|row| row.detail.as_mut()) {
        *detail = format!("{detail:>detail_width$}");
    }
    rows
}

/// Formats a number of seconds as `45m`, `2h10m` or `3d5h`.
pub fn format_remaining(seconds: u64) -> String {
    let minutes = seconds / 60;
    match minutes {
        0..60 => format!("{minutes}m"),
        60..1_440 => format!("{}h{}m", minutes / 60, minutes % 60),
        _ => format!("{}d{}h", minutes / 1_440, (minutes % 1_440) / 60),
    }
}

/// Statusline command that records Claude Code's rate limits and then runs `original`.
pub fn statusline_command(exe: &str, original: &str) -> String {
    format!(
        "{}{STATUSLINE_MARKER}{}",
        shell_quote(exe),
        shell_quote(original)
    )
}

/// The original command of a statusline wrapped by [`statusline_command`], if it is one.
pub fn wrapped_statusline(command: &str) -> Option<String> {
    let (exe, original) = command.split_once(STATUSLINE_MARKER)?;
    if !exe.contains("tmux-aside-usage") {
        return None;
    }
    shell_unquote(original)
}

fn window_label(minutes: u64, language: Language) -> String {
    let messages = messages(language);
    match minutes {
        300 => messages.five_hours.to_string(),
        10_080 => messages.weekly.to_string(),
        minutes if minutes % 1_440 == 0 => (messages.days)(minutes / 1_440),
        minutes => (messages.hours)(minutes / 60),
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn shell_unquote(value: &str) -> Option<String> {
    let inner = value.strip_prefix('\'')?.strip_suffix('\'')?;
    Some(inner.replace("'\\''", "'"))
}
