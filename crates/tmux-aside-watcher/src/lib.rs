use anyhow::{Result, anyhow};
use serde_json::Value;
use tmux_aside::i18n::Language;
use tmux_aside::launch::Launch;
use tmux_aside::protocol::{Action, Link, Row, Tone};
use tmux_aside_agents::{AgentState, Status};
use tmux_aside_github::{PULL_REQUEST_FIELDS, PullRequest, State, parse_pull_request};

/// Text the section shows, in one language.
pub struct Messages {
    /// Default section title.
    pub title: &'static str,
    pub failed_to_run_gh: &'static str,
    implementing: &'static str,
    ready_to_merge: &'static str,
    no_pull_request: &'static str,
    not_open: &'static str,
    draft: &'static str,
    checks_failing: &'static str,
    conflicts: &'static str,
    changes_requested: &'static str,
    blocked: &'static str,
    checks_running: &'static str,
    waiting_for_approval: &'static str,
    waiting_to_be_merged: &'static str,
    go_to_agent: &'static str,
    open_pull_request: &'static str,
}

const EN: Messages = Messages {
    title: "Lifecycle",
    failed_to_run_gh: "failed to run gh",
    implementing: "Implementing",
    ready_to_merge: "Ready to merge",
    no_pull_request: "no pull request yet",
    not_open: "the pull request is no longer open",
    draft: "draft",
    checks_failing: "checks failing",
    conflicts: "conflicts",
    changes_requested: "changes requested",
    blocked: "blocked",
    checks_running: "checks running",
    waiting_for_approval: "waiting for approval",
    waiting_to_be_merged: "waiting to be merged",
    go_to_agent: "go to the agent",
    open_pull_request: "open the pull request in the browser",
};

const ES: Messages = Messages {
    title: "Ciclo de vida",
    failed_to_run_gh: "no se pudo ejecutar gh",
    implementing: "Implementando",
    ready_to_merge: "Listo para mergear",
    no_pull_request: "aún sin pull request",
    not_open: "el pull request ya no está abierto",
    draft: "borrador",
    checks_failing: "checks fallando",
    conflicts: "conflictos",
    changes_requested: "cambios pedidos",
    blocked: "bloqueado",
    checks_running: "checks en curso",
    waiting_for_approval: "esperando aprobación",
    waiting_to_be_merged: "esperando el merge",
    go_to_agent: "ir al agente",
    open_pull_request: "abrir el pull request en el navegador",
};

/// The section's text in `language`.
pub fn messages(language: Language) -> &'static Messages {
    match language {
        Language::En => &EN,
        Language::Es => &ES,
    }
}

/// A stage of the Lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The agent is still working on the pull request, or it does not exist yet.
    Implementing,
    /// The pull request meets its merge criteria and only waits for someone to merge it.
    ReadyToMerge,
}

impl Stage {
    fn name(self, language: Language) -> &'static str {
        let messages = messages(language);
        match self {
            Stage::Implementing => messages.implementing,
            Stage::ReadyToMerge => messages.ready_to_merge,
        }
    }
}

/// Where a pull request stands: its stage, what is missing to reach the next one, the tone
/// the row is drawn with, and the Nudge that would move it closer, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub stage: Stage,
    pub summary: String,
    pub tone: Tone,
    pub nudge: Option<Nudge>,
}

/// What a Nudge asks the agent to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeKind {
    ResolveConflicts,
    AnswerComments,
}

impl NudgeKind {
    fn name(self) -> &'static str {
        match self {
            NudgeKind::ResolveConflicts => "conflicts",
            NudgeKind::AnswerComments => "comments",
        }
    }
}

/// A command the Watcher types into an idle agent; the agent decides how to do it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nudge {
    pub kind: NudgeKind,
    /// One line, in English like the Templates, since the agent reads it.
    pub text: String,
}

impl Nudge {
    fn for_pull_request(pull_request: &PullRequest) -> Option<Self> {
        let url = &pull_request.url;
        if pull_request.conflicting {
            Some(Nudge {
                kind: NudgeKind::ResolveConflicts,
                text: format!(
                    "The pull request {url} has conflicts with its base branch. Update the branch with its base, resolve the conflicts, run the checks and push."
                ),
            })
        } else if pull_request.unresolved_threads > 0 {
            Some(Nudge {
                kind: NudgeKind::AnswerComments,
                text: format!(
                    "The pull request {url} has {} unresolved review comments. Address or answer each one, push your changes and resolve the threads you fixed.",
                    pull_request.unresolved_threads
                ),
            })
        } else {
            None
        }
    }
}

/// The Nudge to type into the agent of `followed`, with the key that records it was sent,
/// unless one of its kind was already sent for the pull request's head commit.
pub fn pending_nudge(followed: &Followed) -> Option<(String, Nudge)> {
    let pull_request = followed.pull_request.as_ref()?;
    let head = pull_request.head.as_deref()?;
    let nudge = Nudge::for_pull_request(pull_request)?;
    let key = format!("{}@{head}", nudge.kind.name());
    (!followed.launch.nudges.contains(&key)).then_some((key, nudge))
}

/// An agent's tmux pane as it is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePane {
    pub pid: u32,
    /// The command running in the foreground of the pane.
    pub command: String,
    /// Shown to someone: the active pane of the active window of an attached session.
    pub viewed: bool,
}

/// Whether the agent can take a Nudge: it is neither working nor waiting for the user, its
/// pane still runs it (the process and command its hooks registered), and nobody is looking
/// at the pane, where they may be typing.
pub fn is_idle(state: &AgentState, pane: &LivePane) -> bool {
    !matches!(state.status, Some(Status::Working | Status::Waiting))
        && state.pane_pid == pane.pid
        && state.command == pane.command
        && !pane.viewed
}

/// The default Ready to merge Criterion: a pull request is Ready to merge when the github
/// plugin would show it as ready (approved, checks passing, mergeable), and Implementing
/// otherwise, including while it does not exist or is no longer open.
pub fn evaluate(followed: &Followed, language: Language) -> Evaluation {
    let messages = messages(language);
    let Some(pull_request) = &followed.pull_request else {
        return match followed.launch.pull_request {
            Some(_) => implementing(messages.not_open, Tone::Muted),
            None => implementing(messages.no_pull_request, Tone::Accent),
        };
    };
    let evaluation = match pull_request.state(&[]) {
        State::Ready => Evaluation {
            stage: Stage::ReadyToMerge,
            summary: messages.waiting_to_be_merged.to_string(),
            tone: Tone::Success,
            nudge: None,
        },
        State::Draft => implementing(messages.draft, Tone::Accent),
        State::Failing => implementing(messages.checks_failing, Tone::Danger),
        State::Conflicting => implementing(messages.conflicts, Tone::Danger),
        State::ChangesRequested => implementing(messages.changes_requested, Tone::Warning),
        State::Blocked => implementing(messages.blocked, Tone::Warning),
        State::Pending => implementing(messages.checks_running, Tone::Accent),
        State::Review => implementing(messages.waiting_for_approval, Tone::Accent),
    };
    Evaluation {
        nudge: Nudge::for_pull_request(pull_request),
        ..evaluation
    }
}

fn implementing(summary: &str, tone: Tone) -> Evaluation {
    Evaluation {
        stage: Stage::Implementing,
        summary: summary.to_string(),
        tone,
        nudge: None,
    }
}

/// A Launch the Watcher follows, with the open pull request of its branch when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Followed {
    pub launch: Launch,
    pub pull_request: Option<PullRequest>,
}

/// One row per followed Launch, in the given order and grouped by repository. Enter jumps to
/// the agent's pane; `p` opens the pull request in the browser.
pub fn rows(followed: &[Followed], language: Language) -> Vec<Row> {
    let messages = messages(language);
    followed
        .iter()
        .map(|followed| {
            let launch = &followed.launch;
            let evaluation = evaluate(followed, language);
            let mut actions = vec![Action {
                label: Some(messages.go_to_agent.to_string()),
                ..Action::new("enter", &["tmux", "switch-client", "-t", &launch.pane])
            }];
            let known = match &followed.pull_request {
                Some(pr) => Some((pr.title.clone(), format!("#{}", pr.number), pr.url.as_str())),
                None => launch.pull_request.as_deref().map(|url| {
                    let number = url.rsplit('/').next().unwrap_or_default();
                    (launch.task.clone(), format!("#{number}"), url)
                }),
            };
            let mut row = match known {
                Some((title, number, url)) => {
                    actions.push(Action {
                        label: Some(messages.open_pull_request.to_string()),
                        ..Action::new("p", &["gh", "pr", "view", "--web", url])
                    });
                    Row {
                        links: vec![Link::new(number.clone(), url)],
                        ..Row::with_detail(title, number)
                    }
                }
                None => Row::new(launch.task.clone()),
            };
            row.detail_tone = evaluation.tone;
            row.bar_tone = evaluation.tone;
            row.lines = vec![format!(
                "{} · {}",
                evaluation.stage.name(language),
                evaluation.summary
            )];
            row.group = launch
                .repository
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            row.window = Some(launch.window.clone());
            row.actions = actions;
            row
        })
        .collect()
}

/// A branch of a GitHub repository whose open pull request is fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub owner: String,
    pub name: String,
    pub branch: String,
}

impl Target {
    /// The `branch` of the repository `owner/name`.
    pub fn new(owner: &str, name: &str, branch: &str) -> Self {
        Target {
            owner: owner.to_string(),
            name: name.to_string(),
            branch: branch.to_string(),
        }
    }
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

/// One GraphQL query asking for the open pull request of every target, aliased `p0`, `p1`...
/// in order, and its variables (`o0`, `n0`, `b0`...) as name and value.
pub fn batch_query(targets: &[Target]) -> (String, Vec<(String, String)>) {
    let mut declarations = Vec::new();
    let mut selections = Vec::new();
    let mut variables = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        declarations.push(format!(
            "$o{index}: String!, $n{index}: String!, $b{index}: String!"
        ));
        selections.push(format!(
            "p{index}: repository(owner: $o{index}, name: $n{index}) {{ pullRequests(headRefName: $b{index}, states: OPEN, first: 1) {{ nodes {{ ...PullRequestFields }} }} }}"
        ));
        variables.extend([
            (format!("o{index}"), target.owner.clone()),
            (format!("n{index}"), target.name.clone()),
            (format!("b{index}"), target.branch.clone()),
        ]);
    }
    let query = format!(
        "query({}) {{ {} }} {PULL_REQUEST_FIELDS}",
        declarations.join(", "),
        selections.join(" ")
    );
    (query, variables)
}

/// Reads the answer to [`batch_query`] for `count` targets: the pull request of each one,
/// in order, or `None` when its branch has none or its repository was not found.
pub fn parse_batch(output: &str, count: usize) -> Result<Vec<Option<PullRequest>>> {
    let response: Value = serde_json::from_str(output)?;
    let data = response.get("data").filter(|data| data.is_object());
    if let Some(errors) = response.get("errors").and_then(Value::as_array)
        && data.is_none()
    {
        let messages: Vec<&str> = errors
            .iter()
            .filter_map(|error| error.get("message").and_then(Value::as_str))
            .collect();
        return Err(anyhow!("GitHub: {}", messages.join("; ")));
    }
    let data = data.ok_or_else(|| anyhow!("unexpected GitHub response"))?;
    Ok((0..count)
        .map(|index| {
            data.pointer(&format!("/p{index}/pullRequests/nodes/0"))
                .and_then(parse_pull_request)
        })
        .collect())
}
