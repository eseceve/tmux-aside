use std::collections::HashMap;
use tmux_aside::i18n::Language;
use tmux_aside::protocol::{Row, Tone};

use serde_json::json;
use tmux_aside_agents::{
    AgentState, Branch, Checks, Diff, Grouping, LivePane, PullRequest, RowOptions, Sort, Status,
    checks_from_rollup, entered, format_elapsed, infer_status, messages, next_agent, notification,
    parse_shortstat, pull_requests_by_branch, reconcile, rows, waits_for_background_agents,
};

fn state(
    pane: &str,
    project: &str,
    window: &str,
    status: Option<Status>,
    status_ts: u64,
) -> AgentState {
    AgentState {
        pane_id: pane.to_string(),
        pane_pid: 100,
        command: "claude".to_string(),
        project: project.to_string(),
        window: window.to_string(),
        status,
        status_ts,
        activity_ts: status_ts,
        title: String::new(),
        workdir: String::new(),
        window_id: String::new(),
        window_index: 0,
        diff: None,
        stalled: false,
        inferred: false,
        background: false,
    }
}

fn flat() -> RowOptions {
    RowOptions {
        group: Grouping::None,
        ..options(Sort::Priority, false)
    }
}

fn options(sort: Sort, elapsed: bool) -> RowOptions {
    RowOptions {
        sort,
        elapsed,
        group: Grouping::Project,
        language: Language::En,
    }
}

fn live(pane: &str) -> LivePane {
    LivePane {
        pane_id: pane.to_string(),
        pane_pid: 100,
        command: "claude".to_string(),
        title: String::new(),
        path: String::new(),
        window_id: "@1".to_string(),
        window_name: String::new(),
        window_index: 0,
        viewed: false,
    }
}

#[test]
fn reconcile_takes_the_working_directory_from_the_live_pane() {
    let live_pane = LivePane {
        path: "/src/api".to_string(),
        ..live("%1")
    };

    let result = reconcile(vec![state("%1", "api", "w", None, 0)], &[live_pane]);

    assert_eq!(result.agents[0].workdir, "/src/api");
}

#[test]
fn keeps_agents_whose_pane_is_alive_with_the_same_process() {
    let result = reconcile(
        vec![state("%1", "api", "w", Some(Status::Working), 0)],
        &[live("%1")],
    );

    assert_eq!(result.agents.len(), 1);
    assert!(result.dropped.is_empty());
}

#[test]
fn drops_agents_whose_pane_is_gone_recycled_or_no_longer_runs_the_agent() {
    let recycled = LivePane {
        pane_pid: 999,
        ..live("%2")
    };
    let finished = LivePane {
        command: "zsh".to_string(),
        ..live("%3")
    };

    let result = reconcile(
        vec![
            state("%1", "api", "w", Some(Status::Working), 0),
            state("%2", "api", "w", Some(Status::Working), 0),
            state("%3", "api", "w", Some(Status::Working), 0),
        ],
        &[recycled, finished],
    );

    assert!(result.agents.is_empty());
    assert_eq!(result.dropped, ["%1", "%2", "%3"]);
}

#[test]
fn viewing_a_waiting_or_done_agent_acknowledges_it() {
    let viewed = |pane: &str| LivePane {
        viewed: true,
        ..live(pane)
    };

    let result = reconcile(
        vec![
            state("%1", "api", "w", Some(Status::Waiting), 0),
            state("%2", "api", "w", Some(Status::Done), 0),
            state("%3", "api", "w", Some(Status::Working), 0),
        ],
        &[viewed("%1"), viewed("%2"), viewed("%3")],
    );

    let statuses: Vec<Option<Status>> = result.agents.iter().map(|agent| agent.status).collect();
    assert_eq!(statuses, [None, None, Some(Status::Working)]);
    assert_eq!(result.acknowledged, ["%1", "%2"]);
}

#[test]
fn rows_show_icon_window_and_elapsed_time_grouped_by_project_with_a_jump_action() {
    let agents = vec![
        state("%7", "api", "review", Some(Status::Waiting), 1_000),
        state("%8", "api", "deploy", Some(Status::Working), 1_000),
    ];

    let rows = rows(
        &agents,
        1_000 + 134,
        options(Sort::Priority, true),
        &HashMap::new(),
    );

    let row = &rows[0];
    assert_eq!(row.text, "review");
    assert_eq!(row.group.as_deref(), Some("api"));
    assert_eq!(row.bar_tone, Tone::Warning);
    assert_eq!(row.detail.as_deref(), Some("2:14"));
    assert_eq!(row.detail_tone, Tone::Muted);
    assert_eq!(
        row.action("enter"),
        Some(
            &[
                "tmux".to_string(),
                "switch-client".to_string(),
                "-t".to_string(),
                "%7".to_string()
            ][..]
        )
    );
}

#[test]
fn the_bar_color_shows_the_status_without_an_icon() {
    let bar = |status| {
        let row: Row = rows(
            &[state("%1", "api", "w", status, 0)],
            0,
            options(Sort::Priority, true),
            &HashMap::new(),
        )
        .remove(0);
        assert_eq!(row.icon, None);
        row.bar_tone
    };

    assert_eq!(bar(Some(Status::Working)), Tone::Accent);
    assert_eq!(bar(Some(Status::Waiting)), Tone::Warning);
    assert_eq!(bar(Some(Status::Done)), Tone::Success);
    assert_eq!(bar(None), Tone::Muted);
}

#[test]
fn priority_sort_groups_projects_alphabetically_and_puts_waiting_first_within_each() {
    let agents = vec![
        state("%1", "web", "build", Some(Status::Working), 0),
        state("%2", "api", "tests", Some(Status::Done), 0),
        state("%3", "api", "deploy", Some(Status::Waiting), 0),
        state("%4", "api", "idle", None, 0),
        state("%5", "api", "refactor", Some(Status::Working), 0),
    ];

    let order: Vec<String> = rows(&agents, 0, options(Sort::Priority, true), &HashMap::new())
        .into_iter()
        .map(|row| row.text)
        .collect();

    assert_eq!(order, ["deploy", "tests", "refactor", "idle", "build"]);
}

#[test]
fn alphabetical_sort_orders_windows_within_each_project() {
    let agents = vec![
        state("%1", "api", "tests", Some(Status::Waiting), 0),
        state("%2", "api", "deploy", Some(Status::Working), 0),
        state("%3", "web", "build", Some(Status::Done), 0),
    ];

    let order: Vec<String> = rows(
        &agents,
        0,
        options(Sort::Alphabetical, true),
        &HashMap::new(),
    )
    .into_iter()
    .map(|row| row.text)
    .collect();

    assert_eq!(order, ["deploy", "tests", "build"]);
}

#[test]
fn window_sort_follows_the_tmux_window_order_across_projects() {
    let at = |pane: &str, project: &str, window: &str, index: u32| AgentState {
        window_index: index,
        ..state(pane, project, window, Some(Status::Waiting), 0)
    };
    let agents = vec![
        at("%1", "api", "alpha", 3),
        at("%2", "web", "beta", 1),
        at("%3", "api", "gamma", 2),
    ];
    let flat_by_window = RowOptions {
        sort: Sort::Window,
        ..flat()
    };

    let order: Vec<String> = rows(&agents, 0, flat_by_window, &HashMap::new())
        .into_iter()
        .map(|row| row.text)
        .collect();

    assert_eq!(order, ["beta", "gamma", "alpha"]);
}

#[test]
fn reconcile_takes_the_window_index_from_the_live_pane() {
    let moved = LivePane {
        window_index: 7,
        ..live("%1")
    };

    let result = reconcile(vec![state("%1", "api", "w", None, 0)], &[moved]);

    assert_eq!(result.agents[0].window_index, 7);
}

#[test]
fn elapsed_time_uses_minutes_and_seconds_then_hours_then_days() {
    assert_eq!(format_elapsed(5), "0:05");
    assert_eq!(format_elapsed(134), "2:14");
    assert_eq!(format_elapsed(3_599), "59:59");
    assert_eq!(format_elapsed(3_600), "1h");
    assert_eq!(format_elapsed(86_399), "23h");
    assert_eq!(format_elapsed(86_400), "1d");
}

#[test]
fn rows_show_the_window_then_project_with_branch_and_the_pane_title() {
    let titled = LivePane {
        title: "✳ Estado de tareas y PRs".to_string(),
        window_id: "@4".to_string(),
        ..live("%1")
    };

    let result = reconcile(vec![state("%1", "api", "review", None, 0)], &[titled]);
    let rows = rows(&result.agents, 0, flat(), &branch("main", None));

    assert_eq!(rows[0].text, "review");
    assert_eq!(rows[0].lines, ["api · main", "CC: Estado de tareas y PRs"]);
    assert_eq!(rows[0].window.as_deref(), Some("@4"));
}

#[test]
fn rows_without_branch_or_title_show_the_project_and_the_agent() {
    let rows = rows(
        &[state("%1", "api", "review", None, 0)],
        0,
        flat(),
        &HashMap::new(),
    );

    assert_eq!(rows[0].lines, ["api", "CC"]);
}

#[test]
fn every_agent_has_a_bar_colored_by_its_status() {
    let bar = |status| {
        rows(
            &[state("%1", "api", "w", status, 0)],
            0,
            options(Sort::Priority, false),
            &HashMap::new(),
        )
        .remove(0)
        .bar_tone
    };

    assert_eq!(bar(Some(Status::Working)), Tone::Accent);
    assert_eq!(bar(Some(Status::Waiting)), Tone::Warning);
    assert_eq!(bar(Some(Status::Done)), Tone::Success);
    assert_eq!(bar(None), Tone::Muted);
}

#[test]
fn elapsed_time_can_be_hidden() {
    let rows = rows(
        &[state("%1", "api", "review", Some(Status::Done), 0)],
        90,
        options(Sort::Priority, false),
        &HashMap::new(),
    );

    assert_eq!(rows[0].detail, None);
}

fn branch(name: &str, pr: Option<PullRequest>) -> HashMap<String, Branch> {
    HashMap::from([(
        "%1".to_string(),
        Branch {
            name: name.to_string(),
            pr,
        },
    )])
}

fn pr(number: u64, checks: Checks) -> HashMap<String, Branch> {
    branch("feature", Some(PullRequest { number, checks }))
}

#[test]
fn rows_show_the_pull_request_number_colored_by_its_checks() {
    let agent = [state("%1", "api", "review", Some(Status::Done), 0)];
    let detail = |checks| {
        let row = rows(&agent, 0, options(Sort::Priority, false), &pr(348, checks)).remove(0);
        (row.detail.unwrap(), row.detail_tone)
    };

    assert_eq!(
        detail(Checks::Passing),
        ("#348 ✓".to_string(), Tone::Success)
    );
    assert_eq!(
        detail(Checks::Failing),
        ("#348 ×".to_string(), Tone::Danger)
    );
    assert_eq!(
        detail(Checks::Pending),
        ("#348 ◷".to_string(), Tone::Accent)
    );
    assert_eq!(detail(Checks::None), ("#348".to_string(), Tone::Muted));
}

#[test]
fn the_pull_request_replaces_the_elapsed_time() {
    let agent = [state("%1", "api", "review", Some(Status::Done), 0)];

    let row = rows(
        &agent,
        90,
        options(Sort::Priority, true),
        &pr(7, Checks::Passing),
    )
    .remove(0);

    assert_eq!(row.detail.as_deref(), Some("#7 ✓"));
}

#[test]
fn checks_fail_if_any_fails_are_pending_if_any_runs_and_pass_otherwise() {
    let run = |status: &str, conclusion: Option<&str>| json!({"__typename": "CheckRun", "status": status, "conclusion": conclusion});
    let context = |state: &str| json!({"__typename": "StatusContext", "state": state});

    assert_eq!(checks_from_rollup(&[]), Checks::None);
    assert_eq!(
        checks_from_rollup(&[
            run("COMPLETED", Some("SUCCESS")),
            run("COMPLETED", Some("SKIPPED")),
            context("SUCCESS")
        ]),
        Checks::Passing
    );
    assert_eq!(
        checks_from_rollup(&[run("COMPLETED", Some("SUCCESS")), run("IN_PROGRESS", None)]),
        Checks::Pending
    );
    assert_eq!(checks_from_rollup(&[context("PENDING")]), Checks::Pending);
    assert_eq!(
        checks_from_rollup(&[run("IN_PROGRESS", None), run("COMPLETED", Some("FAILURE"))]),
        Checks::Failing
    );
    assert_eq!(checks_from_rollup(&[context("ERROR")]), Checks::Failing);
}

#[test]
fn gh_pr_list_output_is_indexed_by_branch() {
    let output = r#"[
        {"number": 348, "headRefName": "chore/cve", "statusCheckRollup": [{"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS"}]},
        {"number": 12, "headRefName": "feat/x", "statusCheckRollup": []}
    ]"#;

    let prs = pull_requests_by_branch(output).unwrap();

    assert_eq!(
        prs["chore/cve"],
        PullRequest {
            number: 348,
            checks: Checks::Passing
        }
    );
    assert_eq!(
        prs["feat/x"],
        PullRequest {
            number: 12,
            checks: Checks::None
        }
    );
}

struct Tree(std::path::PathBuf);

impl Tree {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tmux-aside-project-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Tree(dir)
    }

    fn dir(&self, path: &str) -> std::path::PathBuf {
        let dir = self.0.join(path);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_project_is_the_repository_root_with_a_git_directory() {
    let tree = Tree::new();
    tree.dir("api/.git");

    assert_eq!(
        tmux_aside_agents::project_name(&tree.dir("api/src/lib")),
        "api"
    );
}

#[test]
fn worktrees_belong_to_the_project_named_by_their_worktrees_folder() {
    let tree = Tree::new();
    let worktree = tree.dir("shop__worktrees/docs-x");
    std::fs::write(worktree.join(".git"), "gitdir: elsewhere").unwrap();

    assert_eq!(
        tmux_aside_agents::project_name(&tree.dir("shop__worktrees/docs-x/src")),
        "shop"
    );
}

#[test]
fn outside_a_repository_the_project_is_the_directory_name() {
    let tree = Tree::new();

    assert_eq!(tmux_aside_agents::project_name(&tree.dir("acme")), "acme");
}

#[test]
fn a_project_with_a_single_agent_still_gets_its_group_line() {
    let agent = state("%1", "web", "fix/x", Some(Status::Working), 0);

    let row = rows(
        std::slice::from_ref(&agent),
        0,
        options(Sort::Priority, false),
        &HashMap::new(),
    )
    .remove(0);
    assert_eq!(row.group.as_deref(), Some("web"));
    assert_eq!(row.lines, ["CC"]);

    let row = rows(&[agent], 0, flat(), &HashMap::new()).remove(0);
    assert_eq!(row.group, None);
    assert_eq!(row.lines, ["web", "CC"]);
}

#[test]
fn a_flat_list_puts_waiting_agents_first_across_projects() {
    let agents = vec![
        state("%1", "api", "a", Some(Status::Working), 0),
        state("%2", "web", "b", Some(Status::Waiting), 0),
    ];

    let order: Vec<String> = rows(&agents, 0, flat(), &HashMap::new())
        .into_iter()
        .map(|row| row.text)
        .collect();

    assert_eq!(order, ["b", "a"]);
}

#[test]
fn lines_that_repeat_the_window_name_are_skipped() {
    let mut agent = state("%1", "api", " fix/x", Some(Status::Working), 0);
    agent.title = "✳ fix/x".to_string();
    let mut other = agent.clone();
    other.pane_id = "%2".to_string();

    let rows = rows(
        &[agent, other],
        0,
        options(Sort::Priority, false),
        &branch("fix/x", None),
    );

    assert_eq!(rows[0].group.as_deref(), Some("api"));
    assert_eq!(rows[0].lines, ["CC"]);
}

#[test]
fn waiting_agents_show_how_long_they_have_waited_without_the_elapsed_option() {
    let agents = vec![
        state("%1", "api", "review", Some(Status::Waiting), 1_000),
        state("%2", "web", "build", Some(Status::Done), 1_000),
    ];

    let rows = rows(
        &agents,
        1_000 + 754,
        options(Sort::Priority, false),
        &HashMap::new(),
    );

    assert_eq!(rows[0].detail.as_deref(), Some("12:34"));
    assert_eq!(rows[1].detail, None);
}

#[test]
fn working_agents_show_a_spinner_on_the_right_instead_of_their_pull_request() {
    let agent = [state("%1", "api", "review", Some(Status::Working), 0)];

    let row = rows(
        &agent,
        0,
        options(Sort::Priority, false),
        &pr(7, Checks::Passing),
    )
    .remove(0);

    assert_eq!(row.detail.as_deref(), Some("spinner"));
    assert_eq!(row.detail_tone, Tone::Accent);
}

#[test]
fn waiting_agents_show_their_waiting_time_instead_of_their_pull_request() {
    let agent = [state("%1", "api", "review", Some(Status::Waiting), 1_000)];

    let row = rows(
        &agent,
        1_065,
        options(Sort::Priority, false),
        &pr(7, Checks::Passing),
    )
    .remove(0);

    assert_eq!(row.detail.as_deref(), Some("1:05"));
}

#[test]
fn p_opens_the_pull_request_from_the_agents_directory() {
    let mut agent = state("%1", "api", "review", Some(Status::Done), 0);
    agent.workdir = "/src/api".to_string();

    let with_pr = rows(
        std::slice::from_ref(&agent),
        0,
        options(Sort::Priority, false),
        &pr(7, Checks::Passing),
    )
    .remove(0);
    let open = with_pr.find_action("p").unwrap();
    assert_eq!(open.command, ["gh", "pr", "view", "--web", "7"]);
    assert_eq!(open.cwd.as_deref(), Some("/src/api"));

    let without_pr = rows(&[agent], 0, options(Sort::Priority, false), &HashMap::new()).remove(0);
    assert!(without_pr.find_action("p").is_none());
}

#[test]
fn agents_entering_a_watched_status_are_reported_once() {
    let agents = vec![
        state("%1", "api", "w1", Some(Status::Waiting), 0),
        state("%2", "api", "w2", Some(Status::Done), 0),
        state("%3", "api", "w3", Some(Status::Working), 0),
        state("%4", "api", "w4", Some(Status::Waiting), 0),
    ];
    let previous = HashMap::from([
        ("%1".to_string(), Some(Status::Working)),
        ("%2".to_string(), None),
        ("%3".to_string(), None),
        ("%4".to_string(), Some(Status::Waiting)),
    ]);

    let panes: Vec<&str> = entered(&previous, &agents, &[Status::Waiting, Status::Done])
        .into_iter()
        .map(|agent| agent.pane_id.as_str())
        .collect();

    assert_eq!(panes, ["%1", "%2"]);
}

#[test]
fn new_panes_count_as_having_had_no_status() {
    let agents = vec![state("%9", "api", "w", Some(Status::Waiting), 0)];

    assert_eq!(
        entered(&HashMap::new(), &agents, &[Status::Waiting]).len(),
        1
    );
}

#[test]
fn notifications_fill_the_template_with_the_agent() {
    let agent = AgentState {
        title: "✳ Fix the login".to_string(),
        ..state("%1", "api", "·  fix/login", Some(Status::Waiting), 0)
    };

    assert_eq!(
        notification("{project}/{window} {status}: {title}", &agent, Language::En),
        "api/fix/login waiting: Fix the login"
    );
}

#[test]
fn jumps_go_to_the_longest_waiting_agent_and_cycle_from_there() {
    let agents = vec![
        state("%1", "api", "w1", Some(Status::Waiting), 30),
        state("%2", "api", "w2", Some(Status::Waiting), 10),
        state("%3", "api", "w3", Some(Status::Working), 0),
        state("%4", "api", "w4", Some(Status::Done), 20),
    ];
    let next = |statuses: &[Status], current| {
        next_agent(&agents, statuses, current).map(|agent| agent.pane_id.as_str())
    };

    assert_eq!(next(&[Status::Waiting], "%3"), Some("%2"));
    assert_eq!(next(&[Status::Waiting], "%2"), Some("%1"));
    assert_eq!(next(&[Status::Waiting], "%1"), Some("%2"));
    assert_eq!(next(&[Status::Waiting, Status::Done], "%4"), Some("%2"));
    assert_eq!(next(&[], "%1"), None);
}

#[test]
fn jumps_fall_back_to_the_next_status_when_none_is_in_the_first() {
    let agents = vec![
        state("%1", "api", "w1", Some(Status::Done), 30),
        state("%2", "api", "w2", Some(Status::Done), 10),
        state("%3", "api", "w3", Some(Status::Working), 0),
    ];
    let next = |current| {
        next_agent(&agents, &[Status::Waiting, Status::Done], current)
            .map(|agent| agent.pane_id.as_str())
    };

    assert_eq!(next("%3"), Some("%2"));
    assert_eq!(next("%2"), Some("%1"));
    assert_eq!(next("%1"), Some("%2"));
}

#[test]
fn shortstats_become_insertions_and_deletions() {
    let diff = |insertions, deletions| {
        Some(Diff {
            insertions,
            deletions,
        })
    };

    assert_eq!(
        parse_shortstat(" 3 files changed, 12 insertions(+), 3 deletions(-)"),
        diff(12, 3)
    );
    assert_eq!(
        parse_shortstat(" 1 file changed, 1 insertion(+)"),
        diff(1, 0)
    );
    assert_eq!(
        parse_shortstat(" 1 file changed, 2 deletions(-)"),
        diff(0, 2)
    );
    assert_eq!(parse_shortstat(""), None);
}

#[test]
fn diff_stats_come_before_the_branch_so_a_long_branch_is_what_gets_cut() {
    let agent = AgentState {
        diff: Some(Diff {
            insertions: 12,
            deletions: 3,
        }),
        ..state("%1", "api", "w", None, 0)
    };
    let branches = HashMap::from([(
        "%1".to_string(),
        Branch {
            name: "feat/x".to_string(),
            pr: None,
        },
    )]);

    let rows = rows(&[agent], 0, flat(), &branches);

    assert_eq!(rows[0].lines, ["api · +12 -3 · feat/x", "CC"]);
}

#[test]
fn stalled_agents_are_red_and_say_so() {
    let agent = AgentState {
        stalled: true,
        ..state("%1", "api", "w", Some(Status::Working), 0)
    };

    let row = rows(&[agent], 0, options(Sort::Priority, false), &HashMap::new()).remove(0);

    assert_eq!(row.bar_tone, Tone::Danger);
    assert_eq!(
        (row.detail.as_deref(), row.detail_tone),
        (Some("stalled"), Tone::Danger)
    );
}

#[test]
fn agents_without_hooks_work_while_their_pane_changes_and_are_done_after() {
    let working = Some(Status::Working);
    let done = Some(Status::Done);

    assert_eq!(infer_status(None, true, false), working);
    assert_eq!(infer_status(working, false, false), done);
    assert_eq!(infer_status(working, false, true), None);
    assert_eq!(infer_status(done, false, false), done);
    assert_eq!(infer_status(None, false, false), None);
}

#[test]
fn a_stop_with_background_agents_running_is_not_done() {
    let stop = |tasks| json!({"hook_event_name": "Stop", "background_tasks": tasks});

    assert!(waits_for_background_agents(&stop(json!([
        {"id": "a1", "type": "subagent", "status": "running", "agent_type": "Explore"}
    ]))));
    assert!(!waits_for_background_agents(&stop(json!([
        {"id": "b1", "type": "shell", "status": "running", "command": "npm run dev"},
        {"id": "a2", "type": "subagent", "status": "completed"}
    ]))));
    assert!(!waits_for_background_agents(&stop(json!([]))));
    assert!(!waits_for_background_agents(
        &json!({"hook_event_name": "Stop"})
    ));
}

#[test]
fn an_antigravity_stop_that_is_not_fully_idle_is_not_done() {
    assert!(waits_for_background_agents(
        &json!({"terminationReason": "model_stop", "fullyIdle": false})
    ));
    assert!(!waits_for_background_agents(
        &json!({"terminationReason": "model_stop", "fullyIdle": true})
    ));
}

#[test]
fn rows_name_the_agent_tool_and_skip_a_title_that_repeats_the_project() {
    let agents: Vec<AgentState> = [("%1", "codex"), ("%2", "agy"), ("%3", "aider")]
        .into_iter()
        .map(|(pane, command)| AgentState {
            command: command.to_string(),
            title: "api".to_string(),
            ..state(pane, "api", "w", None, 0)
        })
        .collect();

    let lines: Vec<Vec<String>> = rows(&agents, 0, flat(), &HashMap::new())
        .into_iter()
        .map(|row| row.lines)
        .collect();

    assert_eq!(lines, [["api", "Codex"], ["api", "Agy"], ["api", "aider"]]);
}

#[test]
fn spanish_rows_name_the_stall_and_the_actions_in_spanish() {
    let agent = AgentState {
        stalled: true,
        ..state("%1", "api", "w", Some(Status::Working), 0)
    };
    let options = RowOptions {
        language: Language::Es,
        ..options(Sort::Priority, false)
    };

    let row = rows(&[agent], 0, options, &HashMap::new()).remove(0);

    assert_eq!(row.detail.as_deref(), Some("detenido"));
    assert_eq!(row.actions[0].label.as_deref(), Some("ir al agente"));
}

#[test]
fn spanish_notifications_name_the_status_in_spanish() {
    let agent = state("%1", "api", "fix", Some(Status::Done), 0);

    assert_eq!(
        notification("{window}: {status}", &agent, Language::Es),
        "fix: listo"
    );
}

#[test]
fn next_says_which_statuses_had_no_agents_in_each_language() {
    let statuses = [Status::Waiting, Status::Done];

    assert_eq!(
        (messages(Language::En).no_agents_in)(&statuses),
        "no waiting or done agents"
    );
    assert_eq!(
        (messages(Language::Es).no_agents_in)(&statuses),
        "sin agentes esperando o listos"
    );
}
