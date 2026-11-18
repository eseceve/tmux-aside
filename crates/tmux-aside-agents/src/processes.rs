//! Process identity shared by the observer, legacy hooks and the Watcher.
use std::path::Path;

struct Process<'a> {
    pid: u32,
    parent: u32,
    name: &'a str,
}

fn parse(table: &str) -> Vec<Process<'_>> {
    table
        .lines()
        .filter_map(|line| {
            let (pid, rest) = line.trim().split_once(char::is_whitespace)?;
            let (parent, executable) = rest.trim_start().split_once(char::is_whitespace)?;
            Some(Process {
                pid: pid.parse().ok()?,
                parent: parent.parse().ok()?,
                name: Path::new(executable.trim()).file_name()?.to_str()?,
            })
        })
        .collect()
}

/// Resolve a foreground agent behind its pane shell or a launcher that runs it as a child
/// in the same process group (tmux-aside, terminal-orchestrator), which tmux reports instead.
/// The process PID changes when an agent restarts within the same pane shell.
pub fn pane_process(pane_pid: u32, foreground: &str, table: &str) -> (String, Option<u32>) {
    let processes = parse(table);
    let supported = |name: &str| matches!(name, "codex" | "claude" | "agy");
    if !supported(foreground) {
        if let Some(child) = processes
            .iter()
            .find(|p| p.parent == pane_pid && supported(p.name))
        {
            return (child.name.to_string(), Some(child.pid));
        }
    } else if let Some(process) = processes
        .iter()
        .find(|p| p.pid == pane_pid && p.name == foreground)
        .or_else(|| {
            processes
                .iter()
                .find(|p| p.parent == pane_pid && p.name == foreground)
        })
    {
        return (foreground.to_string(), Some(process.pid));
    }
    (foreground.to_string(), None)
}

/// Whether a legacy hook originates in Codex, including hooks executed by its shared server.
/// Walk the parent chain rather than trusting the inherited target pane.
pub fn from_codex(mut pid: u32, table: &str) -> bool {
    let processes = parse(table);
    for _ in 0..128 {
        let Some(process) = processes.iter().find(|p| p.pid == pid) else {
            return false;
        };
        if process.name == "codex" {
            return true;
        }
        if process.name == "claude" || process.parent == pid {
            return false;
        }
        pid = process.parent;
    }
    false
}
