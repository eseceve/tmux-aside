use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use tmux_aside::i18n::Language;
use tmux_aside::launch::{self, Launch};
use tmux_aside::protocol::{PLUGIN_PROTOCOL, PluginMessage};
use tmux_aside::tmux;
use tmux_aside_agents::{AgentState, agents_dir, state_file};
use tmux_aside_github::PullRequest;
use tmux_aside_watcher::{
    Followed, LivePane, Target, batch_query, github_repository, is_idle, messages, parse_batch,
    pending_nudge, rows,
};

const TICK: Duration = Duration::from_secs(1);
const SUBMIT_DELAY: Duration = Duration::from_millis(500);

/// Sidebar section following every launched pull request through its Lifecycle. Run it with
/// `mode: stream`.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// How often pull requests are fetched from GitHub, in one query for all of them
    #[arg(long, default_value = "30s", value_parser = tmux_aside::config::parse_duration)]
    interval: Duration,
    /// Section title [default: "Lifecycle", in the sidebar's language]
    #[arg(long)]
    title: Option<String>,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let language = Language::from_env();
    let title = cli
        .title
        .unwrap_or_else(|| messages(language).title.to_string());
    print(&PluginMessage::Hello {
        protocol: PLUGIN_PROTOCOL,
        features: vec![],
    })?;
    std::thread::spawn(|| {
        for _ in std::io::stdin().lock().lines() {}
        std::process::exit(0);
    });
    let mut pull_requests: HashMap<String, PullRequest> = HashMap::new();
    let mut fetched: Option<(Instant, Vec<String>)> = None;
    let mut last = String::new();
    loop {
        let mut launches: Vec<Launch> = launch::list()?
            .into_iter()
            .filter(|launch| launch.opens_pull_request)
            .collect();
        let ids: Vec<String> = launches.iter().map(|launch| launch.id.clone()).collect();
        let due = fetched
            .as_ref()
            .is_none_or(|(at, known)| at.elapsed() >= cli.interval || *known != ids);
        if due {
            pull_requests = fetch(&launches, language)?;
            record_pull_requests(&mut launches, &pull_requests)?;
            fetched = Some((Instant::now(), ids));
        }
        let mut followed: Vec<Followed> = launches
            .into_iter()
            .map(|launch| Followed {
                pull_request: pull_requests.get(&launch.id).cloned(),
                launch,
            })
            .collect();
        for followed in &mut followed {
            nudge(followed)?;
        }
        let snapshot = serde_json::to_string(&PluginMessage::Snapshot {
            title: Some(title.clone()),
            summary: None,
            rows: rows(&followed, language),
        })?;
        if snapshot != last {
            println!("{snapshot}");
            last = snapshot;
        }
        std::thread::sleep(TICK.min(cli.interval));
    }
}

/// The open pull request of every Launch whose clone has a GitHub origin, keyed by Launch id.
fn fetch(launches: &[Launch], language: Language) -> Result<HashMap<String, PullRequest>> {
    let targets: Vec<(&Launch, Target)> = launches
        .iter()
        .filter_map(|launch| {
            let (owner, name) = github_repository(&origin_url(&launch.repository)?)?;
            Some((launch, Target::new(&owner, &name, &launch.branch)))
        })
        .collect();
    if targets.is_empty() {
        return Ok(HashMap::new());
    }
    let (query, variables) = batch_query(
        &targets
            .iter()
            .map(|(_, target)| target.clone())
            .collect::<Vec<_>>(),
    );
    let mut command = Command::new("gh");
    command
        .args(["api", "graphql", "-f"])
        .arg(format!("query={query}"));
    for (name, value) in variables {
        command.arg("-f").arg(format!("{name}={value}"));
    }
    let output = command
        .output()
        .context(messages(language).failed_to_run_gh)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() && !stdout.contains("\"data\"") {
        bail!("gh: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    let found = parse_batch(&stdout, targets.len())?;
    Ok(targets
        .into_iter()
        .zip(found)
        .filter_map(|((launch, _), pull_request)| Some((launch.id.clone(), pull_request?)))
        .collect())
}

/// Writes the URL of each newly found pull request into its Launch, for `tmux-aside status`.
fn record_pull_requests(
    launches: &mut [Launch],
    pull_requests: &HashMap<String, PullRequest>,
) -> Result<()> {
    for launch in launches {
        if let Some(pr) = pull_requests.get(&launch.id)
            && launch.pull_request.as_deref() != Some(pr.url.as_str())
        {
            launch.pull_request = Some(pr.url.clone());
            launch::save(launch)?;
        }
    }
    Ok(())
}

/// Types the pending Nudge of `followed` into its agent's pane if the agent is idle, and
/// records it so that it is not typed again for the same head commit.
///
/// The Nudge is recorded before it is typed, so that a failure or a restart in between can
/// lose it but never type it twice. A pane that cannot be typed into does not stop the Watcher.
fn nudge(followed: &mut Followed) -> Result<()> {
    let Some((key, nudge)) = pending_nudge(followed) else {
        return Ok(());
    };
    let pane = followed.launch.pane.clone();
    if !agent_is_idle(&pane) {
        return Ok(());
    }
    followed.launch.nudges.push(key);
    launch::save(&followed.launch)?;
    let typed = tmux::run(&["send-keys", "-t", &pane, "-l", "--", &nudge.text]).and_then(|_| {
        std::thread::sleep(SUBMIT_DELAY);
        tmux::run(&["send-keys", "-t", &pane, "Enter"])
    });
    if let Err(error) = typed {
        eprintln!("failed to nudge {pane}: {error:#}");
    }
    Ok(())
}

fn agent_is_idle(pane: &str) -> bool {
    let Some(state) = agents_dir()
        .ok()
        .and_then(|dir| std::fs::read_to_string(state_file(&dir, pane)).ok())
        .and_then(|json| serde_json::from_str::<AgentState>(&json).ok())
    else {
        return false;
    };
    let Ok(live) = tmux::run(&[
        "display-message",
        "-p",
        "-t",
        pane,
        "#{pane_pid}\t#{pane_current_command}\t#{pane_active}\t#{window_active}\t#{session_attached}",
    ]) else {
        return false;
    };
    let fields: Vec<&str> = live.split('\t').collect();
    let [pid, command, pane_active, window_active, attached] = fields[..] else {
        return false;
    };
    pid.parse().is_ok_and(|pid| {
        is_idle(
            &state,
            &LivePane {
                pid,
                command: command.to_string(),
                viewed: pane_active == "1" && window_active == "1" && attached != "0",
            },
        )
    })
}

fn origin_url(repository: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn print(message: &PluginMessage) -> Result<()> {
    println!("{}", serde_json::to_string(message)?);
    Ok(())
}
