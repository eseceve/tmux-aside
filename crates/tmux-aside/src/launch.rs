use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::config;
use crate::hooks::shell_quote;
use crate::i18n::Language;
use crate::setup::Ai;
use crate::tmux;

const TASK_PLACEHOLDER: &str = "${task}";
const FRONT_MATTER: &str = "---";
const DEFAULT_SESSION: &str = "tmux-aside";
const BUNDLED_TEMPLATES: &[(&str, &str)] =
    &[("implement", include_str!("../templates/implement.md"))];

/// The prompt an agent is launched with, into which the Task reference is filled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    /// Whether the agent opens a pull request, so that the Launch enters the Lifecycle.
    pub opens_pull_request: bool,
    body: String,
}

#[derive(Deserialize, Default)]
struct FrontMatter {
    #[serde(default)]
    pull_request: bool,
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
            body: body.to_string(),
        })
    }

    /// The prompt with `task` in place of `${task}`.
    pub fn prompt(&self, task: &str) -> String {
        self.body.replace(TASK_PLACEHOLDER, task)
    }
}

/// An agent started by tmux-aside in its own worktree and window.
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
    pub template: String,
    pub task: String,
    pub opens_pull_request: bool,
    pub pull_request: Option<String>,
}

/// What `tmux-aside launch` was given on the command line.
pub struct Options {
    pub repository: PathBuf,
    pub template: String,
    pub task: String,
    pub branch: String,
    pub base: Option<String>,
}

/// Creates the branch and its worktree next to the clone, opens a background window in the
/// current tmux session (or the default one outside tmux), starts the agent there with the
/// filled-in Template and records the Launch. Nothing is created when a check fails.
pub fn run(options: Options, language: Language) -> Result<Launch> {
    let messages = language.messages();
    let template = find_template(&options.template)?
        .ok_or_else(|| anyhow!((messages.unknown_template)(&options.template)))?;
    let repository = git(&options.repository, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .map_err(|_| {
            anyhow!((messages.not_a_repository)(
                &options.repository.display().to_string()
            ))
        })?;
    let base = match options.base {
        Some(base) => base,
        None if ref_exists(&repository, "origin/HEAD") => "origin/HEAD".to_string(),
        None => "HEAD".to_string(),
    };
    if ref_exists(&repository, &format!("refs/heads/{}", options.branch)) {
        bail!((messages.branch_exists)(&options.branch));
    }
    if !ref_exists(&repository, &base) {
        bail!((messages.unknown_base)(&base));
    }
    let handle = options.branch.replace('/', "-");
    let name = file_name(&repository)?;
    let worktree = repository
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", repository.display()))?
        .join(format!("{name}__worktrees"))
        .join(&handle);
    if worktree.exists() {
        bail!((messages.worktree_exists)(&worktree.display().to_string()));
    }
    let ai = Ai::Claude;
    let program = find_on_path(ai.program())
        .ok_or_else(|| anyhow!((messages.agent_not_found)(ai.program())))?;
    let session = match (std::env::var_os("TMUX"), std::env::var("TMUX_PANE")) {
        (Some(_), Ok(pane)) => tmux::run(&["display-message", "-p", "-t", &pane, "#S"])?,
        (Some(_), Err(_)) => tmux::run(&["display-message", "-p", "#S"])?,
        (None, _) => DEFAULT_SESSION.to_string(),
    };
    let id = format!("{name}__{handle}");
    let directory = launches_dir()?;
    let record = directory.join(format!("{id}.json"));
    if record.exists() {
        bail!((messages.launch_exists)(&id));
    }

    let worktree_arg = worktree.to_str().context("worktree path is not UTF-8")?;
    git(
        &repository,
        &[
            "worktree",
            "add",
            "-q",
            "--no-track",
            "-b",
            &options.branch,
            worktree_arg,
            &base,
        ],
    )?;
    std::fs::create_dir_all(&directory)?;
    let prompt = directory.join(format!("{id}.prompt"));
    std::fs::write(&prompt, template.prompt(&options.task))?;
    let command = [
        std::env::current_exe()?.as_path(),
        Path::new("_agent"),
        Path::new("--program"),
        &program,
        &prompt,
    ]
    .iter()
    .map(|part| part.to_str().map(shell_quote))
    .collect::<Option<Vec<_>>>()
    .context("path is not UTF-8")?
    .join(" ");
    let target = format!("={session}:");
    let placement: [&str; 3] = if tmux::run(&["has-session", "-t", &format!("={session}")]).is_ok()
    {
        ["new-window", "-t", &target]
    } else {
        ["new-session", "-s", &session]
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
                &handle,
                "-c",
                worktree_arg,
                &command,
            ],
        ]
        .concat(),
    )?;
    let (window, pane) = opened
        .split_once(' ')
        .ok_or_else(|| anyhow!("unexpected tmux output: {opened}"))?;

    let launch = Launch {
        id,
        repository,
        branch: options.branch,
        worktree,
        session,
        window: window.to_string(),
        pane: pane.to_string(),
        ai,
        template: options.template,
        task: options.task,
        opens_pull_request: template.opens_pull_request,
        pull_request: None,
    };
    write(&record, &launch)?;
    Ok(launch)
}

/// Records `launch`, replacing its previous record.
pub fn save(launch: &Launch) -> Result<()> {
    write(&launches_dir()?.join(format!("{}.json", launch.id)), launch)
}

fn write(record: &Path, launch: &Launch) -> Result<()> {
    let partial = record.with_extension("json.tmp");
    std::fs::write(&partial, serde_json::to_string_pretty(launch)?)
        .and_then(|()| std::fs::rename(&partial, record))
        .with_context(|| format!("failed to write {}", record.display()))
}

/// The line telling where `launch` was opened.
pub fn announce(launch: &Launch, language: Language) -> String {
    (language.messages().launched)(&launch.id, &format!("{}:{}", launch.session, launch.window))
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

/// `launches` as plain text, one block per Launch.
pub fn describe(launches: &[Launch], language: Language) -> String {
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
                (messages.repository, launch.repository.display().to_string()),
                (messages.branch, launch.branch.clone()),
                (messages.worktree, launch.worktree.display().to_string()),
                (
                    messages.window,
                    format!("{} {}", launch.session, launch.window),
                ),
                (
                    messages.agent,
                    format!("{} {}", launch.ai.program(), launch.pane),
                ),
                (messages.template, launch.template.clone()),
                (messages.task, launch.task.clone()),
                (messages.pull_request, pull_request.to_string()),
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

/// Runs the agent with the prompt in `prompt_file`, then leaves a shell in the window.
pub fn start_agent(program: &Path, prompt_file: &Path) -> Result<()> {
    let prompt = std::fs::read_to_string(prompt_file)
        .with_context(|| format!("failed to read {}", prompt_file.display()))?;
    let _ = Command::new(program).arg(prompt).status();
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
    Err(Command::new(shell).arg("-l").exec().into())
}

fn find_template(name: &str) -> Result<Option<Template>> {
    let own = config::templates_dir()?.join(format!("{name}.md"));
    match std::fs::read_to_string(&own) {
        Ok(text) => {
            return Template::parse(&text)
                .with_context(|| format!("invalid template {}", own.display()))
                .map(Some);
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(error).with_context(|| format!("failed to read {}", own.display()));
        }
        Err(_) => {}
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

fn git(directory: &Path, args: &[&str]) -> Result<String> {
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
