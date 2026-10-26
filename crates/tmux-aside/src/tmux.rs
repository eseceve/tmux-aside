use std::process::Command;

use anyhow::{Context, Result, bail};

/// Runs a tmux command against the server named by `$TMUX` and returns its trimmed stdout.
pub fn run(args: &[&str]) -> Result<String> {
    let output = Command::new("tmux")
        .args(args)
        .output()
        .context("failed to spawn tmux")?;
    if !output.status.success() {
        bail!(
            "tmux {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
