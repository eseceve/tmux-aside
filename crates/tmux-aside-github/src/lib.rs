use anyhow::{Result, anyhow};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::Value;
use tmux_aside::protocol::{Action, Row, Tone};

const COPY_SCRIPT: &str = "if command -v pbcopy >/dev/null; then printf %s \"$1\" | pbcopy; \
elif command -v wl-copy >/dev/null; then printf %s \"$1\" | wl-copy; \
else printf %s \"$1\" | xclip -selection clipboard; fi";

/// GraphQL query listing pull requests that match a GitHub search (`$q`), with the fields
/// the rows show.
pub const SEARCH_QUERY: &str = "query($q: String!, $limit: Int!) { search(query: $q, type: ISSUE_ADVANCED, first: $limit) { nodes { ... on PullRequest { number title url isDraft additions deletions reviewDecision createdAt updatedAt author { login } mergeable reviewThreads(first: 100) { nodes { isResolved } } latestOpinionatedReviews(first: 20) { nodes { state } } baseRef { branchProtectionRule { requiredApprovingReviewCount } } repository { name nameWithOwner } commits(last: 1) { nodes { commit { statusCheckRollup { state } } } } } } } }";

/// A pull request returned by the search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub repository: String,
    /// `owner/name` of the repository.
    pub full_name: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub draft: bool,
    pub review: Option<String>,
    pub checks: Option<String>,
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
}

/// Parses the response of [`SEARCH_QUERY`], skipping results that are not pull requests.
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
    Ok(nodes
        .iter()
        .filter_map(|node| serde_json::from_value::<Node>(node.clone()).ok())
        .map(Node::into_pull_request)
        .collect())
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
    Pending,
    Ready,
    Review,
    Draft,
}

impl PullRequest {
    /// Drafts are always `Draft`; otherwise failing checks win over a change request, which
    /// wins over running checks. Approved pull requests with passing (or no) checks are `Ready`.
    pub fn state(&self) -> State {
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
    /// Add a line with the unresolved review threads and the approvals.
    pub merge_info: bool,
    /// Start the first extra line with the lines added and removed, as `+N -M`.
    pub diff: bool,
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
                .chain(options.merge_info.then(|| merge_info(pr)))
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
                ..row(pr, &options.colored)
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

/// `2 unresolved · 1/2 approvals`; the threads part is left out when every thread is
/// resolved, and the required count when the base branch requires none or its protection is
/// not visible.
pub fn merge_info(pr: &PullRequest) -> String {
    let approvals = match pr.required_approvals.filter(|required| *required > 0) {
        Some(required) => format!("{}/{required} approvals", pr.approvals),
        None if pr.approvals == 1 => "1 approval".to_string(),
        None => format!("{} approvals", pr.approvals),
    };
    match pr.unresolved_threads {
        0 => approvals,
        threads => format!("{threads} unresolved · {approvals}"),
    }
}

/// Formats an age in seconds as `45m`, `3h` or `2d`.
pub fn format_age(seconds: u64) -> String {
    match seconds / 60 {
        minutes @ 0..60 => format!("{minutes}m"),
        minutes @ 60..1_440 => format!("{}h", minutes / 60),
        minutes => format!("{}d", minutes / 1_440),
    }
}

fn row(pr: &PullRequest, colored: &[Part]) -> Row {
    let (icon, state_tone) = pr.state().style();
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
                label: Some("open in the browser".to_string()),
                ..Action::new("enter", &["gh", "pr", "view", "--web", &pr.url])
            },
            Action {
                label: Some("copy the URL".to_string()),
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
}

impl Node {
    fn into_pull_request(self) -> PullRequest {
        let checks = self
            .commits
            .nodes
            .into_iter()
            .next()
            .and_then(|node| node.commit.status_check_rollup)
            .map(|rollup| rollup.state);
        PullRequest {
            repository: self.repository.name,
            full_name: self.repository.name_with_owner,
            number: self.number,
            title: self.title,
            url: self.url,
            draft: self.is_draft,
            additions: self.additions,
            deletions: self.deletions,
            review: self.review_decision,
            checks,
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
            approvals: self.latest_opinionated_reviews.map_or(0, |reviews| {
                reviews
                    .nodes
                    .iter()
                    .filter(|review| review.state == "APPROVED")
                    .count()
            }),
            required_approvals: self
                .base_ref
                .and_then(|base| base.branch_protection_rule)
                .and_then(|rule| rule.required_approving_review_count),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}
