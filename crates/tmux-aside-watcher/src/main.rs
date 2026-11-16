use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use tmux_aside::config::{self, Criteria, CriterionCommand, DiscoverySource};
use tmux_aside::i18n::Language;
use tmux_aside::launch::{self, Launch, Progress};
use tmux_aside::protocol::{PLUGIN_PROTOCOL, PluginMessage};
use tmux_aside::tmux;
use tmux_aside::{cleanup, pack};
use tmux_aside_agents::{AgentState, agents_dir, state_file};
use tmux_aside_github::{PullRequest, parse_search, search_query};
use tmux_aside_watcher::{
    Answer, Answers, Followed, LivePane, Question, Stage, Target, Worktree, activity,
    already_followed, ask_in_order, batch_query, choose_pull_request, deployed, discovered,
    due_for_qa, evaluate, github_repository, is_idle, leaves, messages, parse_batch, pending_nudge,
    qa, ready_to_deploy, ready_to_merge, rows, should_clean_up,
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
    /// The tmux-aside command that launches QA [default: the one next to this binary, or in PATH]
    #[arg(long, value_name = "PATH")]
    tmux_aside: Option<String>,
}

#[derive(Subcommand)]
enum WatcherCommand {
    /// Run a default Criterion: read a question as JSON on stdin and print the answer
    Criterion {
        #[arg(value_enum)]
        stage: DefaultCriterion,
        /// Label that QA passed, for ready-to-deploy
        #[arg(long, default_value = "qa:passed")]
        passed_label: String,
        /// Label that QA failed, for ready-to-deploy
        #[arg(long, default_value = "qa:failed")]
        failed_label: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum DefaultCriterion {
    ReadyToMerge,
    Qa,
    ReadyToDeploy,
    Deployed,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(WatcherCommand::Criterion {
            stage,
            passed_label,
            failed_label,
        }) => answer(stage, &passed_label, &failed_label),
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

fn answer(stage: DefaultCriterion, passed_label: &str, failed_label: &str) -> Result<()> {
    let language = Language::from_env();
    let question: Question = serde_json::from_reader(std::io::stdin().lock())
        .context("expected a Criterion question as JSON on stdin")?;
    let answer = match stage {
        DefaultCriterion::ReadyToMerge => ready_to_merge(&question.pull_request, language),
        DefaultCriterion::Qa => qa(&question.pull_request, language),
        DefaultCriterion::ReadyToDeploy => {
            ready_to_deploy(&question.pull_request, passed_label, failed_label, language)
        }
        DefaultCriterion::Deployed => deployed(language),
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
    let tmux_aside = cli.tmux_aside.unwrap_or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.join("tmux-aside")))
            .filter(|sibling| sibling.is_file())
            .map_or_else(
                || "tmux-aside".to_string(),
                |sibling| sibling.to_string_lossy().into_owned(),
            )
    });
    let mut latest = Refresh::default();
    let mut refreshing: Option<Receiver<Result<Refresh>>> = None;
    let mut fetched: Option<(Instant, Vec<String>)> = None;
    let mut last = String::new();
    let mut recorded_progress: BTreeMap<String, Progress> = BTreeMap::new();
    loop {
        let all_launches = launch::list()?;
        let protected: Vec<String> = all_launches
            .iter()
            .filter(|launch| !launch.opens_pull_request && !launch.pane.is_empty())
            .map(|launch| launch.pane.clone())
            .collect();
        let mut launches: Vec<Launch> = all_launches
            .into_iter()
            .filter(|launch| launch.opens_pull_request)
            .collect();
        for launch in &mut launches {
            if protected.contains(&launch.pane) {
                launch::update(&launch.id, |record| {
                    record.pane.clear();
                    record.window.clear();
                    record.session.clear();
                })?;
                launch.pane.clear();
                launch.window.clear();
                launch.session.clear();
            }
        }
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
            auto_qa,
            on_deployed,
        } = &latest;
        let mut followed: Vec<Followed> = launches
            .into_iter()
            .map(|launch| {
                let pull_request = pull_requests.get(&launch.id).cloned();
                Followed {
                    worktree: pull_request.as_ref().filter(|pr| pr.finished()).map(
                        |pr| match launch.keeps_worktree || launch.worktree == launch.repository {
                            true => Worktree::Main,
                            false => cleanup::inspect(&launch.worktree, pr.head.as_deref()),
                        },
                    ),
                    answers: answers.get(&launch.id).cloned().unwrap_or_default(),
                    pull_request,
                    launch,
                }
            })
            .collect();
        for followed in &mut followed {
            if !protected.contains(&followed.launch.pane) {
                nudge(followed, language)?;
            }
            if should_clean_up(followed) && clean_up(&followed.launch) {
                followed.worktree = Some(Worktree::Missing);
            }
        }
        if *on_deployed {
            run_on_deployed(&mut followed, &tmux_aside, language);
        }
        let (gone, mut followed): (Vec<Followed>, Vec<Followed>) = followed
            .into_iter()
            .partition(|followed| leaves(followed, language));
        for followed in gone {
            launch::forget(&followed.launch.id)?;
        }
        if *auto_qa {
            launch_due_qa(&mut followed, &tmux_aside, language)?;
        }
        let progress: BTreeMap<String, Progress> = followed
            .iter()
            .map(|followed| {
                let evaluation = evaluate(followed, language);
                let progress = Progress {
                    stage: evaluation.stage.key().to_string(),
                    stage_name: evaluation.stage.name(language).to_string(),
                    summary: evaluation.summary,
                };
                (followed.launch.id.clone(), progress)
            })
            .collect();
        if progress != recorded_progress {
            match launch::record_progress(&progress) {
                Ok(()) => recorded_progress = progress,
                Err(error) => eprintln!("{error:#}"),
            }
        }
        let snapshot = serde_json::to_string(&PluginMessage::Snapshot {
            title: Some(title.clone()),
            summary: None,
            rows: rows(&followed, &tmux_aside, language),
        })?;
        if snapshot != last {
            println!("{snapshot}");
            last = snapshot;
        }
        std::thread::sleep(TICK.min(cli.interval));
    }
}

/// Launches QA for every Group ready for it whose pull requests had no QA launched yet, with
/// `tmux-aside qa`, whose output stays off the plugin's stdout. Records it first, so that a
/// failure never launches it twice.
fn launch_due_qa(followed: &mut [Followed], tmux_aside: &str, language: Language) -> Result<()> {
    for (urls, ids) in due_for_qa(followed, language) {
        let launched = urls.join(" ");
        for member in followed
            .iter_mut()
            .filter(|member| ids.contains(&member.launch.id))
        {
            member.launch.auto_qa = Some(launched.clone());
            let launched = launched.clone();
            if let Err(error) =
                launch::update(&member.launch.id, |launch| launch.auto_qa = Some(launched))
            {
                eprintln!("{error:#}");
            }
        }
        let started = Command::new(tmux_aside)
            .arg("qa")
            .args(&urls)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn();
        match started {
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
            }
            Err(error) => eprintln!("{tmux_aside} qa {launched}: {error}"),
        }
    }
    Ok(())
}

/// Runs the `on_deployed` action once, with `tmux-aside deployed` in the clone of the first
/// one, for every pull request deployed that it did not run for yet, whose output stays off
/// the plugin's stdout. Records it first, so that a failure never runs it twice.
fn run_on_deployed(followed: &mut [Followed], tmux_aside: &str, language: Language) {
    let mut command = Command::new(tmux_aside);
    command.arg("deployed");
    let mut urls = Vec::new();
    let mut directory = None;
    for member in followed.iter_mut().filter(|member| {
        !member.launch.ran_on_deployed && evaluate(member, language).stage == Stage::Deployed
    }) {
        let Some(pull_request) = &member.pull_request else {
            continue;
        };
        urls.push(pull_request.url.clone());
        command.args(["--task", &member.launch.task]);
        directory.get_or_insert_with(|| member.launch.repository.clone());
        member.launch.ran_on_deployed = true;
        if let Err(error) = launch::update(&member.launch.id, |launch| {
            launch.ran_on_deployed = true;
        }) {
            eprintln!("{error:#}");
        }
    }
    if urls.is_empty() {
        return;
    }
    if let Some(directory) = directory.filter(|directory| directory.is_dir()) {
        command.current_dir(directory);
    }
    let started = command
        .args(&urls)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn();
    match started {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(error) => eprintln!("{tmux_aside} deployed {}: {error}", urls.join(" ")),
    }
}

/// What one fetch learned: the pull request of each Launch and its Criteria's answers, both
/// keyed by Launch id.
#[derive(Default)]
struct Refresh {
    pull_requests: HashMap<String, PullRequest>,
    answers: HashMap<String, Answers>,
    /// The config asks the Watcher to launch QA by itself.
    auto_qa: bool,
    /// The config or the Pack has an action for deployed pull requests.
    on_deployed: bool,
}

/// Fetches the launched pull requests, records newly found ones and asks their Criteria. Runs
/// off the main loop, so that a slow Criterion never stops the rows from updating.
fn refresh(launches: &[Launch], language: Language) -> Result<Refresh> {
    let pull_requests = fetch(launches, language)?;
    record_pull_requests(launches, &pull_requests)?;
    link_agents(launches, &pull_requests);
    let config = config::load(&config::default_path()?);
    if let Ok(Some(config)) = &config {
        discover(&config.discover, launches);
    }
    let auto_qa = matches!(&config, Ok(Some(config)) if config.auto_qa);
    let mut on_deployed = false;
    let configured = config.map(|config| {
        config
            .map(|config| {
                let pack = pack::configured(&config).unwrap_or_else(|error| {
                    eprintln!(
                        "{}",
                        (messages(language).invalid_pack)(&format!("{error:#}"))
                    );
                    None
                });
                on_deployed = pack::on_deployed(&config, pack.as_ref()).is_some();
                pack::criteria(&config, pack.as_ref())
            })
            .unwrap_or_default()
    });
    let agents: HashMap<String, String> = launches
        .iter()
        .filter_map(|launch| {
            let (state, live) = live_agent(&launch.pane)?;
            Some((launch.id.clone(), activity(&state, &live)?.to_string()))
        })
        .collect();
    let answers = match configured.map(criteria) {
        Ok(criteria) => ask_criteria(&pull_requests, &agents, &criteria, language),
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
        auto_qa,
        on_deployed,
    })
}

/// The command of each stage's Criterion after Implementing, in Lifecycle order: the one in
/// the config, or the default one, which this binary runs.
fn criteria(configured: Criteria) -> Vec<(Stage, Vec<String>, Duration)> {
    let exe = std::env::current_exe()
        .map(|exe| exe.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "tmux-aside-watcher".to_string());
    let default = |name: &str| vec![exe.clone(), "criterion".to_string(), name.to_string()];
    let command = |stage: Stage, configured: Option<CriterionCommand>, name: &str| match configured
    {
        Some(criterion) => {
            let program = criterion
                .command
                .map_or_else(|| default(name), |command| vec![command]);
            (stage, [program, criterion.args].concat(), criterion.timeout)
        }
        None => (stage, default(name), DEFAULT_TIMEOUT),
    };
    vec![
        command(
            Stage::ReadyToMerge,
            configured.ready_to_merge,
            "ready-to-merge",
        ),
        command(Stage::Qa, configured.qa, "qa"),
        command(
            Stage::ReadyToDeploy,
            configured.ready_to_deploy,
            "ready-to-deploy",
        ),
        command(Stage::Deployed, configured.deployed, "deployed"),
    ]
}

/// Asks the Criteria of every open pull request, all pull requests at once, each one's stages
/// in order until one is unmet or fails, keyed by Launch id.
fn ask_criteria(
    pull_requests: &HashMap<String, PullRequest>,
    agents: &HashMap<String, String>,
    criteria: &[(Stage, Vec<String>, Duration)],
    language: Language,
) -> HashMap<String, Answers> {
    std::thread::scope(|scope| {
        let asked: Vec<_> = pull_requests
            .iter()
            .filter(|(_, pr)| !pr.closed)
            .map(|(id, pr)| {
                let handle = scope.spawn(move || {
                    let stages: Vec<Stage> = criteria.iter().map(|(stage, _, _)| *stage).collect();
                    ask_in_order(&stages, pr, |stage| {
                        let (_, command, timeout) = criteria
                            .iter()
                            .find(|(of, _, _)| *of == stage)
                            .expect("a Criterion per stage");
                        let question = Question {
                            stage: stage.key().to_string(),
                            url: pr.url.clone(),
                            pull_request: pr.clone(),
                            agent: agents.get(id).cloned(),
                        };
                        ask(command, &question, *timeout, language)
                    })
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
            if let Some(target) = launch
                .pull_request
                .as_deref()
                .and_then(Target::pull_request)
            {
                return Some((launch, target));
            }
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
    for (name, value, integer) in variables {
        command
            .arg(if integer { "-F" } else { "-f" })
            .arg(format!("{name}={value}"));
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
            && (launch.pull_request.as_deref() != Some(pr.url.as_str()) || launch.branch.is_empty())
        {
            launch::update(&launch.id, |launch| {
                launch.pull_request = Some(pr.url.clone());
                if launch.branch.is_empty() {
                    launch.branch = pr.branch.clone();
                }
            })?;
        }
    }
    Ok(())
}

/// Records every pull request the `sources` find that no Launch follows yet, so that the Watcher
/// follows it from the next fetch. A source that fails is reported and skipped.
fn discover(sources: &[DiscoverySource], launches: &[Launch]) {
    for source in sources {
        let found = match source {
            DiscoverySource::Search { search } => search_pull_requests(search),
            DiscoverySource::Command { command, args } => listed_pull_requests(command, args),
        };
        let found = match found {
            Ok(found) => found,
            Err(error) => {
                eprintln!("discover: {error:#}");
                continue;
            }
        };
        for (url, repository, branch) in found {
            if already_followed(launches, &url, &repository, &branch) {
                continue;
            }
            let Some(launch) = discovered(&url) else {
                continue;
            };
            if launches.iter().any(|known| known.id == launch.id) {
                continue;
            }
            if let Err(error) = launch::save(&launch) {
                eprintln!("discover: {error:#}");
            }
        }
    }
}

/// The URL, repository name and branch of the pull requests a GitHub `search` finds.
fn search_pull_requests(search: &str) -> Result<Vec<(String, String, String)>> {
    let output = Command::new("gh")
        .args(["api", "graphql", "-f"])
        .arg(format!("query={}", search_query()))
        .arg("-f")
        .arg(format!("q={search}"))
        .args(["-F", "limit=50"])
        .output()
        .context("failed to run gh")?;
    if !output.status.success() {
        bail!("gh: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(parse_search(&String::from_utf8_lossy(&output.stdout))?
        .into_iter()
        .map(|pr| (pr.url, pr.repository, pr.branch))
        .collect())
}

/// The pull request URLs `command` prints, one per line.
fn listed_pull_requests(command: &str, args: &[String]) -> Result<Vec<(String, String, String)>> {
    let output = Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("failed to run {command}"))?;
    if !output.status.success() {
        bail!(
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("/pull/"))
        .map(|url| (url.to_string(), String::new(), String::new()))
        .collect())
}

/// Gives each Launch the Watcher follows without a live agent (a discovered pull request, or a
/// followed one whose agent is gone) the agent that works on its pull request's branch, in a
/// clone of its repository, when there is one no other Launch has.
fn link_agents(launches: &[Launch], pull_requests: &HashMap<String, PullRequest>) {
    let Ok(dir) = agents_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let agents: Vec<AgentState> = entries
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|json| serde_json::from_str(&json).ok())
        .collect();
    let all_launches = launch::list().unwrap_or_else(|_| launches.to_vec());
    let mut taken: Vec<String> = all_launches
        .iter()
        .filter(|launch| !launch.pane.is_empty())
        .map(|launch| launch.pane.clone())
        .collect();
    for launch in launches.iter().filter(|launch| launch.keeps_worktree) {
        if !launch.pane.is_empty() && live_agent(&launch.pane).is_some() {
            continue;
        }
        let (Some(pr), Some(target)) = (
            pull_requests.get(&launch.id),
            launch
                .pull_request
                .as_deref()
                .and_then(Target::pull_request),
        ) else {
            continue;
        };
        let found = agents.iter().find(|agent| {
            !taken.contains(&agent.pane_id)
                && live_agent(&agent.pane_id).is_some()
                && works_on(
                    Path::new(&agent.workdir),
                    &target.owner,
                    &target.name,
                    &pr.branch,
                )
        });
        let Some(agent) = found else {
            continue;
        };
        let workdir = Path::new(&agent.workdir);
        let worktree = git_output(workdir, &["rev-parse", "--show-toplevel"]).map(PathBuf::from);
        let repository = git_output(
            workdir,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .and_then(|common| Some(Path::new(&common).parent()?.to_path_buf()));
        let window = tmux::run(&[
            "display-message",
            "-p",
            "-t",
            &agent.pane_id,
            "#{window_id}",
        ])
        .unwrap_or_default();
        taken.push(agent.pane_id.clone());
        let result = launch::update(&launch.id, |launch| {
            launch.pane = agent.pane_id.clone();
            launch.window = window;
            if let Some(worktree) = worktree {
                launch.worktree = worktree;
            }
            if let Some(repository) = repository {
                launch.repository = repository;
            }
        });
        if let Err(error) = result {
            eprintln!("{error:#}");
        }
    }
}

/// Whether `workdir` is a clone of `owner/name` on `branch`.
fn works_on(workdir: &Path, owner: &str, name: &str, branch: &str) -> bool {
    !branch.is_empty()
        && git_output(workdir, &["rev-parse", "--abbrev-ref", "HEAD"]).as_deref() == Some(branch)
        && git_output(workdir, &["config", "--get", "remote.origin.url"])
            .and_then(|origin| github_repository(&origin))
            == Some((owner.to_string(), name.to_string()))
}

/// Types the pending Nudge of `followed` into its agent's pane if the agent is idle, and
/// records it so that it is not typed again for the same head commit.
///
/// Record only successful deliveries so a failed send can be retried. A crash between
/// submission and persistence can cause a duplicate; delivery is not exactly-once.
fn nudge(followed: &mut Followed, language: Language) -> Result<()> {
    if followed.launch.pane.is_empty() {
        return Ok(());
    }
    let Some((key, nudge)) = pending_nudge(followed, language) else {
        return Ok(());
    };
    let pane = followed.launch.pane.clone();
    if !agent_is_idle(&pane) {
        return Ok(());
    }
    let typed = tmux::run(&["send-keys", "-t", &pane, "-l", "--", &nudge]).and_then(|_| {
        std::thread::sleep(SUBMIT_DELAY);
        tmux::run(&["send-keys", "-t", &pane, "Enter"])
    });
    if let Err(error) = typed {
        eprintln!("failed to nudge {pane}: {error:#}");
        return Ok(());
    }
    launch::update(&followed.launch.id, |launch| {
        launch.nudges.push(key.clone())
    })?;
    followed.launch.nudges.push(key);
    Ok(())
}

fn agent_is_idle(pane: &str) -> bool {
    live_agent(pane).is_some_and(|(state, live)| is_idle(&state, &live))
}

/// The agent the hooks registered in `pane` and the pane as it is now, while both exist.
fn live_agent(pane: &str) -> Option<(AgentState, LivePane)> {
    let state = agents_dir()
        .ok()
        .and_then(|dir| std::fs::read_to_string(state_file(&dir, pane)).ok())
        .and_then(|json| serde_json::from_str::<AgentState>(&json).ok())?;
    let live = tmux::run(&[
        "display-message",
        "-p",
        "-t",
        pane,
        "#{pane_pid}\t#{pane_current_command}\t#{pane_active}\t#{window_active}\t#{session_attached}",
    ])
    .ok()?;
    let fields: Vec<&str> = live.split('\t').collect();
    let [pid, command, pane_active, window_active, attached] = fields[..] else {
        return None;
    };
    let processes = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,comm="])
        .output()
        .ok()?;
    let (command, _) = tmux_aside_agents::processes::pane_process(
        pid.parse().ok()?,
        command,
        &String::from_utf8_lossy(&processes.stdout),
    );
    let live = LivePane {
        pid: pid.parse().ok()?,
        command,
        viewed: pane_active == "1" && window_active == "1" && attached != "0",
    };
    Some((state, live))
}

/// Removes the Launch's worktree, then closes the agent's pane when it is still the one
/// launched there, which closes its window unless someone split it. Returns whether the worktree
/// was removed; failures go to stderr.
fn clean_up(launch: &Launch) -> bool {
    let agent = cleanup::agent_still_there(launch);
    if let Err(error) = cleanup::remove_worktree(&launch.repository, &launch.worktree, false) {
        eprintln!("failed to remove {}: {error:#}", launch.worktree.display());
        return false;
    }
    if agent && let Err(error) = tmux::run(&["kill-pane", "-t", &launch.pane]) {
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
