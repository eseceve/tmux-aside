use anyhow::{Result, anyhow};

use crate::tmux;

const SYNC_HOOKS: &[&str] = &[
    "after-new-window[90]",
    "after-new-session[90]",
    "window-resized[90]",
];

const REAP_HOOKS: &[&str] = &["after-kill-pane[90]", "pane-exited[90]"];

/// Installs global hooks at a fixed index so hooks set by the user or other tools survive.
pub fn install() -> Result<()> {
    let exe = std::env::current_exe()?;
    let exe = exe
        .to_str()
        .ok_or_else(|| anyhow!("executable path is not UTF-8"))?;
    let exe = shell_quote(exe);
    let sync = run_shell(&format!("{exe} _sync --window #{{window_id}}"));
    let reap = run_shell(&format!("{exe} _reap"));
    for (hooks, command) in [(SYNC_HOOKS, &sync), (REAP_HOOKS, &reap)] {
        for hook in hooks {
            tmux::run(&["set-hook", "-g", hook, command])?;
        }
    }
    Ok(())
}

/// Removes only the hook indices owned by tmux-aside.
pub fn remove() {
    for hook in SYNC_HOOKS.iter().chain(REAP_HOOKS) {
        let _ = tmux::run(&["set-hook", "-gu", hook]);
    }
}

fn run_shell(command: &str) -> String {
    format!("run-shell -b {}", tmux_quote(command))
}

pub(crate) fn tmux_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
