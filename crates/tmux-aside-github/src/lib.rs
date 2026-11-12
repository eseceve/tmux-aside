use anyhow::{Result, anyhow};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::Value;
use tmux_aside::i18n::Language;
use tmux_aside::protocol::{Action, Row, Tone};

const COPY_SCRIPT: &str = "if command -v pbcopy >/dev/null; then printf %s \"$1\" | pbcopy; \
elif command -v wl-copy >/dev/null; then printf %s \"$1\" | wl-copy; \
else printf %s \"$1\" | xclip -selection clipboard; fi";

/// GraphQL fragment `PullRequestFields` selecting the fields a [`PullRequest`] is read from.
pub const PULL_REQUEST_FIELDS: &str = "fragment PullRequestFields on PullRequest { number title url headRefOid state isCrossRepository isDraft additions deletions reviewDecision createdAt updatedAt author { login } labels(first: 50) { nodes { name } } mergeable reviewThreads(first: 100) { nodes { isResolved } } latestOpinionatedReviews(first: 20) { nodes { state } } baseRef { branchProtectionRule { requiredApprovingReviewCount } } repository { name nameWithOwner } commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 50) { nodes { __typename ... on CheckRun { name status } ... on StatusContext { context state } } } } } } } }";

/// GraphQL query listing pull requests that match a GitHub search (`$q`), with the fields
/// the rows show.
pub fn search_query() -> String {
    format!(
        "query($q: String!, $limit: Int!) {{ search(query: $q, type: ISSUE_ADVANCED, first: $limit) {{ nodes {{ ...PullRequestFields }} }} }} {PULL_REQUEST_FIELDS}"
    )
}

/// Text the section shows, in one language.
pub struct Messages {
    /// Default section title.
    pub title: &'static str,
    pub failed_to_run_gh: &'static str,
    approvals_of: fn(usize, u32) -> String,
    one_approval: &'static str,
    approvals: fn(usize) -> String,
    unresolved: fn(usize) -> String,
    open_in_browser: &'static str,
    copy_url: &'static str,
}

const EN: Messages = Messages {
    title: "Pull requests",
    failed_to_run_gh: "failed to run gh",
    approvals_of: |approvals, required| format!("{approvals}/{required} approvals"),
    one_approval: "1 approval",
    approvals: |approvals| format!("{approvals} approvals"),
    unresolved: |threads| format!("{threads} unresolved"),
    open_in_browser: "open in the browser",
    copy_url: "copy the URL",
};

const ES: Messages = Messages {
    title: "Pull requests",
    failed_to_run_gh: "no se pudo ejecutar gh",
    approvals_of: |approvals, required| format!("{approvals}/{required} aprobaciones"),
    one_approval: "1 aprobación",
    approvals: |approvals| format!("{approvals} aprobaciones"),
    unresolved: |threads| format!("{threads} sin resolver"),
    open_in_browser: "abrir en el navegador",
    copy_url: "copiar la URL",
};

/// The section's text in `language`.
pub fn messages(language: Language) -> &'static Messages {
    match language {
        Language::En => &EN,
        Language::Es => &ES,
    }
}

/// A pull request returned by the search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub repository: String,
    /// `owner/name` of the repository.
    pub full_name: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    /// The commit the pull request's branch points to.
    pub head: Option<String>,
    /// Merged into its base branch.
    pub merged: bool,
    /// Closed without being merged.
    pub closed: bool,
    /// Opened from a branch of another repository, such as a fork.
    pub cross_repository: bool,
    pub draft: bool,
    /// GitHub's review decision. Repositories that do not require reviews have none, so it
    /// comes from the latest reviews: a change request wins over an approval.
    pub review: Option<String>,
    pub checks: Option<String>,
    /// Names of the checks that have not finished yet.
    pub running_checks: Vec<String>,
    pub author: String,
    /// The pull request cannot merge cleanly into its base branch.
    pub conflicting: bool,
    pub unresolved_threads: usize,
    pub approvals: usize,
    /// Approvals the base branch requires, when its protection rule is visible.
    pub required_approvals: Option<u32>,
    pub created_at: String,
    pub updated_at: String,
    /// Lines added and removed.
    pub additions: u64,
    pub deletions: u64,
    pub labels: Vec<String>,
}

/// Parses the response of [`search_query`], skipping results that are not pull requests.
pub fn parse_search(output: &str) -> Result<Vec<PullRequest>> {
    let response: Value = serde_json::from_str(output)?;
    if let Some(errors) = response.get("errors").and_then(Value::as_array) {
        let messages: Vec<&str> = errors
            .iter()
            .filter_map(|error| error.get("message").and_then(Value::as_str))
            .collect();
        return Err(anyhow!("GitHub: {}", messages.join("; ")));
    }
    let nodes = response
        .pointer("/data/search/nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("unexpected GitHub response"))?;
    Ok(nodes.iter().filter_map(parse_pull_request).collect())
}

/// Reads a pull request selected with [`PULL_REQUEST_FIELDS`], or `None` when `node` is not one.
pub fn parse_pull_request(node: &Value) -> Option<PullRequest> {
    serde_json::from_value::<Node>(node.clone())
        .ok()
        .map(Node::into_pull_request)
}

/// Part of a pull request row that is colored by the pull request's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Part {
    /// A glyph before the title; it is only drawn when colored.
    Icon,
    Title,
    Number,
    /// The bar along the row; muted when not colored.
    Bar,
}

/// What a pull request is waiting on, from the most to the least urgent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Failing,
    Conflicting,
    ChangesRequested,
    /// Carries a label that says it must not be merged yet.
    Blocked,
    Pending,
    Ready,
    Review,
    Draft,
}

impl PullRequest {
    /// Merged or closed: no longer open.
    pub fn finished(&self) -> bool {
        self.merged || self.closed
    }

    /// Drafts are always `Draft`; otherwise failing checks win over a change request, which
    /// wins over any of `blocked_labels`, which wins over running checks. Approved pull requests
    /// with passing (or no) checks are `Ready`.
    pub fn state(&self, blocked_labels: &[String]) -> State {
        let checks = self.checks.as_deref();
        let review = self.review.as_deref();
        if self.draft {
            State::Draft
        } else if matches!(checks, Some("FAILURE" | "ERROR")) {
            State::Failing
        } else if self.conflicting {
            State::Conflicting
        } else if review == Some("CHANGES_REQUESTED") {
            State::ChangesRequested
        } else if self
            .labels
            .iter()
            .any(|label| blocked_labels.contains(label))
        {
            State::Blocked
        } else if checks.is_some_and(|checks| checks != "SUCCESS") {
            State::Pending
        } else if review == Some("APPROVED") {
            State::Ready
        } else {
            State::Review
        }
    }
}

impl State {
    fn style(self) -> (&'static str, Tone) {
        match self {
            State::Failing => ("×", Tone::Danger),
            State::Conflicting => ("≠", Tone::Danger),
            State::ChangesRequested => ("◆", Tone::Warning),
            State::Blocked => ("⊘", Tone::Warning),
            State::Pending => ("◷", Tone::Accent),
            State::Ready => ("✓", Tone::Success),
            State::Review => ("○", Tone::Muted),
            State::Draft => ("◌", Tone::Muted),
        }
    }
}

/// What pull request rows show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowOptions {
    /// Parts colored by the pull request's [`State`].
    pub colored: Vec<Part>,
    /// Current time; when set each row adds a line with its author and age.
    pub byline_at: Option<Timestamp>,
    /// Add a line with the running checks, the unresolved review threads and the approvals.
    pub merge_info: bool,
    /// Start the first extra line with the lines added and removed, as `+N -M`.
    pub diff: bool,
    /// Add a line with the labels that start with any of these prefixes, when there is one.
    pub label_prefixes: Vec<String>,
    /// Labels that make a pull request [`State::Blocked`].
    pub blocked_labels: Vec<String>,
    /// Language of the text the rows add.
    pub language: Language,
}

/// One row per pull request, grouped by repository (alphabetical) with the most recently
/// updated first and a bar along each row, muted unless colored. Enter opens the pull request
/// in the browser.
pub fn rows(pull_requests: &[PullRequest], options: &RowOptions) -> Vec<Row> {
    let mut sorted: Vec<&PullRequest> = pull_requests.iter().collect();
    sorted.sort_by(|a, b| {
        a.repository
            .cmp(&b.repository)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    sorted
        .into_iter()
        .map(|pr| {
            let mut lines: Vec<String> = options
                .byline_at
                .map(|now| byline(pr, now))
                .into_iter()
                .chain(options.merge_info.then(|| merge_info(pr, options.language)))
                .chain(labels(pr, &options.label_prefixes))
                .collect();
            if options.diff {
                let diff = format!("+{} -{}", pr.additions, pr.deletions);
                match lines.first_mut() {
                    Some(first) => *first = format!("{diff} · {first}"),
                    None => lines.push(diff),
                }
            }
            Row {
                lines,
                ..row(pr, options)
            }
        })
        .collect()
}

fn byline(pr: &PullRequest, now: Timestamp) -> String {
    match pr.created_at.parse::<Timestamp>() {
        Ok(created) => {
            let seconds = now.as_second().saturating_sub(created.as_second()).max(0);
            format!("@{} · {}", pr.author, format_age(seconds.unsigned_abs()))
        }
        Err(_) => format!("@{}", pr.author),
    }
}

fn labels(pr: &PullRequest, prefixes: &[String]) -> Option<String> {
    let mut matching: Vec<&str> = pr
        .labels
        .iter()
        .filter(|label| {
            prefixes
                .iter()
                .any(|prefix| label.starts_with(prefix.as_str()))
        })
        .map(String::as_str)
        .collect();
    matching.sort_unstable();
    (!matching.is_empty()).then(|| matching.join(" · "))
}

/// `◷ Jenkins · 2 unresolved · 1/2 approvals`; the running checks are left out when none is
/// running, the threads part when every thread is resolved, and the required count when the
/// base branch requires none or its protection is not visible.
pub fn merge_info(pr: &PullRequest, language: Language) -> String {
    let messages = messages(language);
    let approvals = match pr.required_approvals.filter(|required| *required > 0) {
        Some(required) => (messages.approvals_of)(pr.approvals, required),
        None if pr.approvals == 1 => messages.one_approval.to_string(),
        None => (messages.approvals)(pr.approvals),
    };
    let running =
        (!pr.running_checks.is_empty()).then(|| format!("◷ {}", pr.running_checks.join(", ")));
    let unresolved =
        (pr.unresolved_threads > 0).then(|| (messages.unresolved)(pr.unresolved_threads));
    running
        .into_iter()
        .chain(unresolved)
        .chain([approvals])
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Formats an age in seconds as `45m`, `3h` or `2d`.
pub fn format_age(seconds: u64) -> String {
    match seconds / 60 {
        minutes @ 0..60 => format!("{minutes}m"),
        minutes @ 60..1_440 => format!("{}h", minutes / 60),
        minutes => format!("{}d", minutes / 1_440),
    }
}

fn row(pr: &PullRequest, options: &RowOptions) -> Row {
    let messages = messages(options.language);
    let colored = &options.colored;
    let (icon, state_tone) = pr.state(&options.blocked_labels).style();
    let tone = |part: Part, otherwise: Tone| match colored.contains(&part) {
        true => state_tone,
        false => otherwise,
    };
    Row {
        icon: colored.contains(&Part::Icon).then(|| icon.to_string()),
        icon_tone: state_tone,
        tone: tone(Part::Title, Tone::Plain),
        detail_tone: tone(Part::Number, Tone::Muted),
        bar_tone: tone(Part::Bar, Tone::Muted),
        group: Some(pr.repository.clone()),
        actions: vec![
            Action {
                label: Some(messages.open_in_browser.to_string()),
                ..Action::new("enter", &["gh", "pr", "view", "--web", &pr.url])
            },
            Action {
                label: Some(messages.copy_url.to_string()),
                ..Action::new("c", &["sh", "-c", COPY_SCRIPT, "copy", &pr.url])
            },
        ],
        ..Row::with_detail(pr.title.clone(), format!("#{}", pr.number))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Node {
    number: u64,
    title: String,
    url: String,
    #[serde(default)]
    head_ref_oid: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    is_cross_repository: bool,
    is_draft: bool,
    #[serde(default)]
    additions: u64,
    #[serde(default)]
    deletions: u64,
    review_decision: Option<String>,
    created_at: String,
    updated_at: String,
    author: Option<Author>,
    #[serde(default)]
    labels: Option<Nodes<Label>>,
    #[serde(default)]
    mergeable: Option<String>,
    #[serde(default)]
    review_threads: Option<Nodes<Thread>>,
    #[serde(default)]
    latest_opinionated_reviews: Option<Nodes<Review>>,
    #[serde(default)]
    base_ref: Option<BaseRef>,
    repository: Repository,
    commits: Commits,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Thread {
    is_resolved: bool,
}

#[derive(Deserialize)]
struct Review {
    state: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BaseRef {
    branch_protection_rule: Option<ProtectionRule>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProtectionRule {
    required_approving_review_count: Option<u32>,
}

#[derive(Deserialize)]
struct Label {
    name: String,
}

#[derive(Deserialize)]
struct Author {
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    name: String,
    name_with_owner: String,
}

#[derive(Deserialize)]
struct Commits {
    nodes: Vec<CommitNode>,
}

#[derive(Deserialize)]
struct CommitNode {
    commit: Commit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Commit {
    status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
struct Rollup {
    state: String,
    #[serde(default)]
    contexts: Option<Nodes<CheckContext>>,
}

#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum CheckContext {
    CheckRun { name: String, status: String },
    StatusContext { context: String, state: String },
}

impl CheckContext {
    fn running_name(self) -> Option<String> {
        match self {
            CheckContext::CheckRun { name, status } => (status != "COMPLETED").then_some(name),
            CheckContext::StatusContext { context, state } => {
                matches!(state.as_str(), "PENDING" | "EXPECTED").then_some(context)
            }
        }
    }
}

impl Node {
    fn into_pull_request(self) -> PullRequest {
        let rollup = self
            .commits
            .nodes
            .into_iter()
            .next()
            .and_then(|node| node.commit.status_check_rollup);
        let (checks, running_checks) = match rollup {
            Some(rollup) => (
                Some(rollup.state),
                rollup.contexts.map_or_else(Vec::new, |contexts| {
                    contexts
                        .nodes
                        .into_iter()
                        .filter_map(CheckContext::running_name)
                        .collect()
                }),
            ),
            None => (None, Vec::new()),
        };
        let reviews: Vec<String> =
            self.latest_opinionated_reviews
                .map_or_else(Vec::new, |reviews| {
                    reviews
                        .nodes
                        .into_iter()
                        .map(|review| review.state)
                        .collect()
                });
        let review = self.review_decision.or_else(|| {
            ["CHANGES_REQUESTED", "APPROVED"]
                .into_iter()
                .find(|state| reviews.iter().any(|review| review == state))
                .map(str::to_string)
        });
        PullRequest {
            repository: self.repository.name,
            full_name: self.repository.name_with_owner,
            number: self.number,
            title: self.title,
            url: self.url,
            head: self.head_ref_oid,
            merged: self.state.as_deref() == Some("MERGED"),
            closed: self.state.as_deref() == Some("CLOSED"),
            cross_repository: self.is_cross_repository,
            draft: self.is_draft,
            additions: self.additions,
            deletions: self.deletions,
            review,
            checks,
            running_checks,
            author: self
                .author
                .map_or_else(|| "ghost".to_string(), |author| author.login),
            conflicting: self.mergeable.as_deref() == Some("CONFLICTING"),
            unresolved_threads: self.review_threads.map_or(0, |threads| {
                threads
                    .nodes
                    .iter()
                    .filter(|thread| !thread.is_resolved)
                    .count()
            }),
            approvals: reviews
                .iter()
                .filter(|review| *review == "APPROVED")
                .count(),
            required_approvals: self
                .base_ref
                .and_then(|base| base.branch_protection_rule)
                .and_then(|rule| rule.required_approving_review_count),
            created_at: self.created_at,
            updated_at: self.updated_at,
            labels: self.labels.map_or_else(Vec::new, |labels| {
                labels.nodes.into_iter().map(|label| label.name).collect()
            }),
        }
    }
}
