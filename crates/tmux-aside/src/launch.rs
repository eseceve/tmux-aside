use std::collections::BTreeMap;
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::config;
use crate::hooks::shell_quote;
use crate::i18n::Language;
use crate::pack;
use crate::setup::Ai;
use crate::tmux;

const TASK_PLACEHOLDER: &str = "${task}";
const FRONT_MATTER: &str = "---";
const DEFAULT_SESSION: &str = "tmux-aside";
const BUNDLED_TEMPLATES: &[(&str, &str)] = &[
    ("adopt", include_str!("../templates/adopt.md")),
    ("implement", include_str!("../templates/implement.md")),
    (FIX_TEMPLATE, include_str!("../templates/fix.md")),
    (QA_TEMPLATE, include_str!("../templates/qa.md")),
];
const QA_TEMPLATE: &str = "qa";
const FIX_TEMPLATE: &str = "fix";

/// Per-invocation overrides for the template and configured agent defaults.
#[derive(Clone, Debug, Default, clap::Args)]
pub struct AgentOptions {
    /// Agent to run (overrides template and configuration)
    #[arg(long, value_parser = ["claude", "codex", "agy"])]
    pub agent: Option<String>,
    /// Model name passed to the selected agent
    #[arg(long)]
    pub model: Option<String>,
    /// Reasoning effort supported by the selected agent
    #[arg(long)]
    pub effort: Option<String>,
}

/// Resolved settings preserved when a launch is resumed.
#[derive(Clone, Debug)]
pub struct AgentSettings {
    pub ai: Ai,
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl AgentSettings {
    /// Validates settings before creating a worktree or starting an agent.
    pub fn validate(&self) -> Result<()> {
        if self
            .model
            .as_ref()
            .is_some_and(|model| model.trim().is_empty())
        {
            bail!("model must not be empty");
        }
        if let Some(effort) = &self.effort {
            let supported: &[&str] = match self.ai {
                Ai::Codex => &["none", "minimal", "low", "medium", "high", "xhigh"],
                Ai::Claude => &["low", "medium", "high", "max"],
                Ai::Agy => &["low", "medium", "high", "xhigh", "max"],
            };
            if !supported.contains(&effort.as_str()) {
                bail!(
                    "unsupported effort '{effort}' for {}; expected {}",
                    self.ai.program(),
                    supported.join(", ")
                );
            }
        }
        Ok(())
    }

    /// Native CLI arguments for model and effort, without altering permissions.
    pub fn arguments(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(model) = &self.model {
            args.extend(["--model".into(), model.clone()]);
        }
        if let Some(effort) = &self.effort {
            match self.ai {
                Ai::Codex => {
                    args.extend(["-c".into(), format!("model_reasoning_effort=\"{effort}\"")])
                }
                _ => args.extend(["--effort".into(), effort.clone()]),
            }
        }
        args
    }
}

/// The prompt an agent is launched with, into which the Task reference is filled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    /// Whether the agent opens a pull request, so that the Launch enters the Lifecycle.
    pub opens_pull_request: bool,
    /// The name of the AI the Template is launched with; the config's when unset.
    pub ai: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    body: String,
}

#[derive(Deserialize, Default)]
struct FrontMatter {
    #[serde(default)]
    pull_request: bool,
    #[serde(default)]
    ai: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

impl Template {
    /// Reads a Template: an optional YAML front matter between `---` lines, then the prompt,
    /// where `${task}` stands for the Task reference.
    pub fn parse(text: &str) -> Result<Self> {
        let (front_matter, body) = match text
            .strip_prefix(&format!("{FRONT_MATTER}\n"))
            .and_then(|rest| rest.split_once(&format!("\n{FRONT_MATTER}\n")))
        {
            Some((yaml, body)) => (
                serde_saphyr::from_str::<Option<FrontMatter>>(yaml)
                    .map_err(|error| anyhow!("{error}"))?
                    .unwrap_or_default(),
                body,
            ),
            None => (FrontMatter::default(), text),
        };
        Ok(Template {
            opens_pull_request: front_matter.pull_request,
            ai: front_matter.ai,
            model: front_matter.model,
            effort: front_matter.effort,
            body: body.to_string(),
        })
    }

    /// The prompt with `task` in place of `${task}`.
    pub fn prompt(&self, task: &str) -> String {
        self.body.replace(TASK_PLACEHOLDER, task)
    }
}

/// An agent started by tmux-aside in its own worktree and window, or one that registered itself
/// with `tmux-aside follow`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launch {
    pub id: String,
    pub repository: PathBuf,
    pub branch: String,
    pub worktree: PathBuf,
    pub session: String,
    pub window: String,
    pub pane: String,
    pub ai: Ai,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The Template it was launched with; none for a followed agent.
    #[serde(default)]
    pub template: Option<String>,
    pub task: String,
    pub opens_pull_request: bool,
    pub pull_request: Option<String>,
    /// Nudges already typed into the agent, each as `stage@head commit`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nudges: Vec<String>,
    /// The Group it belongs to; a Launch without one is a Group of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// The pull requests the Watcher last launched QA for by itself, so it does it once per
    /// set of pull requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_qa: Option<String>,
    /// The worktree is not tmux-aside's, as for a followed or discovered pull request: the
    /// Watcher never removes it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keeps_worktree: bool,
    /// The Watcher ran the `on_deployed` action for its pull request.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ran_on_deployed: bool,
}

/// What `tmux-aside launch` was given on the command line.
pub struct Options {
    pub agent: AgentOptions,
    pub repository: PathBuf,
    pub template: String,
    /// The Task reference; the pull request's URL when adopting one and none is given.
    pub task: Option<String>,
    pub source: Source,
    /// The Group the Launch joins.
    pub group: Option<String>,
}

/// Where a Launch's branch comes from.
pub enum Source {
    /// A new branch, created from `base` (the remote's default branch when none).
    Branch { name: String, base: Option<String> },
    /// The branch of an existing pull request, given by its URL.
    PullRequest(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestHead {
    url: String,
    head_ref_name: String,
    is_cross_repository: bool,
    state: String,
}

/// The URL and branch of the pull request at `url`, which must not come from a fork and must be
/// open, or merged when `merged` is accepted.
fn view_pull_request(url: &str, merged: bool, language: Language) -> Result<PullRequestHead> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            url,
            "--json",
            "url,headRefName,isCrossRepository,state",
        ])
        .output()
        .context("failed to run gh")?;
    if !output.status.success() {
        bail!("gh: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    let head: PullRequestHead = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("unexpected answer from gh pr view {url}"))?;
    if head.is_cross_repository {
        bail!((language.messages().fork_not_supported)(&head.url));
    }
    if head.state != "OPEN" && !(merged && head.state == "MERGED") {
        bail!((language.messages().pull_request_not_open)(&head.url));
    }
    Ok(head)
}

/// The owner and name of a GitHub repository from a remote URL in the ssh
/// (`git@github.com:o/r.git`), `ssh://` or https form; `None` for other hosts.
pub fn github_repository(remote_url: &str) -> Option<(String, String)> {
    let path = [
        "git@github.com:",
        "ssh://git@github.com/",
        "https://github.com/",
    ]
    .iter()
    .find_map(|prefix| remote_url.trim().strip_prefix(prefix))?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.trim_end_matches('/').split_once('/')?;
    (!owner.is_empty() && !name.is_empty() && !name.contains('/'))
        .then(|| (owner.to_string(), name.to_string()))
}

/// Fails unless the pull request at `url` belongs to the GitHub repository of `repository`'s
/// `origin`, which the Watcher searches.
fn ensure_same_repository(url: &str, repository: &Path, language: Language) -> Result<()> {
    let origin = git(repository, &["config", "--get", "remote.origin.url"]).unwrap_or_default();
    let pull_request = url.split("/pull/").next().and_then(github_repository);
    if pull_request.is_none() || pull_request != github_repository(&origin) {
        bail!((language.messages().other_repository)(
            url,
            &repository.display().to_string()
        ));
    }
    Ok(())
}

fn ensure_not_followed(url: &str, language: Language) -> Result<()> {
    if let Some(launch) = list()?
        .into_iter()
        .find(|launch| launch.pull_request.as_deref() == Some(url))
    {
        bail!((language.messages().already_followed)(url, &launch.id));
    }
    Ok(())
}

/// Creates the branch and its worktree next to the clone, opens a background window in the
/// current tmux session (or the default one outside tmux), starts the agent there with the
/// filled-in Template and records the Launch. Nothing is created when a check fails.
pub fn run(options: Options, language: Language) -> Result<Launch> {
    let messages = language.messages();
    let template = find_template(&options.template, language)?
        .ok_or_else(|| anyhow!((messages.unknown_template)(&options.template)))?;
    let settings = choose_agent(&template, options.agent, language)?;
    let ai = settings.ai;
    let repository = git(&options.repository, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .map_err(|_| {
            anyhow!((messages.not_a_repository)(
                &options.repository.display().to_string()
            ))
        })?;
    let (branch, checkout, pull_request) = match options.source {
        Source::Branch { name, base } => {
            let base = match base {
                Some(base) => base,
                None if ref_exists(&repository, "origin/HEAD") => "origin/HEAD".to_string(),
                None => "HEAD".to_string(),
            };
            if ref_exists(&repository, &format!("refs/heads/{name}")) {
                bail!((messages.branch_exists)(&name));
            }
            if !ref_exists(&repository, &base) {
                bail!((messages.unknown_base)(&base));
            }
            (name, Checkout::New { base }, None)
        }
        Source::PullRequest(url) => {
            let head = view_pull_request(&url, false, language)?;
            ensure_same_repository(&head.url, &repository, language)?;
            ensure_not_followed(&head.url, language)?;
            let checkout =
                match ref_exists(&repository, &format!("refs/heads/{}", head.head_ref_name)) {
                    true => Checkout::Existing,
                    false => Checkout::Remote,
                };
            (head.head_ref_name, checkout, Some(head.url))
        }
    };
    let task = options
        .task
        .or_else(|| pull_request.clone())
        .ok_or_else(|| anyhow!(messages.task_required))?;
    let handle = branch.replace('/', "-");
    let name = file_name(&repository)?;
    let worktree = repository
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", repository.display()))?
        .join(format!("{name}__worktrees"))
        .join(&handle);
    if worktree.exists() {
        bail!((messages.worktree_exists)(&worktree.display().to_string()));
    }
    let program = find_on_path(ai.program())
        .ok_or_else(|| anyhow!((messages.agent_not_found)(ai.program())))?;
    let session = current_session()?;
    let id = format!("{name}__{handle}");
    let directory = launches_dir()?;
    let record = directory.join(format!("{id}.json"));
    if record.exists() {
        bail!((messages.launch_exists)(&id));
    }

    let worktree_arg = worktree.to_str().context("worktree path is not UTF-8")?;
    match &checkout {
        Checkout::New { base } => git(
            &repository,
            &[
                "worktree",
                "add",
                "-q",
                "--no-track",
                "-b",
                &branch,
                worktree_arg,
                base,
            ],
        )?,
        Checkout::Existing => git(
            &repository,
            &["worktree", "add", "-q", worktree_arg, &branch],
        )?,
        Checkout::Remote => {
            git(&repository, &["fetch", "-q", "origin", &branch])?;
            git(
                &repository,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "--track",
                    "-b",
                    &branch,
                    worktree_arg,
                    &format!("origin/{branch}"),
                ],
            )?
        }
    };
    std::fs::create_dir_all(&directory)?;
    let prompt = directory.join(format!("{id}.prompt"));
    std::fs::write(&prompt, template.prompt(&task))?;
    let (window, pane) = open_agent_window(
        &session,
        &handle,
        &worktree,
        &settings,
        &program,
        Some(&prompt),
    )?;

    let launch = Launch {
        id,
        repository,
        branch,
        worktree,
        session,
        window,
        pane,
        ai,
        model: settings.model,
        effort: settings.effort,
        template: Some(options.template),
        task,
        opens_pull_request: pull_request.is_some() || template.opens_pull_request,
        pull_request,
        nudges: Vec::new(),
        group: options.group,
        auto_qa: None,
        keeps_worktree: false,
        ran_on_deployed: false,
    };
    write(&record, &launch)?;
    Ok(launch)
}

/// The tmux session new windows open in: the current one inside tmux, the default one
/// outside.
fn current_session() -> Result<String> {
    match (std::env::var_os("TMUX"), std::env::var("TMUX_PANE")) {
        (Some(_), Ok(pane)) => tmux::run(&["display-message", "-p", "-t", &pane, "#S"]),
        (Some(_), Err(_)) => tmux::run(&["display-message", "-p", "#S"]),
        (None, _) => Ok(DEFAULT_SESSION.to_string()),
    }
}

/// Opens a background window named `name` in `directory`, creating `session` if missing, and
/// starts `ai`, found at `program`, there with the prompt in `prompt`, or resuming its latest
/// conversation there without one. Returns the window's and pane's ids.
fn open_agent_window(
    session: &str,
    name: &str,
    directory: &Path,
    settings: &AgentSettings,
    program: &Path,
    prompt: Option<&Path>,
) -> Result<(String, String)> {
    settings.validate()?;
    let executable = std::env::current_exe()?;
    let command = [
        executable.as_path(),
        Path::new("_agent"),
        Path::new("--ai"),
        Path::new(settings.ai.program()),
        Path::new("--program"),
        program,
    ]
    .into_iter()
    .chain(
        settings
            .model
            .as_ref()
            .into_iter()
            .flat_map(|value| [Path::new("--model"), Path::new(value)]),
    )
    .chain(
        settings
            .effort
            .as_ref()
            .into_iter()
            .flat_map(|value| [Path::new("--effort"), Path::new(value)]),
    )
    .chain(prompt)
    .map(|part| part.to_str().map(shell_quote))
    .collect::<Option<Vec<_>>>()
    .context("path is not UTF-8")?
    .join(" ");
    let directory = directory.to_str().context("path is not UTF-8")?;
    let target = format!("={session}:");
    let placement: [&str; 3] = if tmux::run(&["has-session", "-t", &format!("={session}")]).is_ok()
    {
        ["new-window", "-t", &target]
    } else {
        ["new-session", "-s", session]
    };
    let opened = tmux::run(
        &[
            placement.as_slice(),
            &[
                "-d",
                "-P",
                "-F",
                "#{window_id} #{pane_id}",
                "-n",
                name,
                "-c",
                directory,
                &command,
            ],
        ]
        .concat(),
    )?;
    let (window, pane) = opened
        .split_once(' ')
        .ok_or_else(|| anyhow!("unexpected tmux output: {opened}"))?;
    Ok((window.to_string(), pane.to_string()))
}

/// Launches one QA agent, with a fresh context and the `qa` Template, for the pull requests at
/// `urls`, in a window at the clone of the first one, or in the current directory when no clone
/// is recorded for them (a discovered pull request). Each must be followed by a Launch. The
/// Task reference lists the URLs. The QA Launch opens no pull request of its own.
pub fn qa(urls: &[String], language: Language) -> Result<Launch> {
    qa_with_options(urls, AgentOptions::default(), language)
}

/// Launches QA with per-invocation agent overrides.
pub fn qa_with_options(
    urls: &[String],
    options: AgentOptions,
    language: Language,
) -> Result<Launch> {
    let messages = language.messages();
    let template = find_template(QA_TEMPLATE, language)?
        .ok_or_else(|| anyhow!((messages.unknown_template)(QA_TEMPLATE)))?;
    let launches = list()?;
    let mut repository = None;
    let mut numbers = Vec::new();
    for url in urls {
        let followed = launches
            .iter()
            .find(|launch| launch.pull_request.as_deref() == Some(url.as_str()))
            .ok_or_else(|| anyhow!((messages.not_followed)(url)))?;
        if repository.is_none() && !followed.repository.as_os_str().is_empty() {
            repository = Some(followed.repository.clone());
        }
        numbers.push(pull_request_number(url).to_string());
    }
    let first = urls
        .first()
        .ok_or_else(|| anyhow!(messages.no_pull_requests))?;
    let name = match &repository {
        Some(repository) => file_name(repository)?.to_string(),
        None => repository_name(first),
    };
    let repository = match repository {
        Some(repository) => repository,
        None => std::env::current_dir()?,
    };
    let settings = choose_agent(&template, options, language)?;
    let ai = settings.ai;
    let program = find_on_path(ai.program())
        .ok_or_else(|| anyhow!((messages.agent_not_found)(ai.program())))?;
    let session = current_session()?;
    let handle = format!("qa-{}", numbers.join("-"));
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    let id = format!("{name}__{handle}-{started}");
    let directory = launches_dir()?;
    std::fs::create_dir_all(&directory)?;
    let task = urls.join(" ");
    let (passed, failed) = qa_labels()?;
    let prompt = directory.join(format!("{id}.prompt"));
    std::fs::write(
        &prompt,
        template
            .prompt(&task)
            .replace("${passed_label}", &passed)
            .replace("${failed_label}", &failed),
    )?;
    let (window, pane) = open_agent_window(
        &session,
        &handle,
        &repository,
        &settings,
        &program,
        Some(&prompt),
    )?;
    let launch = Launch {
        id,
        branch: git(&repository, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default(),
        worktree: repository.clone(),
        repository,
        session,
        window,
        pane,
        ai,
        model: settings.model,
        effort: settings.effort,
        template: Some(QA_TEMPLATE.to_string()),
        task,
        opens_pull_request: false,
        pull_request: None,
        nudges: Vec::new(),
        group: None,
        auto_qa: None,
        keeps_worktree: false,
        ran_on_deployed: false,
    };
    write(&directory.join(format!("{}.json", launch.id)), &launch)?;
    Ok(launch)
}

/// Runs the config's or the Pack's `on_deployed` action for the deployed pull requests at
/// `urls`, in the current directory: the AI starts with its Template, where `${task}` stands
/// for the distinct `tasks` (the URLs when none is given) and `${urls}` for the URLs, both
/// sorted and joined by spaces. Headless, the AI's output goes to stderr and this waits for
/// it; otherwise it opens a window with an interactive agent.
pub fn deployed(urls: &[String], tasks: &[String], language: Language) -> Result<()> {
    let messages = language.messages();
    let config = config::load(&config::default_path()?)?;
    let action = match &config {
        Some(config) => pack::on_deployed(config, pack::configured(config)?.as_ref()),
        None => None,
    }
    .unwrap_or_default();
    let template = find_template(&action.template, language)?
        .ok_or_else(|| anyhow!((messages.unknown_template)(&action.template)))?;
    let settings = choose_agent(&template, AgentOptions::default(), language)?;
    let ai = settings.ai;
    let program = find_on_path(ai.program())
        .ok_or_else(|| anyhow!((messages.agent_not_found)(ai.program())))?;
    let sorted = |values: &[String]| {
        let mut values = values.to_vec();
        values.sort();
        values.dedup();
        values.join(" ")
    };
    let prompt = template
        .prompt(&sorted(if tasks.is_empty() { urls } else { tasks }))
        .replace("${urls}", &sorted(urls));
    if !action.window {
        let status = Command::new(&program)
            .args(settings.arguments())
            .args(ai.print_arguments(&prompt))
            .stdin(Stdio::null())
            .stdout(std::io::stderr().as_fd().try_clone_to_owned()?)
            .status()
            .with_context(|| format!("failed to run {}", program.display()))?;
        if !status.success() {
            bail!("{} {status}", ai.program());
        }
        return Ok(());
    }
    let numbers: Vec<&str> = urls.iter().map(|url| pull_request_number(url)).collect();
    let handle = format!("deployed-{}", numbers.join("-"));
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    let directory = launches_dir()?;
    std::fs::create_dir_all(&directory)?;
    let prompt_file = directory.join(format!("{handle}-{started}.prompt"));
    std::fs::write(&prompt_file, prompt)?;
    open_agent_window(
        &current_session()?,
        &handle,
        &std::env::current_dir()?,
        &settings,
        &program,
        Some(&prompt_file),
    )?;
    Ok(())
}

/// Reopens the agent of every Launch whose pane is gone or no longer in its worktree, such as
/// after tmux or the computer restarted, resuming its latest conversation in a new window of its
/// session, and records where. Launches without an agent or a worktree are left alone. With
/// `dry_run`, only says which it would reopen.
pub fn resurrect(dry_run: bool, language: Language) -> Result<String> {
    let messages = language.messages();
    let mut report = Vec::new();
    for launch in list()? {
        if launch.pane.is_empty() || !launch.worktree.is_dir() || agent_alive(&launch) {
            continue;
        }
        if dry_run {
            report.push((messages.would_reopen)(&launch.id));
            continue;
        }
        let program = find_on_path(launch.ai.program())
            .ok_or_else(|| anyhow!((messages.agent_not_found)(launch.ai.program())))?;
        let name = file_name(&launch.worktree)?.to_string();
        let (window, pane) = open_agent_window(
            &launch.session,
            &name,
            &launch.worktree,
            &AgentSettings {
                ai: launch.ai,
                model: launch.model.clone(),
                effort: launch.effort.clone(),
            },
            &program,
            None,
        )?;
        update(&launch.id, |launch| {
            launch.window = window;
            launch.pane = pane;
        })?;
        report.push((messages.reopened)(&launch.id));
    }
    if report.is_empty() {
        report.push(messages.nothing_to_reopen.to_string());
    }
    Ok(report.join("\n"))
}

/// Whether the pane of `launch` still exists and works in its worktree: tmux reuses pane ids
/// after a restart.
fn agent_alive(launch: &Launch) -> bool {
    tmux::run(&[
        "display-message",
        "-p",
        "-t",
        &launch.pane,
        "#{pane_current_path}",
    ])
    .is_ok_and(|path| Path::new(&path).starts_with(&launch.worktree))
}

/// Launches an agent that fixes what the QA of the pull request at `url` found, with the `fix`
/// Template, in a new branch of the same repository and in the same Group, so that QA reruns
/// for both once the fix reaches QA. A pull request without a Group gets one, named after its
/// Launch. The Task reference points the agent to the QA report.
pub fn fix(url: &str, language: Language) -> Result<Launch> {
    fix_with_options(url, AgentOptions::default(), language)
}

/// Launches a QA fix with per-invocation agent overrides.
pub fn fix_with_options(url: &str, agent: AgentOptions, language: Language) -> Result<Launch> {
    let messages = language.messages();
    let original = list()?
        .into_iter()
        .find(|launch| launch.pull_request.as_deref() == Some(url))
        .ok_or_else(|| anyhow!((messages.not_followed)(url)))?;
    let group = original
        .group
        .clone()
        .unwrap_or_else(|| original.id.clone());
    let number = pull_request_number(url);
    let branch = (1..)
        .map(|attempt| match attempt {
            1 => format!("fix/qa-{number}"),
            _ => format!("fix/qa-{number}-{attempt}"),
        })
        .find(|branch| {
            ["refs/heads", "refs/remotes/origin"]
                .iter()
                .all(|refs| !ref_exists(&original.repository, &format!("{refs}/{branch}")))
        })
        .expect("an unused branch name");
    let launch = run(
        Options {
            agent,
            repository: original.repository,
            template: FIX_TEMPLATE.to_string(),
            task: Some(format!(
                "the QA of {url} failed; its report is the latest QA comment on it"
            )),
            source: Source::Branch {
                name: branch,
                base: None,
            },
            group: Some(group.clone()),
        },
        language,
    )?;
    if original.group.is_none() {
        update(&original.id, |launch| launch.group = Some(group))?;
    }
    Ok(launch)
}

/// The labels that record QA's verdict: those given to the Ready to deploy Criterion in the
/// config, or `qa:passed` and `qa:failed`.
fn qa_labels() -> Result<(String, String)> {
    let args = config::load(&config::default_path()?)?
        .and_then(|config| config.criteria.ready_to_deploy)
        .map(|criterion| criterion.args)
        .unwrap_or_default();
    let label = |flag: &str, default: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .cloned()
            .unwrap_or_else(|| default.to_string())
    };
    Ok((
        label("--passed-label", "qa:passed"),
        label("--failed-label", "qa:failed"),
    ))
}

/// The name of the repository of the pull request at `url`, such as `app` in
/// `https://github.com/org/app/pull/12`.
fn repository_name(url: &str) -> String {
    url.split("/pull/")
        .next()
        .and_then(|repository| repository.rsplit('/').next())
        .unwrap_or("repo")
        .to_string()
}

/// The number of the pull request at `url`, such as `12` in `https://github.com/o/r/pull/12/files`.
fn pull_request_number(url: &str) -> &str {
    url.split("/pull/")
        .nth(1)
        .and_then(|rest| rest.split(['/', '#', '?']).next())
        .unwrap_or("pr")
}

/// Deletes the record of Launch `id`, which leaves the Watcher and `status`.
pub fn forget(id: &str) -> Result<()> {
    let directory = launches_dir()?;
    for extension in ["json", "prompt"] {
        match std::fs::remove_file(directory.join(format!("{id}.{extension}"))) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    }
    Ok(())
}

/// How the worktree's branch is checked out.
enum Checkout {
    /// A new branch from `base`.
    New { base: String },
    /// A local branch that already exists.
    Existing,
    /// A new local branch tracking the remote branch of the same name.
    Remote,
}

#[derive(Deserialize)]
struct AgentPane {
    pane_pid: u32,
    command: String,
}

/// Records the agent in `$TMUX_PANE` as the agent of the pull request at `url`, open or already
/// merged, with the worktree it runs in, so that the Watcher follows it. The Watcher never
/// removes that worktree. Nothing is created: no worktree, no
/// window, and no record when a check fails.
pub fn follow(url: &str, language: Language) -> Result<Launch> {
    let messages = language.messages();
    let not_an_agent = || anyhow!(messages.not_an_agent_pane);
    let pane = std::env::var("TMUX_PANE")
        .ok()
        .filter(|_| std::env::var_os("TMUX").is_some())
        .ok_or_else(not_an_agent)?;
    let agent: AgentPane =
        std::fs::read_to_string(config::agent_state_file(&config::agents_dir()?, &pane))
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .ok_or_else(not_an_agent)?;
    let live = tmux::run(&[
        "display-message",
        "-p",
        "-t",
        &pane,
        "#{pane_pid}\t#{session_name}\t#{window_id}\t#{pane_current_path}",
    ])?;
    let fields: Vec<&str> = live.splitn(4, '\t').collect();
    let [pid, session, window, path] = fields[..] else {
        bail!("unexpected tmux output: {live}");
    };
    if pid.parse::<u32>().ok() != Some(agent.pane_pid) {
        return Err(not_an_agent());
    }
    let ai: Ai = serde_json::from_value(serde_json::Value::String(agent.command))
        .map_err(|_| not_an_agent())?;
    let head = view_pull_request(url, true, language)?;
    let path = Path::new(path);
    let worktree = git(path, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .map_err(|_| anyhow!((messages.not_a_repository)(&path.display().to_string())))?;
    let common = git(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let repository = Path::new(&common)
        .parent()
        .ok_or_else(|| anyhow!("{common} has no parent directory"))?
        .to_path_buf();
    let branch = git(path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch != head.head_ref_name {
        bail!((messages.branch_mismatch)(&head.head_ref_name, &branch));
    }
    ensure_same_repository(&head.url, &repository, language)?;
    ensure_not_followed(&head.url, language)?;
    let id = format!("{}__{}", file_name(&repository)?, branch.replace('/', "-"));
    let directory = launches_dir()?;
    let record = directory.join(format!("{id}.json"));
    if record.exists() {
        bail!((messages.launch_exists)(&id));
    }
    let launch = Launch {
        id,
        repository,
        branch,
        worktree,
        session: session.to_string(),
        window: window.to_string(),
        pane,
        ai,
        model: None,
        effort: None,
        template: None,
        task: head.url.clone(),
        opens_pull_request: true,
        pull_request: Some(head.url),
        nudges: Vec::new(),
        group: None,
        auto_qa: None,
        keeps_worktree: true,
        ran_on_deployed: false,
    };
    std::fs::create_dir_all(&directory)?;
    write(&record, &launch)?;
    Ok(launch)
}

/// Changes the recorded Launch `id` as it is on disk now, so that changes made meanwhile by
/// another process are kept. Does nothing when it is not recorded.
pub fn update(id: &str, change: impl FnOnce(&mut Launch)) -> Result<()> {
    let record = launches_dir()?.join(format!("{id}.json"));
    let Ok(json) = std::fs::read_to_string(&record) else {
        return Ok(());
    };
    let mut launch: Launch = serde_json::from_str(&json)
        .with_context(|| format!("invalid launch {}", record.display()))?;
    change(&mut launch);
    write(&record, &launch)
}

/// Records `launch`, replacing its previous record.
pub fn save(launch: &Launch) -> Result<()> {
    let directory = launches_dir()?;
    std::fs::create_dir_all(&directory)?;
    write(&directory.join(format!("{}.json", launch.id)), launch)
}

fn write(record: &Path, launch: &Launch) -> Result<()> {
    let partial = record.with_extension("json.tmp");
    std::fs::write(&partial, serde_json::to_string_pretty(launch)?)
        .and_then(|()| std::fs::rename(&partial, record))
        .with_context(|| format!("failed to write {}", record.display()))
}

/// The line telling that `launch` is now followed.
pub fn announce_followed(launch: &Launch, language: Language) -> String {
    (language.messages().followed)(&launch.id)
}

/// The line telling where `launch` was opened.
pub fn announce(launch: &Launch, language: Language) -> String {
    (language.messages().launched)(&launch.id, &format!("{}:{}", launch.session, launch.window))
}

/// Where the Watcher last saw a Launch's pull request, as it records it for `status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// The stage, as Criteria name it: `implementing`, `ready_to_merge`, `qa`...
    pub stage: String,
    /// The stage's name in the sidebar's language.
    pub stage_name: String,
    /// What is missing to reach the next stage, or where the pull request stands.
    pub summary: String,
}

/// Records where the Watcher sees each Launch's pull request, replacing what it recorded
/// before, keyed by Launch id.
pub fn record_progress(progress: &BTreeMap<String, Progress>) -> Result<()> {
    let path = progress_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial = path.with_extension("json.tmp");
    std::fs::write(&partial, serde_json::to_string_pretty(progress)?)
        .and_then(|()| std::fs::rename(&partial, &path))
        .with_context(|| format!("failed to write {}", path.display()))
}

/// What the Watcher last recorded with [`record_progress`]; empty when it recorded nothing.
pub fn progress() -> BTreeMap<String, Progress> {
    progress_path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn progress_path() -> Result<PathBuf> {
    Ok(config::state_dir()?.join("stages.json"))
}

/// `launches` as JSON for `status --json`, each with the `stage` and `summary` the Watcher last
/// recorded for it, when it did.
pub fn as_json(launches: &[Launch], progress: &BTreeMap<String, Progress>) -> Result<String> {
    let mut values = Vec::new();
    for launch in launches {
        let mut value = serde_json::to_value(launch)?;
        if let (Some(object), Some(progress)) = (value.as_object_mut(), progress.get(&launch.id)) {
            object.insert("stage".to_string(), progress.stage.clone().into());
            object.insert("summary".to_string(), progress.summary.clone().into());
        }
        values.push(value);
    }
    Ok(serde_json::to_string_pretty(&values)?)
}

/// Every recorded Launch, ordered by id. A record that cannot be read is skipped, with a
/// warning on stderr, so that it hides no other Launch.
pub fn list() -> Result<Vec<Launch>> {
    let directory = launches_dir()?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", directory.display()));
        }
    };
    let mut launches = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            match std::fs::read_to_string(&path)
                .map_err(anyhow::Error::from)
                .and_then(|json| Ok(serde_json::from_str::<Launch>(&json)?))
            {
                Ok(launch) => launches.push(launch),
                Err(error) => eprintln!("skipped invalid launch {}: {error}", path.display()),
            }
        }
    }
    launches.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(launches)
}

/// `launches` as plain text, one block per Launch, with the stage the Watcher last recorded for
/// it in `progress`.
pub fn describe(
    launches: &[Launch],
    progress: &BTreeMap<String, Progress>,
    language: Language,
) -> String {
    let messages = language.messages();
    if launches.is_empty() {
        return format!("{}\n", messages.no_launches);
    }
    launches
        .iter()
        .map(|launch| {
            let pull_request = match (&launch.pull_request, launch.opens_pull_request) {
                (Some(url), _) => url.as_str(),
                (None, true) => messages.pull_request_pending,
                (None, false) => messages.no_pull_request,
            };
            let fields = [
                (
                    messages.repository,
                    or_dash(&launch.repository.display().to_string()),
                ),
                (messages.branch, launch.branch.clone()),
                (
                    messages.worktree,
                    or_dash(&launch.worktree.display().to_string()),
                ),
                (
                    messages.window,
                    or_dash(format!("{} {}", launch.session, launch.window).trim()),
                ),
                (
                    messages.agent,
                    match launch.pane.is_empty() {
                        true => "-".to_string(),
                        false => format!("{} {}", launch.ai.program(), launch.pane),
                    },
                ),
                (
                    messages.template,
                    launch.template.clone().unwrap_or_else(|| "-".to_string()),
                ),
                (messages.task, launch.task.clone()),
                (messages.pull_request, pull_request.to_string()),
                (
                    messages.group,
                    launch.group.clone().unwrap_or_else(|| "-".to_string()),
                ),
                (
                    messages.stage,
                    progress.get(&launch.id).map_or_else(
                        || "-".to_string(),
                        |progress| format!("{} · {}", progress.stage_name, progress.summary),
                    ),
                ),
            ];
            let width = fields
                .iter()
                .map(|(label, _)| label.chars().count())
                .max()
                .unwrap_or(0);
            let mut block = format!("{}\n", launch.id);
            for (label, value) in fields {
                block.push_str(&format!("  {label:<width$}  {value}\n"));
            }
            block
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn or_dash(value: &str) -> String {
    match value.is_empty() {
        true => "-".to_string(),
        false => value.to_string(),
    }
}

/// Runs the agent with the prompt in `prompt_file`, or resumes its latest conversation without
/// one, then leaves a shell in the window.
pub fn start_agent(
    settings: AgentSettings,
    program: &Path,
    prompt_file: Option<&Path>,
) -> Result<()> {
    settings.validate()?;
    let ai = settings.ai;
    let arguments = match prompt_file {
        Some(prompt_file) => ai.start_arguments(
            &std::fs::read_to_string(prompt_file)
                .with_context(|| format!("failed to read {}", prompt_file.display()))?,
        ),
        None => ai.resume_arguments(),
    };
    let _ = Command::new(program)
        .args(settings.arguments())
        .args(arguments)
        .status();
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
    Err(Command::new(shell).arg("-l").exec().into())
}

/// Resolves each setting from invocation, template, configuration, then agent defaults.
fn choose_agent(
    template: &Template,
    options: AgentOptions,
    language: Language,
) -> Result<AgentSettings> {
    let config = config::load(&config::default_path()?)?;
    resolve_agent(template, options, config.as_ref(), language)
}

/// Resolves settings without I/O, rejecting unsupported agent/effort combinations.
pub fn resolve_agent(
    template: &Template,
    options: AgentOptions,
    config: Option<&config::Config>,
    language: Language,
) -> Result<AgentSettings> {
    let name = options
        .agent
        .or_else(|| template.ai.clone())
        .or_else(|| config.as_ref().and_then(|c| c.ai.clone()));
    let ai = match name {
        None => Ai::Claude,
        Some(name) => Ai::from_name(&name)
            .ok_or_else(|| anyhow!((language.messages().unsupported_ai)(&name)))?,
    };
    let settings = AgentSettings {
        ai,
        model: options
            .model
            .or_else(|| template.model.clone())
            .or_else(|| config.as_ref().and_then(|c| c.model.clone())),
        effort: options
            .effort
            .or_else(|| template.effort.clone())
            .or_else(|| config.as_ref().and_then(|c| c.effort.clone())),
    };
    settings.validate()?;
    Ok(settings)
}

/// The Template `name`: the user's own, then the Pack's, then the bundled one. A Pack that
/// cannot be read is reported and skipped.
fn find_template(name: &str, language: Language) -> Result<Option<Template>> {
    let mut folders = vec![config::templates_dir()?];
    let config = config::load(&config::default_path()?)?;
    match config.as_ref().map(pack::configured).transpose() {
        Ok(pack) => folders.extend(pack.flatten().map(|pack| pack.templates)),
        Err(error) => eprintln!(
            "{}",
            (language.messages().invalid_pack)(&format!("{error:#}"))
        ),
    }
    for folder in folders {
        let path = folder.join(format!("{name}.md"));
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                return Template::parse(&text)
                    .with_context(|| format!("invalid template {}", path.display()))
                    .map(Some);
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error).with_context(|| format!("failed to read {}", path.display()));
            }
            Err(_) => {}
        }
    }
    BUNDLED_TEMPLATES
        .iter()
        .find(|(bundled, _)| *bundled == name)
        .map(|(_, text)| Template::parse(text))
        .transpose()
}

fn launches_dir() -> Result<PathBuf> {
    Ok(config::state_dir()?.join("launches"))
}

fn file_name(path: &Path) -> Result<&str> {
    path.file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("{} has no UTF-8 name", path.display()))
}

fn find_on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

fn ref_exists(repository: &Path, name: &str) -> bool {
    git(repository, &["rev-parse", "--verify", "--quiet", name]).is_ok()
}

pub(crate) fn git(directory: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .context("failed to spawn git")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
