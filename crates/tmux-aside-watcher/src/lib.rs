use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tmux_aside::i18n::Language;
use tmux_aside::launch::Launch;
pub use tmux_aside::launch::github_repository;
use tmux_aside::protocol::{Action, Link, Row, Tone};
use tmux_aside_agents::{AgentState, Status};
use tmux_aside_github::{PULL_REQUEST_FIELDS, PullRequest, State, parse_pull_request};

const CANDIDATES: usize = 5;

/// Text the section shows, in one language.
pub struct Messages {
    /// Default section title.
    pub title: &'static str,
    pub failed_to_run_gh: &'static str,
    implementing: &'static str,
    ready_to_merge: &'static str,
    no_pull_request: &'static str,
    qa: &'static str,
    ready_to_deploy: &'static str,
    deployed: &'static str,
    closed: &'static str,
    merged: &'static str,
    waiting_for_qa: &'static str,
    qa_failed: &'static str,
    waiting_to_be_deployed: &'static str,
    in_production: &'static str,
    launch_qa: &'static str,
    worktree_removed: &'static str,
    removal_failed: &'static str,
    uncommitted_kept: &'static str,
    unpushed_kept: &'static str,
    draft: &'static str,
    checks_failing: &'static str,
    conflicts: &'static str,
    changes_requested: &'static str,
    blocked: &'static str,
    checks_running: &'static str,
    waiting_for_approval: &'static str,
    waiting_to_be_merged: &'static str,
    checking: &'static str,
    criterion_failed: fn(&str) -> String,
    /// A Criterion ran longer than its timeout, in seconds.
    pub timed_out: fn(f32) -> String,
    /// A Criterion exited with this status.
    pub exited: fn(i32) -> String,
    /// A Criterion printed this instead of an answer.
    pub invalid_answer: fn(&str) -> String,
    /// A Criterion could not be started.
    pub failed_to_start: fn(&str) -> String,
    /// The config holding the Criteria could not be read.
    pub invalid_config: fn(&str) -> String,
    go_to_agent: &'static str,
    open_pull_request: &'static str,
}

const EN: Messages = Messages {
    title: "Lifecycle",
    failed_to_run_gh: "failed to run gh",
    implementing: "Implementing",
    ready_to_merge: "Ready to merge",
    no_pull_request: "no pull request yet",
    qa: "QA",
    ready_to_deploy: "Ready to deploy",
    deployed: "Deployed",
    closed: "Closed",
    merged: "merged",
    waiting_for_qa: "waiting for QA",
    qa_failed: "QA failed",
    waiting_to_be_deployed: "waiting to be deployed",
    in_production: "in production",
    launch_qa: "launch QA",
    worktree_removed: "worktree removed",
    removal_failed: "the worktree could not be removed",
    uncommitted_kept: "uncommitted changes kept in the worktree",
    unpushed_kept: "unpushed commits kept in the worktree",
    draft: "draft",
    checks_failing: "checks failing",
    conflicts: "conflicts",
    changes_requested: "changes requested",
    blocked: "blocked",
    checks_running: "checks running",
    waiting_for_approval: "waiting for approval",
    waiting_to_be_merged: "waiting to be merged",
    checking: "checking...",
    criterion_failed: |error| format!("criterion failed: {error}"),
    timed_out: |seconds| format!("timed out after {seconds}s"),
    exited: |code| format!("exited with status {code}"),
    invalid_answer: |output| format!("invalid answer: {output}"),
    failed_to_start: |program| format!("failed to start {program}"),
    invalid_config: |error| format!("invalid config: {error}"),
    go_to_agent: "go to the agent",
    open_pull_request: "open the pull request in the browser",
};

const ES: Messages = Messages {
    title: "Ciclo de vida",
    failed_to_run_gh: "no se pudo ejecutar gh",
    implementing: "Implementando",
    ready_to_merge: "Listo para mergear",
    no_pull_request: "aún sin pull request",
    qa: "QA",
    ready_to_deploy: "Listo para desplegar",
    deployed: "Desplegado",
    closed: "Cerrado",
    merged: "mergeado",
    waiting_for_qa: "esperando QA",
    qa_failed: "QA falló",
    waiting_to_be_deployed: "esperando el despliegue",
    in_production: "en producción",
    launch_qa: "lanzar QA",
    worktree_removed: "worktree eliminado",
    removal_failed: "no se pudo eliminar el worktree",
    uncommitted_kept: "cambios sin commitear conservados en el worktree",
    unpushed_kept: "commits sin push conservados en el worktree",
    draft: "borrador",
    checks_failing: "checks fallando",
    conflicts: "conflictos",
    changes_requested: "cambios pedidos",
    blocked: "bloqueado",
    checks_running: "checks en curso",
    waiting_for_approval: "esperando aprobación",
    waiting_to_be_merged: "esperando el merge",
    checking: "revisando...",
    criterion_failed: |error| format!("el criterio falló: {error}"),
    timed_out: |seconds| format!("sin respuesta tras {seconds}s"),
    exited: |code| format!("terminó con estado {code}"),
    invalid_answer: |output| format!("respuesta inválida: {output}"),
    failed_to_start: |program| format!("no se pudo iniciar {program}"),
    invalid_config: |error| format!("config inválida: {error}"),
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
    /// The merged change is verified in a pre-production environment.
    Qa,
    /// QA passed and the change only waits to be promoted to production.
    ReadyToDeploy,
    /// The change reached production; the pull request leaves the Lifecycle.
    Deployed,
    /// The pull request was closed without being merged; it leaves the Lifecycle.
    Closed,
}

impl Stage {
    /// The stage's name in a Criterion's [`Question`].
    pub fn key(self) -> &'static str {
        match self {
            Stage::Implementing => "implementing",
            Stage::ReadyToMerge => "ready_to_merge",
            Stage::Qa => "qa",
            Stage::ReadyToDeploy => "ready_to_deploy",
            Stage::Deployed => "deployed",
            Stage::Closed => "closed",
        }
    }

    fn name(self, language: Language) -> &'static str {
        let messages = messages(language);
        match self {
            Stage::Implementing => messages.implementing,
            Stage::ReadyToMerge => messages.ready_to_merge,
            Stage::Qa => messages.qa,
            Stage::ReadyToDeploy => messages.ready_to_deploy,
            Stage::Deployed => messages.deployed,
            Stage::Closed => messages.closed,
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
    /// A command for the agent, one line, in English like the Templates since the agent reads it.
    pub nudge: Option<String>,
}

/// What the Watcher asks a Criterion on its stdin, as one JSON object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// The stage the Criterion decides: `ready_to_merge`, `qa`, `ready_to_deploy` or `deployed`.
    pub stage: String,
    /// The pull request's URL.
    pub url: String,
    /// What the Watcher already fetched about the pull request.
    pub pull_request: PullRequest,
}

/// What a Criterion answers on its stdout, as one JSON object. Unknown fields are ignored and
/// every field but `met` is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    /// Whether the pull request reached the stage.
    pub met: bool,
    /// What is missing to reach it, or where the pull request stands once it is met.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The row's tone; `accent` until met and `success` once met when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<Tone>,
    /// A command to type into the agent, once per head commit, while it is idle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nudge: Option<String>,
    /// For the Deployed Criterion: whether the change is in production.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployed: Option<bool>,
}

/// The default Ready to merge Criterion: met when the github plugin would show the pull
/// request as ready (approved, checks passing, mergeable). It asks for a Nudge on conflicts,
/// then on unresolved review comments.
pub fn ready_to_merge(pull_request: &PullRequest, language: Language) -> Answer {
    let messages = messages(language);
    if pull_request.merged {
        return met(messages.merged);
    }
    let (met, summary, tone) = match pull_request.state(&[]) {
        State::Ready => (true, messages.waiting_to_be_merged, Tone::Success),
        State::Draft => (false, messages.draft, Tone::Accent),
        State::Failing => (false, messages.checks_failing, Tone::Danger),
        State::Conflicting => (false, messages.conflicts, Tone::Danger),
        State::ChangesRequested => (false, messages.changes_requested, Tone::Warning),
        State::Blocked => (false, messages.blocked, Tone::Warning),
        State::Pending => (false, messages.checks_running, Tone::Accent),
        State::Review => (false, messages.waiting_for_approval, Tone::Accent),
    };
    let url = &pull_request.url;
    let nudge = if pull_request.conflicting {
        Some(format!(
            "The pull request {url} has conflicts with its base branch. Update the branch with its base, resolve the conflicts, run the checks and push."
        ))
    } else if pull_request.unresolved_threads > 0 {
        Some(format!(
            "The pull request {url} has {} unresolved review comments. Address or answer each one, push your changes and resolve the threads you fixed.",
            pull_request.unresolved_threads
        ))
    } else {
        None
    };
    Answer {
        met,
        summary: Some(summary.to_string()),
        tone: Some(tone),
        nudge,
        deployed: None,
    }
}

fn met(summary: &str) -> Answer {
    Answer {
        met: true,
        summary: Some(summary.to_string()),
        ..Answer::default()
    }
}

/// The default QA Criterion: met once the pull request is merged.
pub fn qa(pull_request: &PullRequest, language: Language) -> Answer {
    let messages = messages(language);
    match pull_request.merged {
        true => met(messages.waiting_for_qa),
        false => Answer {
            met: false,
            summary: Some(messages.waiting_to_be_merged.to_string()),
            tone: Some(Tone::Success),
            ..Answer::default()
        },
    }
}

/// The default Ready to deploy Criterion: met once the pull request carries the `passed`
/// label; the `failed` label shows that QA failed.
pub fn ready_to_deploy(
    pull_request: &PullRequest,
    passed: &str,
    failed: &str,
    language: Language,
) -> Answer {
    let messages = messages(language);
    let labeled = |name: &str| pull_request.labels.iter().any(|label| label == name);
    if labeled(passed) {
        met(messages.waiting_to_be_deployed)
    } else if labeled(failed) {
        Answer {
            met: false,
            summary: Some(messages.qa_failed.to_string()),
            tone: Some(Tone::Danger),
            ..Answer::default()
        }
    } else {
        Answer {
            met: false,
            summary: Some(messages.waiting_for_qa.to_string()),
            ..Answer::default()
        }
    }
}

/// The default Deployed Criterion: a pull request counts as deployed as soon as it is Ready to
/// deploy, since tmux-aside knows no deploy pipeline.
pub fn deployed(language: Language) -> Answer {
    Answer {
        deployed: Some(true),
        ..met(messages(language).in_production)
    }
}

/// Whether `answer` means `stage` is reached for `pull_request`: `deployed` decides for
/// Deployed, when given, and a merged pull request is always Ready to merge.
pub fn reached(stage: Stage, answer: &Answer, pull_request: &PullRequest) -> bool {
    match stage {
        Stage::ReadyToMerge => answer.met || pull_request.merged,
        Stage::Deployed => answer.deployed.unwrap_or(answer.met),
        _ => answer.met,
    }
}

/// Asks `ask` for the answer of each of `stages` in Lifecycle order, up to the first one
/// `pull_request` does not reach or whose Criterion fails.
pub fn ask_in_order(
    stages: &[Stage],
    pull_request: &PullRequest,
    mut ask: impl FnMut(Stage) -> Result<Answer, String>,
) -> Answers {
    let mut answers = Vec::new();
    for &stage in stages {
        let answer = ask(stage);
        let go_on = matches!(&answer, Ok(answer) if reached(stage, answer, pull_request));
        answers.push((stage, answer));
        if !go_on {
            break;
        }
    }
    answers
}

/// The Nudge to type into the agent of `followed`, with the key that records it was sent,
/// unless one was already sent at its stage for the pull request's head commit.
pub fn pending_nudge(followed: &Followed, language: Language) -> Option<(String, String)> {
    let head = followed.pull_request.as_ref()?.head.as_deref()?;
    let evaluation = evaluate(followed, language);
    let nudge = evaluation.nudge?;
    let key = format!("{}@{head}", evaluation.stage.key());
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

/// Where `followed` stands. A pull request starts at Implementing and moves through each stage
/// whose Criterion answered met, in Lifecycle order; the first unmet one gives the summary,
/// tone and Nudge (the last met one's Nudge when it asks for none), and a failed one shows its
/// error. A closed one leaves the Lifecycle; a worktree kept after the merge or the close
/// replaces the summary with what it keeps.
pub fn evaluate(followed: &Followed, language: Language) -> Evaluation {
    let messages = messages(language);
    let Some(pull_request) = &followed.pull_request else {
        return implementing(messages.no_pull_request, Tone::Accent);
    };
    let kept = match followed.worktree {
        Some(Worktree::Uncommitted) => Some(messages.uncommitted_kept),
        Some(Worktree::Unpushed) => Some(messages.unpushed_kept),
        Some(Worktree::Clean) => Some(messages.removal_failed),
        _ => None,
    };
    if pull_request.closed {
        return Evaluation {
            stage: Stage::Closed,
            summary: kept.unwrap_or(messages.worktree_removed).to_string(),
            tone: kept.map_or(Tone::Muted, |_| Tone::Warning),
            nudge: None,
        };
    }
    let evaluation = walk(followed, pull_request, language);
    match kept {
        Some(kept) if evaluation.tone != Tone::Danger => Evaluation {
            summary: kept.to_string(),
            tone: Tone::Warning,
            ..evaluation
        },
        _ => evaluation,
    }
}

fn walk(followed: &Followed, pull_request: &PullRequest, language: Language) -> Evaluation {
    let messages = messages(language);
    let mut stage = Stage::Implementing;
    let mut last: Option<&Answer> = None;
    for (next, answer) in &followed.answers {
        match answer {
            Err(error) => {
                return Evaluation {
                    stage,
                    summary: (messages.criterion_failed)(error),
                    tone: Tone::Danger,
                    nudge: None,
                };
            }
            Ok(answer) if reached(*next, answer, pull_request) => {
                stage = *next;
                last = Some(answer);
            }
            Ok(answer) => {
                return Evaluation {
                    stage,
                    summary: answer.summary.clone().unwrap_or_default(),
                    tone: answer.tone.unwrap_or(Tone::Accent),
                    nudge: answer
                        .nudge
                        .clone()
                        .or_else(|| last.and_then(|reached| reached.nudge.clone())),
                };
            }
        }
    }
    match last {
        Some(answer) => Evaluation {
            stage,
            summary: answer.summary.clone().unwrap_or_default(),
            tone: answer.tone.unwrap_or(Tone::Success),
            nudge: answer.nudge.clone(),
        },
        None => implementing(messages.checking, Tone::Accent),
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

/// A Launch the Watcher follows, with the newest pull request of its branch when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Followed {
    pub launch: Launch,
    pub pull_request: Option<PullRequest>,
    /// The state of the Launch's worktree, inspected once its pull request is merged or closed.
    pub worktree: Option<Worktree>,
    /// What the Criteria of the stages after Implementing answered, in Lifecycle order, up to
    /// the first unmet or failed one; an error is the reason the Criterion failed.
    pub answers: Answers,
}

/// What the Criteria of a pull request's stages answered, in Lifecycle order; an error is the
/// reason a Criterion failed.
pub type Answers = Vec<(Stage, Result<Answer, String>)>;

/// What removing a Launch's worktree would lose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Worktree {
    /// Not a git worktree any more, such as after it was removed.
    Missing,
    /// The clone's main working tree, where a followed agent may run; never removed.
    Main,
    /// Nothing would be lost. Seen after a cleanup, it means the removal failed.
    Clean,
    /// Files changed and not committed.
    Uncommitted,
    /// Commits on no remote branch and not the pull request's head.
    Unpushed,
}

/// Whether `followed` leaves the Watcher: its pull request was deployed or closed, and its
/// worktree removed or never its own.
pub fn leaves(followed: &Followed, language: Language) -> bool {
    matches!(
        evaluate(followed, language).stage,
        Stage::Deployed | Stage::Closed
    ) && matches!(followed.worktree, Some(Worktree::Missing | Worktree::Main))
}

/// Whether the Watcher removes the Launch's worktree and closes its window now: only once the
/// pull request is merged or closed, and only when nothing would be lost.
pub fn should_clean_up(followed: &Followed) -> bool {
    followed
        .pull_request
        .as_ref()
        .is_some_and(|pr| pr.finished())
        && followed.worktree == Some(Worktree::Clean)
}

/// One row per followed Launch, in the given order and grouped by repository. Enter jumps to
/// the agent's pane; `p` opens the pull request in the browser; `q`, in QA, launches a QA
/// agent for it with `tmux_aside`, the path to the `tmux-aside` command.
pub fn rows(followed: &[Followed], tmux_aside: &str, language: Language) -> Vec<Row> {
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
            let mut row = match &followed.pull_request {
                Some(pr) => {
                    let number = format!("#{}", pr.number);
                    actions.push(Action {
                        label: Some(messages.open_pull_request.to_string()),
                        ..Action::new("p", &["gh", "pr", "view", "--web", &pr.url])
                    });
                    Row {
                        links: vec![Link::new(number.clone(), pr.url.clone())],
                        ..Row::with_detail(pr.title.clone(), number)
                    }
                }
                None => Row::new(launch.task.clone()),
            };
            if let (Stage::Qa, Some(pr)) = (evaluation.stage, &followed.pull_request) {
                actions.push(Action {
                    label: Some(messages.launch_qa.to_string()),
                    ..Action::new("q", &[tmux_aside, "qa", &pr.url])
                });
            }
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

/// A branch of a GitHub repository whose newest pull request is fetched.
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

/// One GraphQL query asking for the newest pull requests of every target, aliased `p0`, `p1`...
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
            "p{index}: repository(owner: $o{index}, name: $n{index}) {{ pullRequests(headRefName: $b{index}, first: {CANDIDATES}, orderBy: {{field: CREATED_AT, direction: DESC}}) {{ nodes {{ ...PullRequestFields }} }} }}"
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
pub fn parse_batch(output: &str, count: usize) -> Result<Vec<Vec<PullRequest>>> {
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
            data.pointer(&format!("/p{index}/pullRequests/nodes"))
                .and_then(Value::as_array)
                .map_or_else(Vec::new, |nodes| {
                    nodes.iter().filter_map(parse_pull_request).collect()
                })
        })
        .collect())
}

/// The pull request of a Launch among its branch's newest `candidates`, newest first. Ones from
/// another repository never count. An open one wins. A merged or closed one counts only when
/// it is the one already recorded for the Launch, or its head is the worktree's `local_head`,
/// so that an old pull request of a reused branch name is never taken for the Launch's own.
pub fn choose_pull_request(
    candidates: Vec<PullRequest>,
    recorded: Option<&str>,
    local_head: Option<&str>,
) -> Option<PullRequest> {
    let mut own: Vec<PullRequest> = candidates
        .into_iter()
        .filter(|pr| !pr.cross_repository)
        .collect();
    let position = own
        .iter()
        .position(|pr| !pr.finished())
        .or_else(|| own.iter().position(|pr| Some(pr.url.as_str()) == recorded))
        .or_else(|| {
            own.iter()
                .position(|pr| local_head.is_some() && pr.head.as_deref() == local_head)
        })?;
    Some(own.swap_remove(position))
}
