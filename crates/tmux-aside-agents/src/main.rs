use std::collections::HashMap;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};
use tmux_aside::protocol::{PLUGIN_PROTOCOL, PluginMessage, Row, Tone};
use tmux_aside_agents::{
    AgentState, Branch, Diff, Grouping, LivePane, RowOptions, Sort, Status, entered, infer_status,
    next_agent, notification, parse_shortstat, project_name, pull_requests_by_branch, reconcile,
    rows, waits_for_background_agents,
};

const POLL: Duration = Duration::from_secs(1);
const GITHUB_POLL: Duration = Duration::from_secs(30);
const DIFF_POLL: Duration = Duration::from_secs(5);
const ACTIVE_FOR: u64 = 3;
const NOTIFICATION_MS: &str = "5000";
const SUPPORTED_AGENTS: [&str; 3] = ["claude", "codex", "agy"];
const LIVE_PANE_FORMAT: &str = "#{pane_id}\t#{pane_pid}\t#{pane_current_command}\t#{pane_active}\t#{window_active}\t#{session_attached}\t#{pane_current_path}\t#{window_id}\t#{host}\t#{window_name}\t#{pane_title}";

/// Sidebar section listing AI agents running in tmux panes, and the hook commands that
/// report their status.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Order of agents within each project when running as a plugin
    #[arg(long, value_enum, default_value_t = SortArg::Priority)]
    sort: SortArg,
    /// Show how long each agent has been in its status when running as a plugin
    #[arg(long)]
    elapsed: bool,
    /// List agents under a line per project, or in one list with the project on each agent
    #[arg(long, value_enum, default_value_t = GroupArg::Project)]
    group: GroupArg,
    /// Show the lines the agent's branch adds and removes, committed or not, as +N -M
    #[arg(long)]
    diff: bool,
    /// Mark a working agent as stalled when its pane has not changed for DURATION (e.g. 60s)
    #[arg(long, value_name = "DURATION", value_parser = tmux_aside::config::parse_duration)]
    stall_after: Option<Duration>,
    /// Also list panes running COMMAND (e.g. aider) as agents; claude, codex and agy
    /// (Antigravity) are always listed. Until a hook reports their status, they are working while
    /// their pane changes and done when it stops
    #[arg(long, value_name = "COMMAND")]
    detect: Vec<String>,
    /// Notify when an agent enters STATUS, unless its pane is the one being viewed
    #[arg(long, value_name = "STATUS", value_enum)]
    notify: Vec<NotifyStatus>,
    /// Where notifications are shown
    #[arg(long, value_enum, default_value = "tmux")]
    notify_via: Vec<Via>,
    /// Notification text; {window}, {status}, {project} and {title} are replaced
    #[arg(long, default_value = "{window}: {status}")]
    notify_message: String,
}

#[derive(Subcommand)]
enum Command {
    /// Record the status of the agent in $TMUX_PANE (called from agent hooks)
    SetStatus { status: StatusArg },
    /// Record that an agent started in $TMUX_PANE (called from agent hooks)
    Register,
    /// Switch to the next agent in STATUS, longest waiting first, cycling on repeated calls
    Next {
        #[arg(long, value_name = "STATUS", value_enum, default_value = "waiting")]
        status: Vec<NotifyStatus>,
    },
    /// Install the status hooks into an agent's settings
    Setup {
        #[command(subcommand)]
        agent: SetupAgent,
    },
}

#[derive(Subcommand)]
enum SetupAgent {
    /// Merge the hooks into Claude Code's settings.json ($CLAUDE_CONFIG_DIR or ~/.claude)
    Claude,
}

#[derive(Clone, Copy, ValueEnum)]
enum StatusArg {
    Working,
    Waiting,
    Done,
    Clear,
}

#[derive(Clone, Copy, ValueEnum)]
enum NotifyStatus {
    Working,
    Waiting,
    Done,
}

/// Where a notification is shown.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Via {
    /// The status line of every attached tmux client
    Tmux,
    /// A desktop notification: osascript on macOS, notify-send elsewhere
    Desktop,
}

struct Notify {
    statuses: Vec<Status>,
    via: Vec<Via>,
    message: String,
}

#[derive(Clone, Copy, ValueEnum)]
enum GroupArg {
    Project,
    None,
}

#[derive(Clone, Copy, ValueEnum)]
enum SortArg {
    Priority,
    Alphabetical,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::SetStatus { status }) => hook(|| set_status(status)),
        Some(Command::Register) => hook(register),
        Some(Command::Next { status }) => jump(&statuses(status)),
        Some(Command::Setup {
            agent: SetupAgent::Claude,
        }) => setup_claude(),
        None => run_plugin(
            RowOptions {
                sort: match cli.sort {
                    SortArg::Priority => Sort::Priority,
                    SortArg::Alphabetical => Sort::Alphabetical,
                },
                elapsed: cli.elapsed,
                group: match cli.group {
                    GroupArg::Project => Grouping::Project,
                    GroupArg::None => Grouping::None,
                },
            },
            Notify {
                statuses: statuses(cli.notify),
                via: cli.notify_via,
                message: cli.notify_message,
            },
            cli.diff,
            cli.stall_after,
            SUPPORTED_AGENTS
                .into_iter()
                .map(String::from)
                .chain(cli.detect)
                .collect(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn statuses(arguments: Vec<NotifyStatus>) -> Vec<Status> {
    arguments
        .into_iter()
        .map(|status| match status {
            NotifyStatus::Working => Status::Working,
            NotifyStatus::Waiting => Status::Waiting,
            NotifyStatus::Done => Status::Done,
        })
        .collect()
}

fn jump(statuses: &[Status]) -> Result<()> {
    let agents = reconcile(load_states(&agents_dir()?), &live_panes()?).agents;
    let current = tmux(&["display-message", "-p", "#{pane_id}"]).unwrap_or_default();
    match next_agent(&agents, statuses, &current) {
        Some(agent) => tmux(&["switch-client", "-t", &agent.pane_id]).map(drop),
        None => {
            let names: Vec<&str> = statuses.iter().map(|status| status.name()).collect();
            tmux(&[
                "display-message",
                "-l",
                &format!("no {} agents", names.join(" or ")),
            ])
            .map(drop)
        }
    }
}

fn hook(run: impl FnOnce() -> Result<()>) -> Result<()> {
    if let Err(error) = run() {
        eprintln!("tmux-aside-agents: {error:#}");
    }
    Ok(())
}

fn set_status(status: StatusArg) -> Result<()> {
    let status = match status {
        StatusArg::Working => Some(Status::Working),
        StatusArg::Waiting => Some(Status::Waiting),
        StatusArg::Done if waits_for_background_agents(&hook_input()) => {
            return record(Some(Status::Working), true);
        }
        StatusArg::Done => Some(Status::Done),
        StatusArg::Clear => {
            if let Ok(pane) = std::env::var("TMUX_PANE") {
                let _ = std::fs::remove_file(state_file(&agents_dir()?, &pane));
            }
            return Ok(());
        }
    };
    record(status, false)
}

/// JSON the agent passes its hook on stdin, or `null` when run by hand from a terminal.
fn hook_input() -> Value {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Value::Null;
    }
    let mut input = String::new();
    let _ = stdin.read_to_string(&mut input);
    serde_json::from_str(&input).unwrap_or(Value::Null)
}

fn register() -> Result<()> {
    record(None, false)
}

fn record(status: Option<Status>, background: bool) -> Result<()> {
    let Ok(pane) = std::env::var("TMUX_PANE") else {
        return Ok(());
    };
    let info = tmux(&[
        "display-message",
        "-p",
        "-t",
        &pane,
        "#{pane_pid}\t#{pane_current_command}\t#{pane_current_path}\t#{window_name}",
    ])?;
    let fields: Vec<&str> = info.splitn(4, '\t').collect();
    let [pid, command, path, window] = fields[..] else {
        bail!("unexpected pane info: {info}");
    };
    let dir = agents_dir()?;
    std::fs::create_dir_all(&dir)?;
    let file = state_file(&dir, &pane);
    let previous: Option<AgentState> = std::fs::read_to_string(&file)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok());
    let now = now();
    let status_ts = match &previous {
        Some(previous) if previous.status == status => previous.status_ts,
        _ => now,
    };
    let state = AgentState {
        pane_id: pane.clone(),
        pane_pid: pid.parse().context("pane pid is not a number")?,
        command: command.to_string(),
        project: project_name(Path::new(path)),
        window: window.to_string(),
        status,
        status_ts,
        activity_ts: now,
        workdir: path.to_string(),
        window_id: previous
            .as_ref()
            .map(|previous| previous.window_id.clone())
            .unwrap_or_default(),
        title: previous.map(|previous| previous.title).unwrap_or_default(),
        diff: None,
        stalled: false,
        inferred: false,
        background,
    };
    save(&dir, &state)
}

fn run_plugin(
    options: RowOptions,
    notify: Notify,
    diff: bool,
    stall_after: Option<Duration>,
    detect: Vec<String>,
) -> Result<()> {
    let mut stdout = std::io::stdout();
    print(
        &mut stdout,
        &PluginMessage::Hello {
            protocol: PLUGIN_PROTOCOL,
            features: vec![],
        },
    )?;
    std::thread::spawn(|| {
        for _ in std::io::stdin().lock().lines() {}
        std::process::exit(0);
    });
    let dir = agents_dir()?;
    let branches = Arc::new(Mutex::new(HashMap::new()));
    let (refresh, refresh_requests) = mpsc::channel::<()>();
    {
        let dir = dir.clone();
        let branches = branches.clone();
        std::thread::spawn(move || {
            loop {
                let agents = live_panes()
                    .map(|live| reconcile(load_states(&dir), &live).agents)
                    .unwrap_or_default();
                let found = agent_branches(&agents);
                *branches.lock().unwrap() = found;
                if let Err(mpsc::RecvTimeoutError::Disconnected) =
                    refresh_requests.recv_timeout(GITHUB_POLL)
                {
                    return;
                }
                while refresh_requests.try_recv().is_ok() {}
            }
        });
    }
    let diffs = Arc::new(Mutex::new(HashMap::new()));
    if diff {
        let dir = dir.clone();
        let diffs = diffs.clone();
        std::thread::spawn(move || {
            loop {
                let agents = live_panes()
                    .map(|live| reconcile(load_states(&dir), &live).agents)
                    .unwrap_or_default();
                let found: HashMap<String, Diff> = agents
                    .iter()
                    .filter_map(|agent| Some((agent.pane_id.clone(), branch_diff(&agent.workdir)?)))
                    .collect();
                *diffs.lock().unwrap() = found;
                std::thread::sleep(DIFF_POLL);
            }
        });
    }
    let mut screens = Screens::default();
    let mut last = String::new();
    let mut seen: Option<HashMap<String, Option<Status>>> = None;
    let mut known_panes: Vec<String> = Vec::new();
    loop {
        let current = branches.lock().unwrap().clone();
        let mut panes: Vec<String> = load_states(&dir)
            .into_iter()
            .map(|agent| agent.pane_id)
            .collect();
        panes.sort();
        if panes != known_panes {
            let _ = refresh.send(());
            known_panes = panes;
        }
        let live = live_panes();
        match live.and_then(|live| Ok((current_agents(&dir, &live, &detect)?, live))) {
            Ok((mut agents, live)) => {
                let diffs = diffs.lock().unwrap();
                for agent in &mut agents {
                    agent.diff = diffs.get(&agent.pane_id).copied();
                }
                drop(diffs);
                let now = now();
                let watched: Vec<String> = agents
                    .iter()
                    .filter(|agent| {
                        agent.inferred
                            || (stall_after.is_some()
                                && agent.status == Some(Status::Working)
                                && !agent.background)
                    })
                    .map(|agent| agent.pane_id.clone())
                    .collect();
                screens.keep(watched.iter().map(String::as_str));
                for agent in agents
                    .iter_mut()
                    .filter(|agent| watched.contains(&agent.pane_id))
                {
                    let changed = screens.changed_at(&agent.pane_id, now);
                    if agent.inferred {
                        let active =
                            changed.is_some_and(|changed| now.saturating_sub(changed) < ACTIVE_FOR);
                        let viewed = live
                            .iter()
                            .any(|pane| pane.pane_id == agent.pane_id && pane.viewed);
                        let status = infer_status(agent.status, active, viewed);
                        if status != agent.status {
                            agent.status = status;
                            agent.status_ts = now;
                            save(&dir, agent)?;
                        }
                    } else if let Some(stall_after) = stall_after {
                        let alive = changed.unwrap_or(now).max(agent.activity_ts);
                        agent.stalled = now.saturating_sub(alive) >= stall_after.as_secs();
                    }
                }
                if let Some(seen) = &seen {
                    for agent in entered(seen, &agents, &notify.statuses) {
                        send_notification(&notify.via, notification(&notify.message, agent));
                    }
                }
                seen = Some(
                    agents
                        .iter()
                        .map(|agent| (agent.pane_id.clone(), agent.status))
                        .collect(),
                );
                let rows = if agents.is_empty() {
                    let mut placeholder = Row::new("no agents");
                    placeholder.tone = Tone::Muted;
                    vec![placeholder]
                } else {
                    rows(&agents, now, options, &current)
                };
                let line = serde_json::to_string(&PluginMessage::Snapshot {
                    title: Some("Agents".to_string()),
                    rows,
                })?;
                if line != last {
                    writeln!(stdout, "{line}")?;
                    stdout.flush()?;
                    last = line;
                }
            }
            Err(error) => eprintln!("{error:#}"),
        }
        std::thread::sleep(POLL);
    }
}

/// When the visible content of each watched pane last changed, from hashes of its captures.
#[derive(Default)]
struct Screens(HashMap<String, (u64, Option<u64>)>);

impl Screens {
    /// Forgets every pane but `panes`, so a pane watched again starts fresh.
    fn keep<'a>(&mut self, panes: impl Iterator<Item = &'a str>) {
        let panes: Vec<&str> = panes.collect();
        self.0.retain(|pane, _| panes.contains(&pane.as_str()));
    }

    /// Captures `pane` and returns when its content last changed, or `None` until it changes
    /// for the first time.
    fn changed_at(&mut self, pane: &str, now: u64) -> Option<u64> {
        let content = tmux(&["capture-pane", "-p", "-t", pane]).unwrap_or_default();
        let mut hasher = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(&content, &mut hasher);
        let hash = std::hash::Hasher::finish(&hasher);
        let entry = self.0.entry(pane.to_string()).or_insert((hash, None));
        if entry.0 != hash {
            *entry = (hash, Some(now));
        }
        entry.1
    }
}

/// Agents of live panes, acknowledging the waiting or done ones being viewed, forgetting the
/// ones whose pane is gone and adding panes that run one of the `detect` commands.
fn current_agents(dir: &Path, live: &[LivePane], detect: &[String]) -> Result<Vec<AgentState>> {
    let mut result = reconcile(load_states(dir), live);
    for pane in live.iter().filter(|pane| detect.contains(&pane.command)) {
        if result
            .agents
            .iter()
            .any(|agent| agent.pane_id == pane.pane_id)
        {
            continue;
        }
        let now = now();
        let agent = AgentState {
            pane_id: pane.pane_id.clone(),
            pane_pid: pane.pane_pid,
            command: pane.command.clone(),
            project: project_name(Path::new(&pane.path)),
            window: pane.window_name.clone(),
            status: None,
            status_ts: now,
            activity_ts: now,
            title: pane.title.clone(),
            workdir: pane.path.clone(),
            window_id: pane.window_id.clone(),
            diff: None,
            stalled: false,
            inferred: true,
            background: false,
        };
        std::fs::create_dir_all(dir)?;
        save(dir, &agent)?;
        result.agents.push(agent);
    }
    for pane in &result.dropped {
        let _ = std::fs::remove_file(state_file(dir, pane));
    }
    for agent in &mut result.agents {
        agent.project = project_name(Path::new(&agent.workdir));
    }
    for agent in &result.agents {
        if result.acknowledged.contains(&agent.pane_id) {
            save(dir, agent)?;
        }
    }
    Ok(result.agents)
}

fn send_notification(via: &[Via], message: String) {
    let via = via.to_vec();
    std::thread::spawn(move || {
        if via.contains(&Via::Tmux) {
            let clients = tmux(&["list-clients", "-F", "#{client_name}"]).unwrap_or_default();
            for client in clients.lines() {
                let _ = tmux(&[
                    "display-message",
                    "-l",
                    "-d",
                    NOTIFICATION_MS,
                    "-c",
                    client,
                    &message,
                ]);
            }
        }
        if via.contains(&Via::Desktop) {
            let _ = if cfg!(target_os = "macos") {
                Process::new("osascript")
                    .args([
                        "-e",
                        "on run argv",
                        "-e",
                        "display notification (item 1 of argv) with title \"tmux-aside\"",
                        "-e",
                        "end run",
                        &message,
                    ])
                    .output()
            } else {
                Process::new("notify-send")
                    .args(["tmux-aside", &message])
                    .output()
            };
        }
    });
}

/// Branch of every agent in a git repository and its open pull request, keyed by pane id.
/// Branches come from `git`; pull requests from one `gh pr list` per repository, and a
/// repository where `gh` fails keeps its branches without pull requests.
fn agent_branches(agents: &[AgentState]) -> HashMap<String, Branch> {
    let mut branches = HashMap::new();
    let mut panes_by_repo: HashMap<String, Vec<String>> = HashMap::new();
    for agent in agents {
        let Ok(output) = git(
            &agent.workdir,
            &["rev-parse", "--show-toplevel", "--abbrev-ref", "HEAD"],
        ) else {
            continue;
        };
        let mut lines = output.lines();
        if let (Some(root), Some(branch)) = (lines.next(), lines.next()) {
            branches.insert(
                agent.pane_id.clone(),
                Branch {
                    name: branch.to_string(),
                    pr: None,
                },
            );
            panes_by_repo
                .entry(root.to_string())
                .or_default()
                .push(agent.pane_id.clone());
        }
    }
    for (root, panes) in panes_by_repo {
        let listed = Process::new("gh")
            .args([
                "pr",
                "list",
                "--state",
                "open",
                "--limit",
                "100",
                "--json",
                "number,headRefName,statusCheckRollup",
            ])
            .current_dir(&root)
            .output();
        let by_branch = match listed {
            Ok(output) if output.status.success() => {
                pull_requests_by_branch(&String::from_utf8_lossy(&output.stdout))
                    .unwrap_or_default()
            }
            Ok(output) => {
                eprintln!(
                    "gh pr list in {root}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                continue;
            }
            Err(error) => {
                eprintln!("gh pr list in {root}: {error}");
                continue;
            }
        };
        for pane in panes {
            if let Some(branch) = branches.get_mut(&pane) {
                branch.pr = by_branch.get(&branch.name).copied();
            }
        }
    }
    branches
}

/// Lines changed in `dir` since its branch left `origin/HEAD`, including uncommitted changes;
/// since `HEAD` when the remote's default branch is unknown.
fn branch_diff(dir: &str) -> Option<Diff> {
    let base = git(dir, &["merge-base", "origin/HEAD", "HEAD"]).unwrap_or_else(|_| "HEAD".into());
    parse_shortstat(&git(dir, &["diff", "--shortstat", &base]).ok()?)
}

fn git(dir: &str, args: &[&str]) -> Result<String> {
    let output = Process::new("git").arg("-C").arg(dir).args(args).output()?;
    if !output.status.success() {
        bail!("git {} failed in {dir}", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn live_panes() -> Result<Vec<LivePane>> {
    let output = tmux(&["list-panes", "-a", "-F", LIVE_PANE_FORMAT])?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.splitn(11, '\t').collect();
            let [
                pane_id,
                pid,
                command,
                pane_active,
                window_active,
                attached,
                path,
                window_id,
                host,
                window_name,
                title,
            ] = fields[..]
            else {
                return None;
            };
            Some(LivePane {
                pane_id: pane_id.to_string(),
                pane_pid: pid.parse().ok()?,
                command: command.to_string(),
                path: path.to_string(),
                window_id: window_id.to_string(),
                window_name: window_name.to_string(),
                title: if title == host { "" } else { title }.to_string(),
                viewed: pane_active == "1" && window_active == "1" && attached != "0",
            })
        })
        .collect())
}

fn load_states(dir: &Path) -> Vec<AgentState> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|content| serde_json::from_str(&content).ok())
        .collect()
}

fn save(dir: &Path, state: &AgentState) -> Result<()> {
    let file = state_file(dir, &state.pane_id);
    let temporary = file.with_extension(format!("json.{}", std::process::id()));
    std::fs::write(&temporary, serde_json::to_string(state)?)?;
    std::fs::rename(&temporary, &file)?;
    Ok(())
}

fn agents_dir() -> Result<PathBuf> {
    Ok(tmux_aside::config::state_dir()?.join("agents"))
}

fn state_file(dir: &Path, pane: &str) -> PathBuf {
    dir.join(format!("{}.json", pane.trim_start_matches('%')))
}

fn tmux(args: &[&str]) -> Result<String> {
    let output = Process::new("tmux")
        .args(args)
        .output()
        .context("failed to run tmux")?;
    if !output.status.success() {
        bail!(
            "tmux {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn print(stdout: &mut std::io::Stdout, message: &PluginMessage) -> Result<()> {
    writeln!(stdout, "{}", serde_json::to_string(message)?)?;
    stdout.flush()?;
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

const CLAUDE_HOOKS: [(&str, Option<&str>, &str); 6] = [
    (
        "SessionStart",
        Some("startup|resume|clear|fork"),
        "register",
    ),
    ("UserPromptSubmit", None, "set-status working"),
    ("PermissionRequest", None, "set-status waiting"),
    (
        "Notification",
        Some("permission_prompt|elicitation_dialog"),
        "set-status waiting",
    ),
    ("PostToolUse", None, "set-status working"),
    ("Stop", None, "set-status done"),
];

fn setup_claude() -> Result<()> {
    let dir = match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => std::env::home_dir()
            .ok_or_else(|| anyhow!("cannot locate the home directory"))?
            .join(".claude"),
    };
    let path = dir.join("settings.json");
    let existing = match std::fs::read_to_string(&path) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let mut settings: Value = match &existing {
        Some(content) => serde_json::from_str(content).with_context(|| {
            format!("{} is not valid JSON; nothing was changed", path.display())
        })?,
        None => json!({}),
    };
    let exe = std::env::current_exe()?;
    let exe = exe
        .to_str()
        .ok_or_else(|| anyhow!("executable path is not UTF-8"))?;
    merge_claude_hooks(&mut settings, &format!("'{}'", exe.replace('\'', "'\\''")))?;

    std::fs::create_dir_all(&dir)?;
    if let Some(content) = &existing {
        std::fs::write(path.with_extension("json.bak"), content)?;
    }
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&settings)?),
    )?;
    println!("Claude Code hooks installed in {}", path.display());
    println!("They take effect in new Claude Code sessions.");
    Ok(())
}

fn merge_claude_hooks(settings: &mut Value, exe: &str) -> Result<()> {
    let hooks = settings
        .as_object_mut()
        .ok_or_else(|| anyhow!("settings.json must contain a JSON object"))?
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("\"hooks\" in settings.json must be an object"))?;
    for (event, matcher, arguments) in CLAUDE_HOOKS {
        let command = format!("{exe} {arguments}");
        let entries = hooks
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| anyhow!("hooks.{event} in settings.json must be an array"))?;
        let mut found = false;
        for hook in entries
            .iter_mut()
            .filter_map(|entry| entry.get_mut("hooks").and_then(Value::as_array_mut))
            .flatten()
        {
            let ours = hook
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|existing| {
                    existing.contains("tmux-aside-agents") && existing.ends_with(arguments)
                });
            if ours {
                hook["command"] = json!(command);
                found = true;
            }
        }
        if !found {
            let mut entry = json!({ "hooks": [{ "type": "command", "command": command }] });
            if let Some(matcher) = matcher {
                entry["matcher"] = json!(matcher);
            }
            entries.push(entry);
        }
    }
    Ok(())
}
