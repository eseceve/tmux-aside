use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

use crate::config;
use crate::daemon;
use crate::hooks;
use crate::tmux;
use crate::width::{Width, resolve_width};

const ROLE_OPTION: &str = "@tmux_aside";
const WIDTH_OPTION: &str = "@tmux_aside_width";

/// Turns the sidebar on in every window and keeps it there through hooks.
///
/// Idempotent: windows that already have a sidebar are resized to `width` instead.
pub fn on(width: Option<Width>) -> Result<()> {
    match width {
        Some(width) => tmux::run(&["set-option", "-g", WIDTH_OPTION, &width.to_string()])?,
        None => tmux::run(&["set-option", "-gu", WIDTH_OPTION])?,
    };
    hooks::install()?;
    for window_id in tmux::run(&["list-windows", "-a", "-F", "#{window_id}"])?.lines() {
        sync_window(window_id)?;
    }
    Ok(())
}

/// Removes every sidebar pane, the hooks and the explicit width. A width set by dragging
/// the sidebar is kept for the next `on`.
pub fn off() -> Result<()> {
    hooks::remove();
    for pane_id in sidebar_panes()? {
        let _ = tmux::run(&["kill-pane", "-t", &pane_id]);
    }
    tmux::run(&["set-option", "-gu", WIDTH_OPTION])?;
    daemon::stop(&daemon::default_socket()?);
    Ok(())
}

/// Turns the sidebar off when any sidebar pane exists, on otherwise.
pub fn toggle(width: Option<Width>) -> Result<()> {
    if sidebar_panes()?.is_empty() {
        on(width)
    } else {
        off()
    }
}

/// Ensures `window_id` has exactly one sidebar at the configured width.
pub fn sync_window(window_id: &str) -> Result<()> {
    let width = configured_width()?;
    let window_width: u16 =
        tmux::run(&["display-message", "-p", "-t", window_id, "#{window_width}"])?
            .parse()
            .context("window width is not a number")?;
    let size = resolve_width(width, window_width).to_string();

    let panes = tmux::run(&[
        "list-panes",
        "-t",
        window_id,
        "-F",
        &format!("#{{pane_id}} #{{?{ROLE_OPTION},1,0}}"),
    ])?;
    let mut panes = panes.lines().filter_map(|line| line.split_once(' '));
    let Some((first_pane, _)) = panes.clone().next() else {
        return Ok(());
    };
    if let Some((sidebar_pane, _)) = panes.find(|(_, is_sidebar)| *is_sidebar == "1") {
        tmux::run(&["resize-pane", "-t", sidebar_pane, "-x", &size])?;
        return Ok(());
    }
    create_sidebar(first_pane, &size)
}

/// Reacts to the user dragging the sidebar of `window_id` to `pane_width` columns: with an
/// explicit width the sidebar snaps back, otherwise the new width is adopted by every sidebar.
pub fn adopt_manual_resize(window_id: &str, pane_width: u16) -> Result<()> {
    if explicit_width()?.is_some() {
        return sync_window(window_id);
    }
    store_synced_width(pane_width)?;
    for other in tmux::run(&["list-windows", "-a", "-F", "#{window_id}"])?.lines() {
        if other != window_id {
            sync_window(other)?;
        }
    }
    Ok(())
}

/// Width in columns the sidebar of a window `window_width` columns wide should have.
pub fn expected_width(window_width: u16) -> Result<u16> {
    Ok(resolve_width(configured_width()?, window_width))
}

/// Kills sidebars left alone in their window, which closes that window, and gives the
/// others back their width after a closed neighbor handed them its space.
pub fn reap() -> Result<()> {
    let panes = tmux::run(&[
        "list-panes",
        "-a",
        "-F",
        &format!("#{{?{ROLE_OPTION},#{{window_panes}} #{{window_id}} #{{pane_id}},}}"),
    ])?;
    for line in panes.lines() {
        match line.split(' ').collect::<Vec<_>>()[..] {
            ["1", _, pane_id] => {
                let _ = tmux::run(&["kill-pane", "-t", pane_id]);
            }
            [_, window_id, _] => {
                let _ = sync_window(window_id);
            }
            _ => {}
        }
    }
    Ok(())
}

fn create_sidebar(target_pane: &str, size: &str) -> Result<()> {
    let exe = std::env::current_exe()?;
    let exe = exe
        .to_str()
        .ok_or_else(|| anyhow!("executable path is not UTF-8"))?;
    let pane_id = tmux::run(&[
        "split-window",
        "-hbf",
        "-l",
        size,
        "-t",
        target_pane,
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        exe,
        "_render",
    ])?;
    if let Err(error) = tmux::run(&["set-option", "-p", "-t", &pane_id, ROLE_OPTION, "1"]) {
        let _ = tmux::run(&["kill-pane", "-t", &pane_id]);
        return Err(error.context(format!("failed to tag sidebar pane {pane_id}")));
    }
    Ok(())
}

fn configured_width() -> Result<Option<Width>> {
    if let Some(width) = explicit_width()? {
        return Ok(Some(width));
    }
    Ok(synced_width().map(Width::Columns))
}

fn settings_path() -> Result<PathBuf> {
    Ok(config::state_dir()?.join("settings.json"))
}

fn synced_width() -> Option<u16> {
    let settings: Value =
        serde_json::from_str(&std::fs::read_to_string(settings_path().ok()?).ok()?).ok()?;
    settings.get("sidebar_width")?.as_u64()?.try_into().ok()
}

fn store_synced_width(width: u16) -> Result<()> {
    let path = settings_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, json!({ "sidebar_width": width }).to_string())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn explicit_width() -> Result<Option<Width>> {
    let value = tmux::run(&["show-option", "-gqv", WIDTH_OPTION])?;
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse()
        .map(Some)
        .map_err(|error: String| anyhow!(error))
}

/// Shows `window` with its sidebar focused, or runs `fallback` when the window has no sidebar.
pub fn select(window: &str, fallback: &[String]) -> Result<()> {
    let sidebar = tmux::run(&[
        "list-panes",
        "-t",
        window,
        "-F",
        &format!("#{{?{ROLE_OPTION},#{{pane_id}},}}"),
    ])
    .ok()
    .and_then(|panes| {
        panes
            .lines()
            .find(|line| !line.is_empty())
            .map(str::to_string)
    });
    match (sidebar, fallback.split_first()) {
        (Some(pane), _) => tmux::run(&["switch-client", "-t", &pane]).map(|_| ()),
        (None, Some((program, args))) => {
            std::process::Command::new(program).args(args).status()?;
            Ok(())
        }
        (None, None) => Ok(()),
    }
}

fn sidebar_panes() -> Result<Vec<String>> {
    Ok(tmux::run(&[
        "list-panes",
        "-a",
        "-F",
        &format!("#{{?{ROLE_OPTION},#{{pane_id}},}}"),
    ])?
    .lines()
    .filter(|line| !line.is_empty())
    .map(str::to_string)
    .collect())
}
