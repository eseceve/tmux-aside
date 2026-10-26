use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tmux_aside::protocol::{Action, Row, Tone};

/// What an agent is doing, as reported by its hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Working,
    Waiting,
    Done,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::Waiting => "waiting",
            Status::Done => "done",
        }
    }
}

/// Persisted state of the agent running in one tmux pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentState {
    pub pane_id: String,
    pub pane_pid: u32,
    pub command: String,
    pub project: String,
    pub window: String,
    pub status: Option<Status>,
    pub status_ts: u64,
    pub activity_ts: u64,
    /// Live pane title (agents such as Claude Code put a conversation summary there).
    #[serde(default)]
    pub title: String,
    /// Directory the agent runs in, used to find its branch's pull request.
    #[serde(default)]
    pub workdir: String,
    /// tmux window the agent's pane is in.
    #[serde(default)]
    pub window_id: String,
    /// Lines changed on the agent's branch, when diff stats are shown.
    #[serde(skip)]
    pub diff: Option<Diff>,
    /// Working, but its pane has not changed for longer than the stall threshold.
    #[serde(skip)]
    pub stalled: bool,
    /// Found by its command rather than registered by hooks; its status is inferred from
    /// whether its pane is changing.
    #[serde(default)]
    pub inferred: bool,
    /// Working only because background subagents it started are still running.
    #[serde(default)]
    pub background: bool,
}

/// Lines added and removed by a branch, committed or not, since it left its base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diff {
    pub insertions: u64,
    pub deletions: u64,
}

/// Git branch an agent works on and its open pull request, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub pr: Option<PullRequest>,
}

/// Combined result of a pull request's CI checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checks {
    Passing,
    Failing,
    Pending,
    None,
}

/// Open pull request of an agent's branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub checks: Checks,
}

/// How agent rows are ordered and what they show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowOptions {
    pub sort: Sort,
    /// Show the time spent in the current status when the agent has no pull request.
    pub elapsed: bool,
    pub group: Grouping,
}

/// Whether agents are listed under a line per project or in one list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Grouping {
    /// A group line per project, even for a project with a single agent.
    #[default]
    Project,
    /// One list ordered across projects, each agent naming its project on its own line.
    None,
}

/// A pane of the running tmux server, as listed by `list-panes -a`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePane {
    pub pane_id: String,
    pub pane_pid: u32,
    pub command: String,
    pub title: String,
    pub path: String,
    pub window_id: String,
    pub window_name: String,
    /// The pane is the one a user is currently looking at.
    pub viewed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    /// Waiting, then done, then working, then idle agents.
    Priority,
    Alphabetical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciliation {
    pub agents: Vec<AgentState>,
    /// Panes whose state no longer belongs to a live agent.
    pub dropped: Vec<String>,
    /// Panes whose waiting or done status was cleared because the user viewed them.
    pub acknowledged: Vec<String>,
}

/// Matches stored states against live panes: drops agents whose pane is gone, was recycled
/// (different process) or no longer runs the agent, and acknowledges waiting or done agents
/// whose pane is being viewed.
pub fn reconcile(states: Vec<AgentState>, live: &[LivePane]) -> Reconciliation {
    let mut result = Reconciliation {
        agents: Vec::new(),
        dropped: Vec::new(),
        acknowledged: Vec::new(),
    };
    for mut state in states {
        let pane = live.iter().find(|pane| {
            pane.pane_id == state.pane_id
                && pane.pane_pid == state.pane_pid
                && pane.command == state.command
        });
        let Some(pane) = pane else {
            result.dropped.push(state.pane_id);
            continue;
        };
        state.title = pane.title.clone();
        state.workdir = pane.path.clone();
        state.window_id = pane.window_id.clone();
        if pane.viewed && matches!(state.status, Some(Status::Waiting | Status::Done)) {
            state.status = None;
            result.acknowledged.push(state.pane_id.clone());
        }
        result.agents.push(state);
    }
    result
}

/// Builds one row per agent: the window name, then a line with the diff stats and branch (and
/// the project, when not grouped) and a line with the pane title, skipping what repeats the
/// window name or the line above. With `Grouping::Project` agents are grouped by project
/// (projects in alphabetical order) and sorted within by `options.sort`; with `Grouping::None`
/// they are sorted by `options.sort` across projects. The right side shows the pull request of the agent's branch (`branches`, keyed by
/// pane id) colored by its checks, or else the time spent waiting (or in any status when
/// `options.elapsed`). Enter on a row jumps to the agent's pane.
pub fn rows(
    agents: &[AgentState],
    now: u64,
    options: RowOptions,
    branches: &HashMap<String, Branch>,
) -> Vec<Row> {
    let mut agents: Vec<&AgentState> = agents.iter().collect();
    let grouped = options.group == Grouping::Project;
    agents.sort_by(|a, b| {
        let by_group = match grouped {
            true => group_name(a).cmp(&group_name(b)),
            false => std::cmp::Ordering::Equal,
        };
        by_group.then_with(|| match options.sort {
            Sort::Priority => priority(a.status)
                .cmp(&priority(b.status))
                .then_with(|| a.window.cmp(&b.window)),
            Sort::Alphabetical => a.window.cmp(&b.window),
        })
    });
    agents
        .into_iter()
        .map(|agent| {
            row(
                agent,
                now,
                options.elapsed,
                grouped,
                branches.get(&agent.pane_id),
            )
        })
        .collect()
}

/// Agents whose status became one of `statuses` since `previous`, the last status seen per
/// pane; panes missing from it count as having had no status.
pub fn entered<'a>(
    previous: &HashMap<String, Option<Status>>,
    agents: &'a [AgentState],
    statuses: &[Status],
) -> Vec<&'a AgentState> {
    agents
        .iter()
        .filter(|agent| {
            agent
                .status
                .is_some_and(|status| statuses.contains(&status))
                && previous.get(&agent.pane_id).copied().flatten() != agent.status
        })
        .collect()
}

/// Whether a Claude Code `Stop` hook input lists subagents, workflows or teammates still
/// running in the background, which wake the agent again when they finish.
pub fn waits_for_background_agents(stop_input: &Value) -> bool {
    stop_input
        .get("background_tasks")
        .and_then(Value::as_array)
        .is_some_and(|tasks| {
            tasks.iter().any(|task| {
                let field = |name| task.get(name).and_then(Value::as_str);
                field("status") == Some("running")
                    && matches!(field("type"), Some("subagent" | "workflow" | "teammate"))
            })
        })
}

/// Status of an agent without hooks: working while its pane is `active` (changing), done once
/// it stops unless the pane is `viewed`, and otherwise unchanged.
pub fn infer_status(previous: Option<Status>, active: bool, viewed: bool) -> Option<Status> {
    match (active, viewed, previous) {
        (true, _, _) => Some(Status::Working),
        (false, true, _) => None,
        (false, false, Some(Status::Working)) => Some(Status::Done),
        (false, false, previous) => previous,
    }
}

/// The agent to jump to from the pane `current`: agents in one of `statuses` are ordered by
/// how long they have been in it, longest first, and the one after `current` is picked when
/// `current` is among them, else the first, so repeated jumps cycle through all of them.
pub fn next_agent<'a>(
    agents: &'a [AgentState],
    statuses: &[Status],
    current: &str,
) -> Option<&'a AgentState> {
    let mut candidates: Vec<&AgentState> = agents
        .iter()
        .filter(|agent| {
            agent
                .status
                .is_some_and(|status| statuses.contains(&status))
        })
        .collect();
    candidates.sort_by(|a, b| {
        a.status_ts
            .cmp(&b.status_ts)
            .then(a.pane_id.cmp(&b.pane_id))
    });
    let after = candidates
        .iter()
        .position(|agent| agent.pane_id == current)
        .map_or(0, |index| index + 1);
    candidates.get(after % candidates.len().max(1)).copied()
}

/// Fills `{window}`, `{status}`, `{project}` and `{title}` in `template` with `agent`'s values.
pub fn notification(template: &str, agent: &AgentState) -> String {
    template
        .replace("{window}", clean_title(&agent.window))
        .replace("{status}", agent.status.map_or("", Status::name))
        .replace("{project}", &group_name(agent))
        .replace("{title}", clean_title(&agent.title))
}

/// Parses `git diff --shortstat` output; `None` when nothing changed.
pub fn parse_shortstat(output: &str) -> Option<Diff> {
    let count = |kind: &str| {
        output
            .split(',')
            .find(|part| part.contains(kind))
            .and_then(|part| part.split_whitespace().next()?.parse().ok())
            .unwrap_or(0)
    };
    let diff = Diff {
        insertions: count("insertion"),
        deletions: count("deletion"),
    };
    (diff.insertions + diff.deletions > 0).then_some(diff)
}

/// Summarizes a `statusCheckRollup`: failing if any check failed, pending if any is still
/// running, passing when every check succeeded or was skipped.
pub fn checks_from_rollup(rollup: &[Value]) -> Checks {
    if rollup.is_empty() {
        return Checks::None;
    }
    let field =
        |check: &Value, name: &str| check.get(name).and_then(Value::as_str).map(str::to_string);
    let mut pending = false;
    for check in rollup {
        let outcome = field(check, "conclusion")
            .or_else(|| field(check, "state"))
            .unwrap_or_default();
        match outcome.as_str() {
            "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED"
            | "STARTUP_FAILURE" => {
                return Checks::Failing;
            }
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => {}
            _ => pending = true,
        }
    }
    if pending {
        Checks::Pending
    } else {
        Checks::Passing
    }
}

/// Parses `gh pr list --json number,headRefName,statusCheckRollup` output into pull requests
/// keyed by head branch.
pub fn pull_requests_by_branch(output: &str) -> anyhow::Result<HashMap<String, PullRequest>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Listed {
        number: u64,
        head_ref_name: String,
        #[serde(default)]
        status_check_rollup: Vec<Value>,
    }
    let listed: Vec<Listed> = serde_json::from_str(output)?;
    Ok(listed
        .into_iter()
        .map(|pr| {
            let checks = checks_from_rollup(&pr.status_check_rollup);
            (
                pr.head_ref_name,
                PullRequest {
                    number: pr.number,
                    checks,
                },
            )
        })
        .collect())
}

/// Formats an elapsed number of seconds as `m:ss` under an hour, `Nh` under a day, `Nd` after.
pub fn format_elapsed(seconds: u64) -> String {
    match seconds {
        0..3_600 => format!("{}:{:02}", seconds / 60, seconds % 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

fn row(agent: &AgentState, now: u64, elapsed: bool, grouped: bool, branch: Option<&Branch>) -> Row {
    let bar_tone = match agent.status {
        Some(Status::Working) if agent.stalled => Tone::Danger,
        Some(Status::Working) => Tone::Accent,
        Some(Status::Waiting) => Tone::Warning,
        Some(Status::Done) => Tone::Success,
        None => Tone::Muted,
    };
    let window = clean_title(&agent.window);
    let branch_name = branch.map(|branch| branch.name.as_str());
    let group = group_name(agent);
    let diff = agent
        .diff
        .map(|diff| format!("+{} -{}", diff.insertions, diff.deletions));
    let project_line = [
        (!grouped).then_some(group.as_str()),
        diff.as_deref(),
        branch_name.filter(|name| *name != window),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let title = Some(clean_title(&agent.title))
        .filter(|title| !title.is_empty() && *title != window && *title != group);
    let agent_line = match title {
        Some(title) => format!("{} · {title}", agent_name(&agent.command)),
        None => agent_name(&agent.command).to_string(),
    };
    let lines = [project_line, agent_line]
        .into_iter()
        .filter(|line| !line.is_empty() && line != window)
        .collect();
    let elapsed_time = || format_elapsed(now.saturating_sub(agent.status_ts));
    let pr = branch.and_then(|branch| branch.pr.as_ref());
    let (detail, detail_tone) = match (agent.status, pr) {
        (Some(Status::Working), _) if agent.stalled => (Some("stalled".to_string()), Tone::Danger),
        (Some(Status::Working), _) => (Some("spinner".to_string()), Tone::Accent),
        (Some(Status::Waiting), _) => (Some(elapsed_time()), Tone::Muted),
        (_, Some(pr)) => pull_request_detail(pr),
        (_, None) => (elapsed.then(elapsed_time), Tone::Muted),
    };
    Row {
        bar_tone,
        group: grouped.then_some(group),
        lines,
        window: (!agent.window_id.is_empty()).then(|| agent.window_id.clone()),
        detail,
        detail_tone,
        actions: std::iter::once(Action {
            label: Some("go to the agent".to_string()),
            ..Action::new("enter", &["tmux", "switch-client", "-t", &agent.pane_id])
        })
        .chain(pr.map(|pr| Action {
            label: Some("open its pull request".to_string()),
            cwd: (!agent.workdir.is_empty()).then(|| agent.workdir.clone()),
            ..Action::new("p", &["gh", "pr", "view", "--web", &pr.number.to_string()])
        }))
        .collect(),
        ..Row::new(agent.window.clone())
    }
}

fn agent_name(command: &str) -> &str {
    match command {
        "claude" => "CC",
        "codex" => "Codex",
        "agy" => "Agy",
        other => other,
    }
}

fn pull_request_detail(pr: &PullRequest) -> (Option<String>, Tone) {
    let number = format!("#{}", pr.number);
    let (text, tone) = match pr.checks {
        Checks::Passing => (format!("{number} ✓"), Tone::Success),
        Checks::Failing => (format!("{number} ×"), Tone::Danger),
        Checks::Pending => (format!("{number} ◷"), Tone::Accent),
        Checks::None => (number, Tone::Muted),
    };
    (Some(text), tone)
}

fn clean_title(title: &str) -> &str {
    title
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .trim()
}

fn priority(status: Option<Status>) -> u8 {
    match status {
        Some(Status::Waiting) => 0,
        Some(Status::Done) => 1,
        Some(Status::Working) => 2,
        None => 3,
    }
}

/// Project an agent's directory belongs to, as workmux names it: the nearest ancestor whose
/// `.git` is a directory, else the prefix of an ancestor named `<project>__worktrees`, else
/// the directory's own name.
pub fn project_name(path: &Path) -> String {
    for ancestor in path.ancestors() {
        let Some(name) = ancestor.file_name().map(|name| name.to_string_lossy()) else {
            continue;
        };
        if ancestor.join(".git").is_dir() {
            return name.into_owned();
        }
        if let Some(project) = name.strip_suffix("__worktrees") {
            return project.to_string();
        }
    }
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn group_name(agent: &AgentState) -> String {
    agent.project.clone()
}
