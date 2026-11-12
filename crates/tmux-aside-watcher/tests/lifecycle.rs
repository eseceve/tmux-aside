mod support;

use std::path::Path;

use serde_json::{Value, json};
use support::node;
use tmux_aside::i18n::Language;
use tmux_aside::launch::Launch;
use tmux_aside::protocol::Tone;
use tmux_aside_agents::{AgentState, Status};
use tmux_aside_github::{PullRequest, parse_pull_request};
use tmux_aside_watcher::{
    Followed, LivePane, NudgeKind, Stage, Target, Worktree, batch_query, choose_pull_request,
    evaluate, github_repository, is_idle, parse_batch, pending_nudge, rows, should_clean_up,
};

fn pull_request(changes: Value) -> PullRequest {
    parse_pull_request(&node(changes)).unwrap()
}

fn launch(branch: &str, task: &str) -> Launch {
    Launch {
        window: "@4".to_string(),
        pane: "%7".to_string(),
        task: task.to_string(),
        ..support::launch(Path::new("/src/shop"), branch)
    }
}

fn followed(pull_request: Option<PullRequest>) -> Followed {
    Followed {
        launch: launch("feat/x", "x"),
        pull_request,
        worktree: None,
    }
}

#[test]
fn the_repository_is_read_from_every_form_of_github_remote() {
    for url in [
        "git@github.com:org/shop.git",
        "https://github.com/org/shop.git",
        "https://github.com/org/shop",
        "ssh://git@github.com/org/shop.git",
    ] {
        assert_eq!(
            github_repository(url),
            Some(("org".to_string(), "shop".to_string())),
            "{url}"
        );
    }
    assert_eq!(github_repository("git@gitlab.com:org/shop.git"), None);
}

#[test]
fn one_query_asks_for_the_newest_pull_request_of_every_branch() {
    let targets = [
        Target::new("org", "shop", "feat/a"),
        Target::new("org", "api", "fix/b"),
    ];

    let (query, variables) = batch_query(&targets);

    assert!(
        query.contains("p0: repository(owner: $o0, name: $n0)"),
        "{query}"
    );
    assert!(
        query.contains("p1: repository(owner: $o1, name: $n1)"),
        "{query}"
    );
    assert!(
        query.contains("pullRequests(headRefName: $b1, first: 5, orderBy"),
        "{query}"
    );
    assert!(query.contains("fragment PullRequestFields"), "{query}");
    assert_eq!(
        variables,
        [
            ("o0", "org"),
            ("n0", "shop"),
            ("b0", "feat/a"),
            ("o1", "org"),
            ("n1", "api"),
            ("b1", "fix/b"),
        ]
        .map(|(name, value)| (name.to_string(), value.to_string()))
    );
}

#[test]
fn the_batched_answer_gives_each_branch_its_pull_requests() {
    let output = json!({"data": {
        "p0": {"pullRequests": {"nodes": [node(json!({})), node(json!({"number": 9}))]}},
        "p1": {"pullRequests": {"nodes": []}},
        "p2": null
    }})
    .to_string();

    let found = parse_batch(&output, 3).unwrap();

    let numbers: Vec<u64> = found[0].iter().map(|pr| pr.number).collect();
    assert_eq!(numbers, [12, 9]);
    assert!(found[1].is_empty());
    assert!(found[2].is_empty());
}

fn numbered(number: u64, changes: Value) -> PullRequest {
    let mut changes = changes;
    changes["number"] = json!(number);
    changes["url"] = json!(format!("https://github.com/org/shop/pull/{number}"));
    pull_request(changes)
}

fn chosen(
    candidates: Vec<PullRequest>,
    recorded: Option<&str>,
    local_head: Option<&str>,
) -> Option<u64> {
    choose_pull_request(candidates, recorded, local_head).map(|pr| pr.number)
}

#[test]
fn an_open_pull_request_of_the_branch_is_the_launchs_own() {
    let candidates = vec![
        numbered(3, json!({"state": "CLOSED"})),
        numbered(2, json!({"state": "OPEN"})),
    ];

    assert_eq!(chosen(candidates, None, None), Some(2));
}

#[test]
fn an_old_finished_pull_request_of_a_reused_branch_name_is_not_the_launchs_own() {
    let candidates = vec![numbered(1, json!({"state": "MERGED", "headRefOid": "old"}))];

    assert_eq!(chosen(candidates, None, Some("new")), None);
}

#[test]
fn a_finished_pull_request_is_the_launchs_own_when_recorded_or_at_the_worktrees_head() {
    let merged = || vec![numbered(4, json!({"state": "MERGED", "headRefOid": "abc"}))];

    assert_eq!(
        chosen(merged(), Some("https://github.com/org/shop/pull/4"), None),
        Some(4)
    );
    assert_eq!(chosen(merged(), None, Some("abc")), Some(4));
}

#[test]
fn a_pull_request_from_another_repository_is_never_the_launchs_own() {
    let candidates = vec![numbered(5, json!({"isCrossRepository": true}))];

    assert_eq!(chosen(candidates, None, None), None);
}

#[test]
fn github_errors_fail_the_fetch() {
    let output = json!({"errors": [{"message": "Bad credentials"}]}).to_string();

    let error = parse_batch(&output, 1).unwrap_err();

    assert!(error.to_string().contains("Bad credentials"));
}

#[test]
fn without_a_pull_request_the_launch_is_implementing() {
    let evaluation = evaluate(&followed(None), Language::En);

    assert_eq!(evaluation.stage, Stage::Implementing);
    assert_eq!(evaluation.summary, "no pull request yet");
}

#[test]
fn an_approved_pull_request_with_passing_checks_is_ready_to_merge() {
    let evaluation = evaluate(&followed(Some(pull_request(json!({})))), Language::En);

    assert_eq!(evaluation.stage, Stage::ReadyToMerge);
    assert_eq!(evaluation.tone, Tone::Success);
}

#[test]
fn a_pull_request_that_is_not_ready_says_what_is_missing() {
    let cases = [
        (json!({"isDraft": true}), "draft", Tone::Accent),
        (
            json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "FAILURE"}}}]}}),
            "checks failing",
            Tone::Danger,
        ),
        (
            json!({"mergeable": "CONFLICTING"}),
            "conflicts",
            Tone::Danger,
        ),
        (
            json!({"reviewDecision": "CHANGES_REQUESTED"}),
            "changes requested",
            Tone::Warning,
        ),
        (
            json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "PENDING"}}}]}}),
            "checks running",
            Tone::Accent,
        ),
        (
            json!({"reviewDecision": "REVIEW_REQUIRED"}),
            "waiting for approval",
            Tone::Accent,
        ),
    ];
    for (changes, summary, tone) in cases {
        let evaluation = evaluate(&followed(Some(pull_request(changes.clone()))), Language::En);

        assert_eq!(evaluation.stage, Stage::Implementing, "{changes}");
        assert_eq!(evaluation.summary, summary, "{changes}");
        assert_eq!(evaluation.tone, tone, "{changes}");
    }
}

#[test]
fn a_row_shows_the_stage_and_links_the_pull_request_and_the_agents_window() {
    let followed = [Followed {
        launch: launch("feat/dark", "Add a dark theme"),
        pull_request: Some(pull_request(json!({}))),
        worktree: None,
    }];

    let rows = rows(&followed, Language::En);

    let row = &rows[0];
    assert_eq!(row.text, "Add a dark theme");
    assert_eq!(row.detail.as_deref(), Some("#12"));
    assert_eq!(row.lines, ["Ready to merge · waiting to be merged"]);
    assert_eq!(row.bar_tone, Tone::Success);
    assert_eq!(row.group.as_deref(), Some("shop"));
    assert_eq!(row.window.as_deref(), Some("@4"));
    assert_eq!(row.links[0].url, "https://github.com/org/shop/pull/12");
    assert_eq!(
        row.action("enter").unwrap(),
        ["tmux", "switch-client", "-t", "%7"]
    );
    assert_eq!(
        row.action("p").unwrap(),
        [
            "gh",
            "pr",
            "view",
            "--web",
            "https://github.com/org/shop/pull/12"
        ]
    );
}

#[test]
fn a_launch_without_a_pull_request_shows_its_task_reference() {
    let followed = [Followed {
        launch: launch("feat/new", "https://example.com/issues/7"),
        pull_request: None,
        worktree: None,
    }];

    let rows = rows(&followed, Language::En);

    assert_eq!(rows[0].text, "https://example.com/issues/7");
    assert_eq!(rows[0].detail, None);
    assert_eq!(rows[0].lines, ["Implementing · no pull request yet"]);
    assert!(rows[0].action("p").is_none());
}

#[test]
fn stages_and_summaries_are_translated() {
    let followed = [Followed {
        launch: launch("feat/new", "x"),
        pull_request: None,
        worktree: None,
    }];

    let rows = rows(&followed, Language::Es);

    assert_eq!(rows[0].lines, ["Implementando · aún sin pull request"]);
}

fn threads(resolved: &[bool]) -> Value {
    let nodes: Vec<Value> = resolved
        .iter()
        .map(|resolved| json!({"isResolved": resolved}))
        .collect();
    json!({ "nodes": nodes })
}

#[test]
fn a_conflict_asks_the_agent_to_resolve_it() {
    let nudge = evaluate(
        &followed(Some(pull_request(json!({"mergeable": "CONFLICTING"})))),
        Language::En,
    )
    .nudge
    .unwrap();

    assert_eq!(nudge.kind, NudgeKind::ResolveConflicts);
    assert!(
        nudge.text.contains("https://github.com/org/shop/pull/12"),
        "{}",
        nudge.text
    );
    assert!(nudge.text.contains("conflicts"), "{}", nudge.text);
}

#[test]
fn unresolved_review_comments_ask_the_agent_to_answer_them() {
    let nudge = evaluate(
        &followed(Some(pull_request(
            json!({"reviewThreads": threads(&[false, true, false])}),
        ))),
        Language::En,
    )
    .nudge
    .unwrap();

    assert_eq!(nudge.kind, NudgeKind::AnswerComments);
    assert!(
        nudge.text.contains("2 unresolved review comments"),
        "{}",
        nudge.text
    );
}

#[test]
fn conflicts_are_asked_for_before_comments() {
    let pr = pull_request(json!({"mergeable": "CONFLICTING", "reviewThreads": threads(&[false])}));

    let nudge = evaluate(&followed(Some(pr)), Language::En).nudge.unwrap();

    assert_eq!(nudge.kind, NudgeKind::ResolveConflicts);
}

#[test]
fn a_pull_request_with_nothing_to_do_asks_for_no_nudge() {
    let pr = pull_request(json!({"reviewThreads": threads(&[true])}));

    assert_eq!(evaluate(&followed(Some(pr)), Language::En).nudge, None);
}

#[test]
fn a_nudge_is_pending_once_per_head_commit() {
    let mut followed = followed(Some(pull_request(json!({"mergeable": "CONFLICTING"}))));
    let (key, _) = pending_nudge(&followed).unwrap();
    followed.launch.nudges.push(key);

    assert!(pending_nudge(&followed).is_none());

    followed.pull_request = Some(pull_request(
        json!({"mergeable": "CONFLICTING", "headRefOid": "bbb"}),
    ));
    assert!(pending_nudge(&followed).is_some());
}

fn agent(status: Option<Status>) -> AgentState {
    serde_json::from_value(json!({
        "pane_id": "%7",
        "pane_pid": 40,
        "command": "claude",
        "project": "shop",
        "window": "agent",
        "status": status,
        "status_ts": 0,
        "activity_ts": 0
    }))
    .unwrap()
}

fn pane(pid: u32, command: &str, viewed: bool) -> LivePane {
    LivePane {
        pid,
        command: command.to_string(),
        viewed,
    }
}

#[test]
fn an_agent_is_idle_when_neither_working_nor_waiting_and_still_in_its_unviewed_pane() {
    let unviewed = pane(40, "claude", false);
    assert!(is_idle(&agent(Some(Status::Done)), &unviewed));
    assert!(
        is_idle(&agent(None), &unviewed),
        "a done agent someone already looked at"
    );
    assert!(!is_idle(&agent(Some(Status::Working)), &unviewed));
    assert!(!is_idle(&agent(Some(Status::Waiting)), &unviewed));
    assert!(
        !is_idle(&agent(Some(Status::Done)), &pane(40, "zsh", false)),
        "the agent exited"
    );
    assert!(
        !is_idle(&agent(Some(Status::Done)), &pane(41, "claude", false)),
        "another pane"
    );
    assert!(
        !is_idle(&agent(Some(Status::Done)), &pane(40, "claude", true)),
        "someone is looking"
    );
}

fn finished(state: &str, worktree: Option<Worktree>) -> Followed {
    Followed {
        worktree,
        ..followed(Some(pull_request(json!({"state": state}))))
    }
}

#[test]
fn a_merged_or_closed_pull_request_with_a_clean_worktree_is_cleaned_up() {
    assert!(should_clean_up(&finished("MERGED", Some(Worktree::Clean))));
    assert!(should_clean_up(&finished("CLOSED", Some(Worktree::Clean))));
}

#[test]
fn a_worktree_with_work_that_would_be_lost_is_kept() {
    for kept in [Worktree::Uncommitted, Worktree::Unpushed] {
        assert!(
            !should_clean_up(&finished("MERGED", Some(kept))),
            "{kept:?}"
        );
    }
}

#[test]
fn a_worktree_already_removed_is_not_cleaned_up_again() {
    assert!(!should_clean_up(&finished(
        "MERGED",
        Some(Worktree::Missing)
    )));
}

#[test]
fn nothing_is_cleaned_up_while_the_pull_request_is_open() {
    assert!(!should_clean_up(&finished("OPEN", Some(Worktree::Clean))));
    assert!(!should_clean_up(&followed(None)));
}

#[test]
fn a_merged_pull_request_says_its_worktree_was_removed() {
    let evaluation = evaluate(&finished("MERGED", Some(Worktree::Missing)), Language::En);

    assert_eq!(evaluation.stage, Stage::Merged);
    assert_eq!(evaluation.summary, "worktree removed");
    assert_eq!(evaluation.tone, Tone::Success);
}

#[test]
fn a_kept_worktree_warns_what_it_keeps() {
    let uncommitted = evaluate(
        &finished("CLOSED", Some(Worktree::Uncommitted)),
        Language::En,
    );
    let unpushed = evaluate(&finished("MERGED", Some(Worktree::Unpushed)), Language::En);

    assert_eq!(uncommitted.stage, Stage::Closed);
    assert_eq!(
        uncommitted.summary,
        "uncommitted changes kept in the worktree"
    );
    assert_eq!(uncommitted.tone, Tone::Warning);
    assert_eq!(unpushed.summary, "unpushed commits kept in the worktree");
    assert_eq!(unpushed.tone, Tone::Warning);
}

#[test]
fn a_merged_pull_request_asks_for_no_nudge() {
    let merged = pull_request(json!({"state": "MERGED", "mergeable": "CONFLICTING"}));

    assert!(pending_nudge(&followed(Some(merged))).is_none());
}

#[test]
fn a_worktree_that_could_not_be_removed_warns() {
    let evaluation = evaluate(&finished("MERGED", Some(Worktree::Clean)), Language::En);

    assert_eq!(evaluation.summary, "the worktree could not be removed");
    assert_eq!(evaluation.tone, Tone::Warning);
}

#[test]
fn the_clones_own_working_tree_is_never_cleaned_up() {
    let followed = finished("MERGED", Some(Worktree::Main));

    assert!(!should_clean_up(&followed));
    assert_eq!(
        evaluate(&followed, Language::En).summary,
        "the clone's own working tree is kept"
    );
}
