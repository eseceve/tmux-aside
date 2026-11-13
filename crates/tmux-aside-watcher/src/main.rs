use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use tmux_aside::config::{self, CriterionCommand};
use tmux_aside::i18n::Language;
use tmux_aside::launch::{self, Launch};
use tmux_aside::protocol::{PLUGIN_PROTOCOL, PluginMessage};
use tmux_aside::tmux;
use tmux_aside_agents::{AgentState, agents_dir, state_file};
use tmux_aside_github::PullRequest;
use tmux_aside_watcher::{
    Answer, Answers, Followed, LivePane, Question, Stage, Target, Worktree, batch_query,
    choose_pull_request, github_repository, is_idle, messages, parse_batch, pending_nudge,
    ready_to_merge, rows, should_clean_up,
};

const TICK: Duration = Duration::from_secs(1);
const SUBMIT_DELAY: Duration = Duration::from_millis(500);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const CRITERION_POLL: Duration = Duration::from_millis(20);

/// Sidebar section following every launched pull request through its Lifecycle. Run it with
/// `mode: stream`.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<WatcherCommand>,
    /// How often pull requests are fetched from GitHub, in one query for all of them
    #[arg(long, default_value = "30s", value_parser = tmux_aside::config::parse_duration)]
    interval: Duration,
    /// Section title [default: "Lifecycle", in the sidebar's language]
    #[arg(long)]
    title: Option<String>,
}

#[derive(Subcommand)]
enum WatcherCommand {
    /// Run a default Criterion: read a question as JSON on stdin and print the answer
    Criterion {
        #[arg(value_enum)]
        stage: DefaultCriterion,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum DefaultCriterion {
    ReadyToMerge,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(WatcherCommand::Criterion { stage }) => answer(stage),
        None => run(cli),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn answer(stage: DefaultCriterion) -> Result<()> {
    let language = Language::from_env();
    let question: Question = serde_json::from_reader(std::io::stdin().lock())
        .context("expected a Criterion question as JSON on stdin")?;
    let answer = match stage {
        DefaultCriterion::ReadyToMerge => ready_to_merge(&question.pull_request, language),
    };
    println!("{}", serde_json::to_string(&answer)?);
    Ok(())
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
    let mut latest = Refresh::default();
    let mut refreshing: Option<Receiver<Result<Refresh>>> = None;
    let mut fetched: Option<(Instant, Vec<String>)> = None;
    let mut last = String::new();
    loop {
        let launches: Vec<Launch> = launch::list()?
            .into_iter()
            .filter(|launch| launch.opens_pull_request)
            .collect();
        if let Some(receiver) = &refreshing {
            match receiver.try_recv() {
                Ok(refresh) => {
                    latest = refresh?;
                    refreshing = None;
                }
                Err(TryRecvError::Disconnected) => refreshing = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let ids: Vec<String> = launches.iter().map(|launch| launch.id.clone()).collect();
        let due = fetched
            .as_ref()
            .is_none_or(|(at, known)| at.elapsed() >= cli.interval || *known != ids);
        if due && refreshing.is_none() {
            let (sender, receiver) = mpsc::channel();
            let snapshot = launches.clone();
            std::thread::spawn(move || {
                let _ = sender.send(refresh(&snapshot, language));
            });
            refreshing = Some(receiver);
            fetched = Some((Instant::now(), ids));
        }
        let Refresh {
            pull_requests,
            answers,
        } = &latest;
        let mut followed: Vec<Followed> = launches
            .into_iter()
            .map(|launch| {
                let pull_request = pull_requests.get(&launch.id).cloned();
                Followed {
                    worktree: pull_request.as_ref().filter(|pr| pr.finished()).map(
                        |pr| match launch.worktree == launch.repository {
                            true => Worktree::Main,
                            false => inspect_worktree(&launch.worktree, pr.head.as_deref()),
                        },
                    ),
                    answers: answers.get(&launch.id).cloned().unwrap_or_default(),
                    pull_request,
                    launch,
                }
            })
            .collect();
        for followed in &mut followed {
            nudge(followed, language)?;
            if should_clean_up(followed) && clean_up(&followed.launch) {
                followed.worktree = Some(Worktree::Missing);
            }
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

/// What one fetch learned: the pull request of each Launch and its Criteria's answers, both
/// keyed by Launch id.
#[derive(Default)]
struct Refresh {
    pull_requests: HashMap<String, PullRequest>,
    answers: HashMap<String, Answers>,
}

/// Fetches the launched pull requests, records newly found ones and asks their Criteria. Runs
/// off the main loop, so that a slow Criterion never stops the rows from updating.
fn refresh(launches: &[Launch], language: Language) -> Result<Refresh> {
    let pull_requests = fetch(launches, language)?;
    record_pull_requests(launches, &pull_requests)?;
    let answers = match criteria() {
        Ok(criteria) => ask_criteria(&pull_requests, &criteria, language),
        Err(error) => {
            let error = (messages(language).invalid_config)(&format!("{error:#}"));
            pull_requests
                .keys()
                .map(|id| (id.clone(), vec![(Stage::ReadyToMerge, Err(error.clone()))]))
                .collect()
        }
    };
    Ok(Refresh {
        pull_requests,
        answers,
    })
}

/// The command of each stage's Criterion after Implementing, in Lifecycle order: the one in
/// the config, or the default one, which this binary runs.
fn criteria() -> Result<Vec<(Stage, Vec<String>, Duration)>> {
    let configured = config::load(&config::default_path()?)?
        .map(|config| config.criteria)
        .unwrap_or_default();
    let exe = std::env::current_exe()
        .map(|exe| exe.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "tmux-aside-watcher".to_string());
    let default = |name: &str| {
        (
            vec![exe.clone(), "criterion".to_string(), name.to_string()],
            DEFAULT_TIMEOUT,
        )
    };
    let command = |configured: Option<CriterionCommand>, name: &str| match configured {
        Some(criterion) => (
            [vec![criterion.command], criterion.args].concat(),
            criterion.timeout,
        ),
        None => default(name),
    };
    let (ready_command, ready_timeout) = command(configured.ready_to_merge, "ready-to-merge");
    Ok(vec![(Stage::ReadyToMerge, ready_command, ready_timeout)])
}

/// Asks the Criteria of every open pull request, all pull requests at once, each one's stages
/// in order until one is unmet or fails, keyed by Launch id.
fn ask_criteria(
    pull_requests: &HashMap<String, PullRequest>,
    criteria: &[(Stage, Vec<String>, Duration)],
    language: Language,
) -> HashMap<String, Answers> {
    std::thread::scope(|scope| {
        let asked: Vec<_> = pull_requests
            .iter()
            .filter(|(_, pr)| !pr.finished())
            .map(|(id, pr)| {
                let handle = scope.spawn(move || {
                    let mut answers = Vec::new();
                    for (stage, command, timeout) in criteria {
                        let question = Question {
                            stage: stage.key().to_string(),
                            url: pr.url.clone(),
                            pull_request: pr.clone(),
                        };
                        let answer = ask(command, &question, *timeout, language);
                        let go_on = matches!(&answer, Ok(answer) if answer.met);
                        answers.push((*stage, answer));
                        if !go_on {
                            break;
                        }
                    }
                    answers
                });
                (id.clone(), handle)
            })
            .collect();
        asked
            .into_iter()
            .map(|(id, handle)| {
                let answers = handle.join().unwrap_or_else(|_| {
                    let error = (messages(language).failed_to_start)("criterion");
                    vec![(Stage::ReadyToMerge, Err(error))]
                });
                (id, answers)
            })
            .collect()
    })
}

/// Runs one Criterion `command` with `question` on its stdin. After `timeout` it kills the
/// Criterion with every process it started, which share its process group.
fn ask(
    command: &[String],
    question: &Question,
    timeout: Duration,
    language: Language,
) -> Result<Answer, String> {
    let messages = messages(language);
    let (program, args) = command
        .split_first()
        .ok_or_else(|| (messages.failed_to_start)("criterion"))?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .process_group(0)
        .spawn()
        .map_err(|_| (messages.failed_to_start)(program))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = serde_json::to_writer(&mut stdin, question);
    }
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let (sender, output) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        let _ = sender.send(text);
    });
    let deadline = Instant::now() + timeout;
    let timed_out = |child: &mut std::process::Child| {
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{}", child.id())])
            .status();
        let _ = child.wait();
        Err((messages.timed_out)(timeout.as_secs_f32()))
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(CRITERION_POLL),
            _ => return timed_out(&mut child),
        }
    };
    let left = deadline.saturating_duration_since(Instant::now());
    let Ok(output) = output.recv_timeout(left.max(CRITERION_POLL)) else {
        return timed_out(&mut child);
    };
    if !status.success() {
        return Err((messages.exited)(status.code().unwrap_or(-1)));
    }
    serde_json::from_str(output.trim()).map_err(|_| {
        let preview: String = output.trim().chars().take(60).collect();
        (messages.invalid_answer)(&preview)
    })
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
        .filter_map(|((launch, _), candidates)| {
            let local_head = git_output(&launch.worktree, &["rev-parse", "HEAD"]);
            let pull_request = choose_pull_request(
                candidates,
                launch.pull_request.as_deref(),
                local_head.as_deref(),
            )?;
            Some((launch.id.clone(), pull_request))
        })
        .collect())
}

/// Writes the URL of each newly found pull request into its Launch, for `tmux-aside status`.
fn record_pull_requests(
    launches: &[Launch],
    pull_requests: &HashMap<String, PullRequest>,
) -> Result<()> {
    for launch in launches {
        if let Some(pr) = pull_requests.get(&launch.id)
            && launch.pull_request.as_deref() != Some(pr.url.as_str())
        {
            launch::update(&launch.id, |launch| {
                launch.pull_request = Some(pr.url.clone());
            })?;
        }
    }
    Ok(())
}

/// Types the pending Nudge of `followed` into its agent's pane if the agent is idle, and
/// records it so that it is not typed again for the same head commit.
///
/// The Nudge is recorded before it is typed, so that a failure or a restart in between can
/// lose it but never type it twice. A pane that cannot be typed into does not stop the Watcher.
fn nudge(followed: &mut Followed, language: Language) -> Result<()> {
    let Some((key, nudge)) = pending_nudge(followed, language) else {
        return Ok(());
    };
    let pane = followed.launch.pane.clone();
    if !agent_is_idle(&pane) {
        return Ok(());
    }
    followed.launch.nudges.push(key.clone());
    launch::update(&followed.launch.id, |launch| launch.nudges.push(key))?;
    let typed = tmux::run(&["send-keys", "-t", &pane, "-l", "--", &nudge]).and_then(|_| {
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

/// What removing `worktree` would lose. Commits are pushed when the branch is at the pull
/// request's `head` or when a remote branch holds them.
fn inspect_worktree(worktree: &Path, head: Option<&str>) -> Worktree {
    let git = |args: &[&str]| git_output(worktree, args);
    if !worktree.exists() {
        return Worktree::Missing;
    }
    let Some(changes) = git(&["status", "--porcelain"]) else {
        return Worktree::Missing;
    };
    if !changes.is_empty() {
        return Worktree::Uncommitted;
    }
    if head.is_some() && git(&["rev-parse", "HEAD"]).as_deref() == head {
        return Worktree::Clean;
    }
    match git(&["rev-list", "--count", "HEAD", "--not", "--remotes"]).as_deref() {
        Some("0") => Worktree::Clean,
        _ => Worktree::Unpushed,
    }
}

/// Removes the Launch's worktree, then closes the agent's pane when it is still the one
/// launched there, which closes its window unless someone split it. Returns whether the worktree was removed; failures go to stderr.
fn clean_up(launch: &Launch) -> bool {
    let worktree = launch
        .worktree
        .canonicalize()
        .unwrap_or_else(|_| launch.worktree.clone());
    let agent_still_there = tmux::run(&[
        "display-message",
        "-p",
        "-t",
        &launch.pane,
        "#{window_id}\t#{pane_current_path}",
    ])
    .ok()
    .and_then(|live| {
        let (window, path) = live.split_once('\t')?;
        Some(window == launch.window && Path::new(path).starts_with(&worktree))
    })
    .unwrap_or(false);
    let removed = Command::new("git")
        .arg("-C")
        .arg(&launch.repository)
        .args(["worktree", "remove"])
        .arg(&launch.worktree)
        .output();
    match removed {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            eprintln!(
                "failed to remove {}: {}",
                launch.worktree.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return false;
        }
        Err(error) => {
            eprintln!("failed to remove {}: {error}", launch.worktree.display());
            return false;
        }
    }
    if agent_still_there && let Err(error) = tmux::run(&["kill-pane", "-t", &launch.pane]) {
        eprintln!("failed to close {}: {error:#}", launch.pane);
    }
    true
}

fn origin_url(repository: &Path) -> Option<String> {
    git_output(repository, &["remote", "get-url", "origin"])
}

/// The trimmed output of `git` run in `directory`, or `None` when it fails.
fn git_output(directory: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
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
