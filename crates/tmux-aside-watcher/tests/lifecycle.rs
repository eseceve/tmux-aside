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
    Answer, Answers, Followed, LivePane, Stage, Target, Worktree, activity, ask_in_order,
    batch_query, choose_pull_request, deployed, due_for_qa, evaluate, github_repository, group_key,
    group_qa, is_idle, leaves, parse_batch, pending_nudge, qa, ready_to_deploy, ready_to_merge,
    rows, should_clean_up,
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

/// `launch` with `pull_request`, as the default Ready to merge Criterion answers it.
fn following(launch: Launch, pull_request: Option<PullRequest>) -> Followed {
    let answers = pull_request
        .iter()
        .filter(|pr| !pr.closed)
        .flat_map(default_answers)
        .collect();
    Followed {
        launch,
        pull_request,
        worktree: None,
        answers,
    }
}

/// What the default Criteria answer for `pull_request`, up to the first stage not reached.
fn default_answers(pull_request: &PullRequest) -> Answers {
    let language = Language::En;
    let stages = [
        Stage::ReadyToMerge,
        Stage::Qa,
        Stage::ReadyToDeploy,
        Stage::Deployed,
    ];
    ask_in_order(&stages, pull_request, |stage| {
        Ok(match stage {
            Stage::ReadyToMerge => ready_to_merge(pull_request, language),
            Stage::Qa => qa(pull_request, language),
            Stage::ReadyToDeploy => {
                ready_to_deploy(pull_request, "qa:passed", "qa:failed", language)
            }
            _ => deployed(language),
        })
    })
}

fn followed(pull_request: Option<PullRequest>) -> Followed {
    following(launch("feat/x", "x"), pull_request)
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
        .map(|(name, value)| (name.to_string(), value.to_string(), false))
    );
}

#[test]
fn a_known_pull_request_is_asked_for_by_number() {
    let target = Target::pull_request("https://github.com/org/shop/pull/12/files").unwrap();

    let (query, variables) = batch_query(&[target]);

    assert!(
        query.contains("p0: repository(owner: $o0, name: $n0) { pullRequest(number: $m0)"),
        "{query}"
    );
    assert!(query.contains("$m0: Int!"), "{query}");
    assert_eq!(variables[2], ("m0".to_string(), "12".to_string(), true));
    assert_eq!(Target::pull_request("https://example.com/x"), None);
}

#[test]
fn the_answer_for_a_known_pull_request_is_that_pull_request() {
    let output = json!({"data": {"p0": {"pullRequest": node(json!({"number": 12}))}}}).to_string();

    let found = parse_batch(&output, 1).unwrap();

    assert_eq!(found[0].len(), 1);
    assert_eq!(found[0][0].number, 12);
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
    let followed = [following(
        launch("feat/dark", "Add a dark theme"),
        Some(pull_request(json!({}))),
    )];

    let rows = rows(&followed, "tmux-aside", Language::En);

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
    let followed = [following(
        launch("feat/new", "https://example.com/issues/7"),
        None,
    )];

    let rows = rows(&followed, "tmux-aside", Language::En);

    assert_eq!(rows[0].text, "https://example.com/issues/7");
    assert_eq!(rows[0].detail, None);
    assert_eq!(rows[0].lines, ["Implementing · no pull request yet"]);
    assert!(rows[0].action("p").is_none());
}

#[test]
fn stages_and_summaries_are_translated() {
    let followed = [following(launch("feat/new", "x"), None)];

    let rows = rows(&followed, "tmux-aside", Language::Es);

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

    assert!(
        nudge.contains("https://github.com/org/shop/pull/12"),
        "{nudge}"
    );
    assert!(nudge.contains("has conflicts"), "{nudge}");
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

    assert!(nudge.contains("2 unresolved review comments"), "{nudge}");
}

#[test]
fn conflicts_are_asked_for_before_comments() {
    let pr = pull_request(json!({"mergeable": "CONFLICTING", "reviewThreads": threads(&[false])}));

    let nudge = evaluate(&followed(Some(pr)), Language::En).nudge.unwrap();

    assert!(nudge.contains("has conflicts"), "{nudge}");
}

#[test]
fn a_pull_request_with_nothing_to_do_asks_for_no_nudge() {
    let pr = pull_request(json!({"reviewThreads": threads(&[true])}));

    assert_eq!(evaluate(&followed(Some(pr)), Language::En).nudge, None);
}

#[test]
fn a_nudge_is_pending_once_per_head_commit() {
    let mut followed = followed(Some(pull_request(json!({"mergeable": "CONFLICTING"}))));
    let (key, _) = pending_nudge(&followed, Language::En).unwrap();
    followed.launch.nudges.push(key);

    assert!(pending_nudge(&followed, Language::En).is_none());

    let again = following(
        followed.launch.clone(),
        Some(pull_request(
            json!({"mergeable": "CONFLICTING", "headRefOid": "bbb"}),
        )),
    );
    assert!(pending_nudge(&again, Language::En).is_some());
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
fn a_merged_pull_request_enters_qa() {
    let evaluation = evaluate(&finished("MERGED", Some(Worktree::Missing)), Language::En);

    assert_eq!(evaluation.stage, Stage::Qa);
    assert_eq!(evaluation.summary, "waiting for QA");
    assert_eq!(evaluation.tone, Tone::Accent);
}

fn merged_with(labels: &[&str], worktree: Worktree) -> Followed {
    let nodes: Vec<Value> = labels.iter().map(|name| json!({"name": name})).collect();
    let pr = pull_request(json!({"state": "MERGED", "labels": {"nodes": nodes}}));
    Followed {
        worktree: Some(worktree),
        ..followed(Some(pr))
    }
}

#[test]
fn the_passed_label_makes_it_deployed_by_default_so_it_leaves_once_cleaned_up() {
    let removed = merged_with(&["qa:passed"], Worktree::Missing);
    let kept = merged_with(&["qa:passed"], Worktree::Uncommitted);

    assert_eq!(evaluate(&removed, Language::En).stage, Stage::Deployed);
    assert!(leaves(&removed, Language::En));
    assert!(!leaves(&kept, Language::En));
}

#[test]
fn the_failed_label_keeps_it_in_qa_with_a_failure() {
    let evaluation = evaluate(
        &merged_with(&["qa:failed"], Worktree::Missing),
        Language::En,
    );

    assert_eq!(evaluation.stage, Stage::Qa);
    assert_eq!(evaluation.summary, "QA failed");
    assert_eq!(evaluation.tone, Tone::Danger);
}

#[test]
fn the_qa_label_names_are_criterion_arguments() {
    let pr = pull_request(json!({"state": "MERGED", "labels": {"nodes": [{"name": "qa:ok"}]}}));

    assert!(ready_to_deploy(&pr, "qa:ok", "qa:ko", Language::En).met);
    assert!(!ready_to_deploy(&pr, "qa:passed", "qa:failed", Language::En).met);
}

#[test]
fn a_replaced_deployed_criterion_keeps_it_ready_to_deploy_until_in_production() {
    let mut followed = merged_with(&["qa:passed"], Worktree::Missing);
    followed.answers.pop();
    followed.answers.push((
        Stage::Deployed,
        Ok(Answer {
            met: false,
            summary: Some("waiting for the release".to_string()),
            deployed: Some(false),
            ..Answer::default()
        }),
    ));

    let evaluation = evaluate(&followed, Language::En);

    assert_eq!(evaluation.stage, Stage::ReadyToDeploy);
    assert_eq!(evaluation.summary, "waiting for the release");
    assert!(!leaves(&followed, Language::En));
}

#[test]
fn a_closed_pull_request_leaves_once_its_worktree_is_removed() {
    assert!(leaves(
        &finished("CLOSED", Some(Worktree::Missing)),
        Language::En
    ));
    assert!(!leaves(
        &finished("CLOSED", Some(Worktree::Unpushed)),
        Language::En
    ));
}

#[test]
fn a_row_in_qa_launches_a_qa_agent_with_its_key() {
    let followed = [finished("MERGED", Some(Worktree::Missing))];

    let rows = rows(&followed, "/bin/tmux-aside", Language::En);

    assert_eq!(
        rows[0].action("q").unwrap(),
        [
            "/bin/tmux-aside",
            "qa",
            "https://github.com/org/shop/pull/12"
        ]
    );
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

    assert!(pending_nudge(&followed(Some(merged)), Language::En).is_none());
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
    assert_eq!(evaluate(&followed, Language::En).stage, Stage::Qa);
}

fn answered(answers: Vec<(Stage, Result<Answer, String>)>) -> Followed {
    Followed {
        answers,
        ..followed(Some(pull_request(json!({}))))
    }
}

#[test]
fn an_unmet_criterion_gives_its_summary_tone_and_nudge() {
    let evaluation = evaluate(
        &answered(vec![(
            Stage::ReadyToMerge,
            Ok(Answer {
                met: false,
                summary: Some("needs a review from the web team".to_string()),
                tone: Some(Tone::Warning),
                nudge: Some("Ask the web team for a review.".to_string()),
                deployed: None,
            }),
        )]),
        Language::En,
    );

    assert_eq!(evaluation.stage, Stage::Implementing);
    assert_eq!(evaluation.summary, "needs a review from the web team");
    assert_eq!(evaluation.tone, Tone::Warning);
    assert_eq!(
        evaluation.nudge.as_deref(),
        Some("Ask the web team for a review.")
    );
}

#[test]
fn a_met_criterion_moves_the_pull_request_to_its_stage() {
    let evaluation = evaluate(
        &answered(vec![(
            Stage::ReadyToMerge,
            Ok(Answer {
                met: true,
                ..Answer::default()
            }),
        )]),
        Language::En,
    );

    assert_eq!(evaluation.stage, Stage::ReadyToMerge);
    assert_eq!(evaluation.tone, Tone::Success);
}

#[test]
fn a_failed_criterion_shows_its_error() {
    let evaluation = evaluate(
        &answered(vec![(
            Stage::ReadyToMerge,
            Err("timed out after 2s".to_string()),
        )]),
        Language::En,
    );

    assert_eq!(evaluation.stage, Stage::Implementing);
    assert_eq!(evaluation.summary, "criterion failed: timed out after 2s");
    assert_eq!(evaluation.tone, Tone::Danger);
    assert_eq!(evaluation.nudge, None);
}

#[test]
fn an_answer_ignores_unknown_fields_and_needs_only_met() {
    let answer: Answer = serde_json::from_str(r#"{"met": false, "since": "v2"}"#).unwrap();

    assert_eq!(answer, Answer::default());
}

#[test]
fn a_merged_pull_request_is_ready_to_merge_whatever_its_criterion_answers() {
    let merged = pull_request(json!({"state": "MERGED"}));
    let unmet = Answer::default();

    let answers = ask_in_order(
        &[Stage::ReadyToMerge, Stage::Qa],
        &merged,
        |stage| match stage {
            Stage::ReadyToMerge => Ok(unmet.clone()),
            _ => Ok(qa(&merged, Language::En)),
        },
    );

    assert_eq!(answers.len(), 2);
}

#[test]
fn a_failed_qa_stays_red_even_when_the_worktree_is_kept() {
    let evaluation = evaluate(
        &merged_with(&["qa:failed"], Worktree::Uncommitted),
        Language::En,
    );

    assert_eq!(evaluation.summary, "QA failed");
    assert_eq!(evaluation.tone, Tone::Danger);
}

fn member(branch: &str, group: Option<&str>, repository: &str, pr: Option<Value>) -> Followed {
    let launch = Launch {
        group: group.map(str::to_string),
        ..support::launch(&Path::new("/src").join(repository), branch)
    };
    following(launch, pr.map(|pr| parse_pull_request(&pr).unwrap()))
}

fn merged_pr(number: u64) -> Value {
    node(json!({
        "state": "MERGED",
        "number": number,
        "url": format!("https://github.com/org/shop/pull/{number}"),
    }))
}

#[test]
fn rows_of_the_same_group_are_shown_together_under_its_name() {
    let followed = [
        member("feat/a", Some("theme"), "shop", None),
        member("feat/b", None, "api", None),
        member("feat/c", Some("theme"), "web", None),
    ];

    let rows = rows(&followed, "tmux-aside", Language::En);

    let groups: Vec<&str> = rows
        .iter()
        .map(|row| row.group.as_deref().unwrap())
        .collect();
    assert_eq!(groups, ["api", "theme", "theme"]);
}

#[test]
fn qa_waits_until_every_pull_request_of_the_group_reached_qa() {
    let followed = [
        member("feat/a", Some("theme"), "shop", Some(merged_pr(1))),
        member(
            "feat/b",
            Some("theme"),
            "shop",
            Some(node(json!({"number": 2}))),
        ),
        member("feat/c", Some("theme"), "shop", None),
    ];

    let qa = group_qa(&followed, &group_key(&followed[0].launch), Language::En);
    let rows = rows(&followed, "tmux-aside", Language::En);

    assert!(!qa.ready);
    assert_eq!(qa.waiting_for, ["#2", "feat/c"]);
    let waiting = rows
        .iter()
        .find(|row| row.detail.as_deref() == Some("#1"))
        .unwrap();
    assert_eq!(waiting.lines, ["QA · waiting for #2, feat/c"]);
    assert!(waiting.action("q").is_none());
}

#[test]
fn the_qa_key_launches_qa_for_the_whole_ready_group() {
    let followed = [
        member("feat/a", Some("theme"), "shop", Some(merged_pr(1))),
        member("feat/b", Some("theme"), "shop", Some(merged_pr(2))),
        member("feat/c", None, "shop", Some(merged_pr(3))),
    ];

    let rows = rows(&followed, "tmux-aside", Language::En);

    let row = rows
        .iter()
        .find(|row| row.detail.as_deref() == Some("#1"))
        .unwrap();
    assert_eq!(
        row.action("q").unwrap(),
        [
            "tmux-aside",
            "qa",
            "https://github.com/org/shop/pull/1",
            "https://github.com/org/shop/pull/2",
        ]
    );
}

#[test]
fn qa_is_due_once_per_set_of_pull_requests_of_a_ready_group() {
    let mut followed = vec![
        member("feat/a", Some("theme"), "shop", Some(merged_pr(1))),
        member("feat/b", Some("theme"), "shop", Some(merged_pr(2))),
    ];

    let due = due_for_qa(&followed, Language::En);
    assert_eq!(due.len(), 1);
    let launched = due[0].0.join(" ");
    for member in &mut followed {
        member.launch.auto_qa = Some(launched.clone());
    }
    assert!(due_for_qa(&followed, Language::En).is_empty());

    let mut fix = member("fix/a", Some("theme"), "shop", Some(merged_pr(3)));
    fix.launch.auto_qa = None;
    followed.push(fix);
    assert_eq!(due_for_qa(&followed, Language::En)[0].0.len(), 3);
}

#[test]
fn qa_is_not_due_again_when_a_member_leaves_the_group() {
    let mut followed = vec![
        member("feat/a", Some("theme"), "shop", Some(merged_pr(1))),
        member("feat/b", Some("theme"), "shop", Some(merged_pr(2))),
    ];
    let launched = due_for_qa(&followed, Language::En)[0].0.join(" ");
    for member in &mut followed {
        member.launch.auto_qa = Some(launched.clone());
    }

    followed.remove(0);

    assert!(due_for_qa(&followed, Language::En).is_empty());
}

fn failed_pr(number: u64) -> Value {
    node(json!({
        "state": "MERGED",
        "number": number,
        "url": format!("https://github.com/org/shop/pull/{number}"),
        "labels": {"nodes": [{"name": "qa:failed"}]},
    }))
}

#[test]
fn a_failed_qa_shows_in_red_and_offers_a_fix() {
    let followed = [member("feat/a", None, "shop", Some(failed_pr(1)))];

    let rows = rows(&followed, "tmux-aside", Language::En);

    assert_eq!(rows[0].lines, ["QA · QA failed"]);
    assert_eq!(rows[0].bar_tone, Tone::Danger);
    assert_eq!(
        rows[0].action("f").unwrap(),
        ["tmux-aside", "fix", "https://github.com/org/shop/pull/1"]
    );
}

#[test]
fn qa_is_offered_again_only_once_the_fix_reached_qa_too() {
    let mut followed = vec![
        member("feat/a", Some("theme"), "shop", Some(failed_pr(1))),
        member(
            "fix/qa-1",
            Some("theme"),
            "shop",
            Some(node(json!({"number": 2}))),
        ),
    ];

    let rows_before = rows(&followed, "tmux-aside", Language::En);
    let failed = rows_before
        .iter()
        .find(|row| row.detail.as_deref() == Some("#1"))
        .unwrap();
    assert_eq!(failed.lines, ["QA · QA failed · waiting for #2"]);
    assert!(failed.action("q").is_none());

    followed[1] = member("fix/qa-1", Some("theme"), "shop", Some(merged_pr(2)));
    let rows_after = rows(&followed, "tmux-aside", Language::En);
    let failed = rows_after
        .iter()
        .find(|row| row.detail.as_deref() == Some("#1"))
        .unwrap();
    assert_eq!(
        failed.action("q").unwrap(),
        [
            "tmux-aside",
            "qa",
            "https://github.com/org/shop/pull/1",
            "https://github.com/org/shop/pull/2",
        ]
    );
}

#[test]
fn a_failing_criterion_in_qa_offers_no_fix() {
    let mut followed = member("feat/a", None, "shop", Some(merged_pr(1)));
    followed.answers.pop();
    followed.answers.push((
        Stage::ReadyToDeploy,
        Err("exited with status 1".to_string()),
    ));

    let rows = rows(&[followed], "tmux-aside", Language::En);

    assert_eq!(rows[0].bar_tone, Tone::Danger);
    assert!(rows[0].action("f").is_none());
}

#[test]
fn codex_and_antigravity_agents_are_nudged_like_claude_code_when_done() {
    for command in ["codex", "agy"] {
        let mut state = agent(Some(Status::Done));
        state.command = command.to_string();

        assert!(is_idle(&state, &pane(40, command, false)), "{command}");
    }
}

#[test]
fn a_criterion_learns_whether_the_agent_is_idle_or_busy_or_gone() {
    let live = pane(40, "claude", true);

    assert_eq!(activity(&agent(Some(Status::Done)), &live), Some("idle"));
    assert_eq!(activity(&agent(None), &live), Some("idle"));
    assert_eq!(activity(&agent(Some(Status::Working)), &live), Some("busy"));
    assert_eq!(activity(&agent(Some(Status::Waiting)), &live), Some("busy"));
    assert_eq!(
        activity(&agent(Some(Status::Done)), &pane(40, "zsh", false)),
        None
    );
}

#[test]
fn a_launch_without_an_agent_has_no_jump_and_no_window() {
    let launch = Launch {
        pane: String::new(),
        window: String::new(),
        ..launch("feat/x", "x")
    };

    let rows = rows(
        &[following(launch, Some(pull_request(json!({}))))],
        "tmux-aside",
        Language::En,
    );

    assert!(rows[0].action("enter").is_none());
    assert_eq!(rows[0].window, None);
    assert!(rows[0].action("p").is_some());
}
