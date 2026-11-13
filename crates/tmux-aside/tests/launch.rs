mod common;

use std::path::PathBuf;
use std::process::{Command, Output};

use common::TmuxServer;
use common::daemon::{Home, wait_for};
use common::repo::{Repo, git_in, implement, launch, launch_ok, launches_dir, with_fake_agent};
use serde_json::Value;
use tmux_aside::launch::Template;

const PR_URL: &str = "https://github.com/org/shop/pull/7";

/// Makes the fake `gh` answer `gh pr view` with an open pull request of `branch` in
/// `org/shop`, with `changes` applied on top.
fn answer_pull_request(command: &mut Command, repo: &Repo, branch: &str, changes: Value) {
    let mut answer = serde_json::json!({
        "url": PR_URL,
        "headRefName": branch,
        "isCrossRepository": false,
        "state": "OPEN",
    });
    for (key, value) in changes.as_object().unwrap() {
        answer[key] = value.clone();
    }
    let path = repo.dir.join("gh.json");
    std::fs::write(&path, answer.to_string()).unwrap();
    command.env("FAKE_GH_RESPONSE", path);
}

fn adopt(server: &TmuxServer, repo: &Repo, branch: &str, changes: Value) -> Output {
    let clone = repo.clone();
    let mut command = server.aside_command(&[
        "launch",
        clone.to_str().unwrap(),
        "--template",
        "adopt",
        "--pr",
        PR_URL,
    ]);
    with_fake_agent(&mut command);
    answer_pull_request(&mut command, repo, branch, changes);
    command.output().unwrap()
}

/// Pushes a branch with one commit to the origin, as a pull request's author would.
fn push_branch(repo: &Repo, branch: &str) -> String {
    git_in(&repo.origin(), &["checkout", "-q", "-b", branch]);
    git_in(
        &repo.origin(),
        &["commit", "-q", "--allow-empty", "-m", "work"],
    );
    let head = git_in(&repo.origin(), &["rev-parse", "HEAD"]);
    git_in(&repo.origin(), &["checkout", "-q", "main"]);
    head
}

fn status_json(server: &TmuxServer) -> Vec<Value> {
    let output = server.aside(&["status", "--json"]);
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn a_template_fills_in_the_task_reference_and_declares_its_pull_request() {
    let template =
        Template::parse("---\npull_request: true\n---\nImplement ${task}, then open a PR.\n")
            .unwrap();

    assert!(template.opens_pull_request);
    assert_eq!(
        template.prompt("https://example.com/issues/7"),
        "Implement https://example.com/issues/7, then open a PR.\n"
    );
}

#[test]
fn a_template_without_front_matter_opens_no_pull_request() {
    let template = Template::parse("Research ${task}.").unwrap();

    assert!(!template.opens_pull_request);
    assert_eq!(template.prompt("caching"), "Research caching.");
}

#[test]
fn launch_creates_a_branch_and_a_worktree_next_to_the_clone_and_starts_the_agent() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();

    launch_ok(&server, &repo, &implement("feat/dark-theme"));

    let worktree = repo.worktree("feat-dark-theme");
    assert_eq!(
        git_in(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "feat/dark-theme"
    );
    assert_eq!(
        git_in(&worktree, &["rev-parse", "HEAD"]),
        git_in(&repo.clone(), &["rev-parse", "origin/HEAD"]),
        "the branch starts from the remote's default branch"
    );
    let windows = server.tmux(&["list-windows", "-t", "main", "-F", "#{window_name}"]);
    assert!(windows.lines().any(|name| name == "feat-dark-theme"));
    let prompt = worktree.join(".agent-prompt");
    wait_for("the agent to start", || prompt.exists());
    let prompt = std::fs::read_to_string(prompt).unwrap();
    assert!(prompt.contains("Add a dark theme"), "{prompt}");
    assert!(!prompt.contains("${task}"), "{prompt}");
}

#[test]
fn a_base_branch_can_be_given() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();

    launch_ok(
        &server,
        &repo,
        &[implement("fix/x").as_slice(), &["--base", "HEAD"]].concat(),
    );

    assert_eq!(
        git_in(&repo.worktree("fix-x"), &["rev-parse", "HEAD"]),
        git_in(&repo.clone(), &["rev-parse", "HEAD"])
    );
}

#[test]
fn launching_keeps_the_focus_on_the_current_window() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let active = server.tmux(&["display-message", "-p", "-t", "main", "#{window_id}"]);

    launch_ok(&server, &repo, &implement("feat/focus"));

    assert_eq!(server.window_ids().len(), 2);
    assert_eq!(
        server.tmux(&["display-message", "-p", "-t", "main", "#{window_id}"]),
        active
    );
}

#[test]
fn outside_tmux_the_window_opens_in_a_default_session_created_if_missing() {
    let repo = Repo::new();
    let home = Home::new("launch");
    let tmpdir = repo.dir.join("t");
    std::fs::create_dir_all(&tmpdir).unwrap();
    let clone = repo.clone();
    let mut args = vec!["launch", clone.to_str().unwrap()];
    args.extend(implement("feat/outside"));

    let output = with_fake_agent(
        Command::new(env!("CARGO_BIN_EXE_tmux-aside"))
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env("TMUX_TMPDIR", &tmpdir)
            .env("TMUX_ASIDE_LANG", "en")
            .envs(home.envs())
            .args(&args),
    )
    .output()
    .unwrap();

    let socket = std::fs::read_dir(&tmpdir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("default"))
        .find(|socket| socket.exists())
        .expect("no tmux server started in TMUX_TMPDIR");
    let tmux = |args: &[&str]| {
        let output = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    let windows = tmux(&["list-windows", "-a", "-F", "#{session_name} #{window_name}"]);
    tmux(&["kill-server"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(windows, "tmux-aside feat-outside");
}

#[test]
fn an_unknown_template_fails_and_creates_nothing() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();

    let output = launch(
        &server,
        &repo,
        &["--template", "nope", "--task", "x", "--branch", "feat/nope"],
    );

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown template: nope"));
    assert_nothing_created(&server, &repo, "feat/nope");
}

#[test]
fn a_path_that_is_not_a_repository_fails_and_creates_nothing() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let elsewhere = repo.dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();

    let mut args = vec!["launch", elsewhere.to_str().unwrap()];
    args.extend(implement("feat/nowhere"));
    let output = with_fake_agent(&mut server.aside_command(&args))
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a git repository"));
    assert_nothing_created(&server, &repo, "feat/nowhere");
}

fn assert_nothing_created(server: &TmuxServer, repo: &Repo, branch: &str) {
    assert_eq!(server.window_ids().len(), 1);
    assert!(!repo.dir.join("clone__worktrees").exists());
    assert_eq!(git_in(&repo.clone(), &["branch", "--list", branch]), "");
    assert!(!launches_dir(&server.home).exists());
}

#[test]
fn status_lists_every_launch_with_its_fields() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    launch_ok(&server, &repo, &implement("feat/b"));
    launch_ok(&server, &repo, &implement("feat/a"));

    let launches = status_json(&server);

    assert_eq!(launches.len(), 2);
    let launch = &launches[0];
    let clone = std::fs::canonicalize(repo.clone()).unwrap();
    assert_eq!(launch["branch"], "feat/a");
    assert_eq!(launch["repository"], clone.to_str().unwrap());
    assert_eq!(
        launch["worktree"],
        clone
            .parent()
            .unwrap()
            .join("clone__worktrees/feat-a")
            .to_str()
            .unwrap()
    );
    assert_eq!(launch["session"], "main");
    assert!(launch["window"].as_str().unwrap().starts_with('@'));
    assert!(launch["pane"].as_str().unwrap().starts_with('%'));
    assert_eq!(launch["ai"], "claude");
    assert_eq!(launch["template"], "implement");
    assert_eq!(launch["task"], "Add a dark theme");
    assert_eq!(launch["opens_pull_request"], true);
    assert_eq!(launch["pull_request"], Value::Null);
}

#[test]
fn status_prints_the_same_as_plain_text() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    launch_ok(&server, &repo, &implement("feat/text"));

    let text = String::from_utf8(server.aside(&["status"]).stdout).unwrap();

    for expected in [
        "feat/text",
        "clone__worktrees/feat-text",
        "main",
        "claude",
        "implement",
        "Add a dark theme",
        "not opened yet",
    ] {
        assert!(text.contains(expected), "{expected} missing from:\n{text}");
    }
}

#[test]
fn status_text_is_translated() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    launch_ok(&server, &repo, &implement("feat/es"));

    let output = server
        .aside_command(&["status"])
        .env("TMUX_ASIDE_LANG", "es")
        .output()
        .unwrap();

    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Rama"), "{text}");
    assert!(text.contains("aún no abierto"), "{text}");
}

#[test]
fn a_launch_is_still_listed_after_the_tmux_server_restarts() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    launch_ok(&server, &repo, &implement("feat/restart"));
    server.tmux(&["kill-server"]);

    let output = server
        .aside_command(&["status", "--json"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let launches: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0]["branch"], "feat/restart");
}

#[test]
fn a_users_template_overrides_a_bundled_one_and_may_open_no_pull_request() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let templates = server.home.path("config/tmux-aside/templates");
    std::fs::create_dir_all(&templates).unwrap();
    std::fs::write(templates.join("implement.md"), "Just look into ${task}.\n").unwrap();

    launch_ok(&server, &repo, &implement("research/theme"));

    let prompt = repo.worktree("research-theme").join(".agent-prompt");
    wait_for("the agent to start", || prompt.exists());
    assert_eq!(
        std::fs::read_to_string(prompt).unwrap(),
        "Just look into Add a dark theme.\n"
    );
    assert_eq!(status_json(&server)[0]["opens_pull_request"], false);
}

#[test]
fn an_existing_branch_fails_and_creates_nothing() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    git_in(&repo.clone(), &["branch", "feat/taken"]);

    let output = launch(&server, &repo, &implement("feat/taken"));

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("the branch already exists"));
    assert_eq!(server.window_ids().len(), 1);
    assert!(!repo.dir.join("clone__worktrees").exists());
}

#[test]
fn a_launch_with_the_same_name_from_another_clone_does_not_replace_the_first() {
    let server = TmuxServer::start(1);
    let first = Repo::new();
    let second = Repo::new();
    launch_ok(&server, &first, &implement("feat/same"));

    let output = launch(&server, &second, &implement("feat/same"));

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("already exists"));
    assert!(!second.dir.join("clone__worktrees").exists());
    let launches = status_json(&server);
    assert_eq!(launches.len(), 1);
    assert_eq!(
        launches[0]["repository"],
        std::fs::canonicalize(first.clone())
            .unwrap()
            .to_str()
            .unwrap()
    );
}

#[test]
fn launch_pr_checks_out_the_pull_requests_branch_and_starts_the_agent_on_it() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let head = push_branch(&repo, "feat/pr");

    let output = adopt(&server, &repo, "feat/pr", serde_json::json!({}));

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let worktree = repo.worktree("feat-pr");
    assert_eq!(git_in(&worktree, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git_in(&worktree, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        "origin/feat/pr"
    );
    assert_eq!(server.window_ids().len(), 2);
    let prompt = worktree.join(".agent-prompt");
    wait_for("the agent to start", || prompt.exists());
    assert!(std::fs::read_to_string(prompt).unwrap().contains(PR_URL));
    let launch = &status_json(&server)[0];
    assert_eq!(launch["pull_request"], PR_URL);
    assert_eq!(launch["task"], PR_URL);
    assert_eq!(launch["template"], "adopt");
    assert_eq!(launch["opens_pull_request"], true);
}

#[test]
fn launch_pr_reuses_a_local_branch_of_the_pull_request() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    git_in(&repo.clone(), &["branch", "feat/local"]);

    let output = adopt(&server, &repo, "feat/local", serde_json::json!({}));

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        git_in(&repo.worktree("feat-local"), &["rev-parse", "HEAD"]),
        git_in(&repo.clone(), &["rev-parse", "feat/local"])
    );
}

#[test]
fn launch_pr_of_a_fork_fails_and_creates_nothing() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();

    let output = adopt(
        &server,
        &repo,
        "feat/fork",
        serde_json::json!({"isCrossRepository": true}),
    );

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("forks are not supported"));
    assert_nothing_created(&server, &repo, "feat/fork");
}

/// A worktree of `branch` in the clone, with an agent pane running in it whose hooks recorded
/// it as Claude Code. Returns the pane id.
fn agent_in_worktree(server: &TmuxServer, repo: &Repo, branch: &str) -> String {
    let worktree = repo.worktree(&branch.replace('/', "-"));
    git_in(
        &repo.clone(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            branch,
            worktree.to_str().unwrap(),
        ],
    );
    let pane = server.tmux(&[
        "new-window",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "-c",
        worktree.to_str().unwrap(),
        "exec cat",
    ]);
    let expected = std::fs::canonicalize(&worktree).unwrap();
    wait_for("the pane to be in its worktree", || {
        let path = server.tmux(&["display-message", "-p", "-t", &pane, "#{pane_current_path}"]);
        std::fs::canonicalize(path).is_ok_and(|path| path == expected)
    });
    let pid = server.tmux(&["display-message", "-p", "-t", &pane, "#{pane_pid}"]);
    let agents = server.home.path("state/tmux-aside/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join(format!("{}.json", pane.trim_start_matches('%'))),
        serde_json::json!({"pane_pid": pid.parse::<u32>().unwrap(), "command": "claude"})
            .to_string(),
    )
    .unwrap();
    pane
}

fn follow(server: &TmuxServer, repo: &Repo, pane: &str, branch: &str) -> Output {
    let mut command = server.aside_command(&["follow", PR_URL]);
    command.env("TMUX_PANE", pane);
    with_fake_agent(&mut command);
    answer_pull_request(&mut command, repo, branch, serde_json::json!({}));
    command.output().unwrap()
}

#[test]
fn follow_records_the_agent_in_this_pane_without_creating_anything() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let pane = agent_in_worktree(&server, &repo, "feat/f");
    let windows = server.window_ids().len();

    let output = follow(&server, &repo, &pane, "feat/f");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(server.window_ids().len(), windows);
    let launch = &status_json(&server)[0];
    let clone = std::fs::canonicalize(repo.clone()).unwrap();
    assert_eq!(launch["pane"], pane.as_str());
    assert_eq!(launch["branch"], "feat/f");
    assert_eq!(launch["repository"], clone.to_str().unwrap());
    assert_eq!(
        launch["worktree"],
        std::fs::canonicalize(repo.worktree("feat-f"))
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert_eq!(launch["pull_request"], PR_URL);
    assert_eq!(launch["ai"], "claude");
    assert_eq!(launch["template"], serde_json::Value::Null);
}

#[test]
fn follow_outside_an_agent_pane_fails_and_records_nothing() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let pane = agent_in_worktree(&server, &repo, "feat/f");
    let agents = server.home.path("state/tmux-aside/agents");
    std::fs::remove_dir_all(&agents).unwrap();

    let output = follow(&server, &repo, &pane, "feat/f");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("pane of an AI agent"));
    assert!(!launches_dir(&server.home).exists());
}

#[test]
fn follow_outside_tmux_fails() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let pane = agent_in_worktree(&server, &repo, "feat/f");
    let mut command = server.aside_command(&["follow", PR_URL]);
    command.env_remove("TMUX").env("TMUX_PANE", &pane);

    let output = command.output().unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("pane of an AI agent"));
}

#[test]
fn follow_from_another_branch_than_the_pull_requests_fails() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let pane = agent_in_worktree(&server, &repo, "feat/f");

    let output = follow(&server, &repo, &pane, "feat/other");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("the pull request is on feat/other"));
    assert!(!launches_dir(&server.home).exists());
}

#[test]
fn a_pull_request_already_followed_cannot_be_followed_or_adopted_again() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let pane = agent_in_worktree(&server, &repo, "feat/f");
    assert!(follow(&server, &repo, &pane, "feat/f").status.success());

    let again = follow(&server, &repo, &pane, "feat/f");
    let adopted = adopt(&server, &repo, "feat/f", serde_json::json!({}));

    for output in [again, adopted] {
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("is already followed by clone__feat-f"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(status_json(&server).len(), 1);
}

#[test]
fn a_pull_request_that_is_not_open_cannot_be_adopted() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    push_branch(&repo, "feat/done");

    let output = adopt(
        &server,
        &repo,
        "feat/done",
        serde_json::json!({"state": "MERGED"}),
    );

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("is not open"));
    assert_nothing_created(&server, &repo, "feat/done");
}

#[test]
fn a_pull_request_of_another_repository_cannot_be_adopted() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    push_branch(&repo, "feat/elsewhere");

    let output = adopt(
        &server,
        &repo,
        "feat/elsewhere",
        serde_json::json!({"url": "https://github.com/org/other/pull/3"}),
    );

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("is not a pull request of the GitHub repository")
    );
    assert_nothing_created(&server, &repo, "feat/elsewhere");
}

#[test]
fn qa_launches_one_qa_agent_for_followed_pull_requests() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let pane = agent_in_worktree(&server, &repo, "feat/f");
    assert!(follow(&server, &repo, &pane, "feat/f").status.success());
    let windows = server.window_ids().len();

    let mut command = server.aside_command(&["qa", PR_URL]);
    with_fake_agent(&mut command);
    let output = command.output().unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(server.window_ids().len(), windows + 1);
    let prompt = repo.clone().join(".agent-prompt");
    wait_for("the QA agent to start", || prompt.exists());
    let prompt = std::fs::read_to_string(prompt).unwrap();
    assert!(prompt.contains(PR_URL), "{prompt}");
    assert!(prompt.contains("--add-label qa:passed"), "{prompt}");
    let qa = status_json(&server)
        .into_iter()
        .find(|launch| launch["template"] == "qa")
        .expect("no QA launch");
    assert_eq!(qa["task"], PR_URL);
    assert_eq!(qa["opens_pull_request"], false);
}

#[test]
fn qa_of_a_pull_request_no_launch_follows_fails() {
    let server = TmuxServer::start(1);

    let mut command = server.aside_command(&["qa", PR_URL]);
    with_fake_agent(&mut command);
    let output = command.output().unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("is not followed by any launch"));
    assert_eq!(server.window_ids().len(), 1);
}

#[test]
fn the_qa_agent_is_told_the_labels_the_config_names() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    server.home.write_config(
        "criteria:\n  ready_to_deploy:\n    args: [\"--passed-label\", \"qa:ok\", \"--failed-label\", \"qa:ko\"]\n",
    );
    let pane = agent_in_worktree(&server, &repo, "feat/f");
    assert!(follow(&server, &repo, &pane, "feat/f").status.success());

    let mut command = server.aside_command(&["qa", PR_URL]);
    with_fake_agent(&mut command);
    assert!(command.output().unwrap().status.success());

    let prompt = repo.clone().join(".agent-prompt");
    wait_for("the QA agent to start", || prompt.exists());
    let prompt = std::fs::read_to_string(prompt).unwrap();
    assert!(
        prompt.contains("--add-label qa:ok --remove-label qa:ko"),
        "{prompt}"
    );
    assert!(!prompt.contains("qa:passed"), "{prompt}");
}

/// Runs `tmux-aside deployed` in the clone with `args`.
fn run_deployed(server: &TmuxServer, repo: &Repo, args: &[&str]) -> Output {
    let mut command = server.aside_command(&[&["deployed"], args].concat());
    with_fake_agent(&mut command).current_dir(repo.clone());
    command.output().unwrap()
}

#[test]
fn deployed_runs_the_ai_headless_with_the_tasks_and_urls_of_the_deployed_pull_requests() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    own_template(&server, "deployed", "Sync ${task} (${urls})");
    let windows = server.window_ids().len();

    let output = run_deployed(
        &server,
        &repo,
        &[
            "--task",
            "T-2",
            "--task",
            "T-1",
            "--task",
            "T-2",
            PR_URL,
            "https://github.com/org/shop/pull/3",
        ],
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(repo.clone().join(".agent-prompt")).unwrap(),
        format!("Sync T-1 T-2 (https://github.com/org/shop/pull/3 {PR_URL})")
    );
    assert_eq!(server.window_ids().len(), windows);
}

#[test]
fn deployed_opens_a_window_when_its_action_asks_for_one() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    server
        .home
        .write_config("on_deployed:\n  template: sync\n  window: true\n");
    own_template(&server, "sync", "Sync ${task}");
    let windows = server.window_ids().len();

    let output = run_deployed(&server, &repo, &[PR_URL]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(server.window_ids().len(), windows + 1);
    let prompt = repo.clone().join(".agent-prompt");
    wait_for("the agent to start", || prompt.exists());
    assert_eq!(
        std::fs::read_to_string(prompt).unwrap(),
        format!("Sync {PR_URL}")
    );
}

#[test]
fn deployed_without_its_template_fails() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();

    let output = run_deployed(&server, &repo, &[PR_URL]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown template: deployed"));
}

/// A launch of `feat/r` whose agent started, then lost its window as in a crash.
fn crashed_launch(server: &TmuxServer, repo: &Repo) -> (Value, PathBuf) {
    launch_ok(server, repo, &implement("feat/r"));
    let prompt = repo.worktree("feat-r").join(".agent-prompt");
    wait_for("the agent to start", || prompt.exists());
    let launch = status_json(server).remove(0);
    server.tmux(&["kill-window", "-t", launch["window"].as_str().unwrap()]);
    std::fs::remove_file(&prompt).unwrap();
    (launch, prompt)
}

fn resurrect(server: &TmuxServer, extra: &[&str]) -> Output {
    let mut command = server.aside_command(&[&["resurrect"], extra].concat());
    with_fake_agent(&mut command);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn resurrect_reopens_a_launch_whose_window_is_gone_and_resumes_its_conversation() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let (before, prompt) = crashed_launch(&server, &repo);
    let windows = server.window_ids().len();

    let output = resurrect(&server, &[]);

    assert!(String::from_utf8_lossy(&output.stdout).contains("reopened clone__feat-r"));
    assert_eq!(server.window_ids().len(), windows + 1);
    wait_for("the agent to resume", || prompt.exists());
    assert_eq!(std::fs::read_to_string(&prompt).unwrap(), "--continue");
    let after = status_json(&server).remove(0);
    assert_ne!(after["pane"], before["pane"]);
    assert!(
        server
            .window_ids()
            .contains(&after["window"].as_str().unwrap().to_string())
    );
}

#[test]
fn resurrect_dry_run_lists_what_it_would_reopen_and_opens_nothing() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    crashed_launch(&server, &repo);
    let windows = server.window_ids().len();

    let output = resurrect(&server, &["--dry-run"]);

    assert!(String::from_utf8_lossy(&output.stdout).contains("would reopen clone__feat-r"));
    assert_eq!(server.window_ids().len(), windows);
}

#[test]
fn resurrect_leaves_a_launch_whose_agent_is_still_running() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    launch_ok(&server, &repo, &implement("feat/r"));
    let prompt = repo.worktree("feat-r").join(".agent-prompt");
    wait_for("the agent to start in its worktree", || prompt.exists());
    let windows = server.window_ids().len();

    let output = resurrect(&server, &[]);

    assert!(String::from_utf8_lossy(&output.stdout).contains("nothing to reopen"));
    assert_eq!(server.window_ids().len(), windows);
}

#[test]
fn launch_records_the_group() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();

    launch_ok(
        &server,
        &repo,
        &[implement("feat/grouped").as_slice(), &["--group", "theme"]].concat(),
    );

    assert_eq!(status_json(&server)[0]["group"], "theme");
}

#[test]
fn fix_launches_an_agent_on_a_new_branch_in_the_same_group() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let pane = agent_in_worktree(&server, &repo, "feat/f");
    assert!(follow(&server, &repo, &pane, "feat/f").status.success());

    let mut command = server.aside_command(&["fix", PR_URL]);
    with_fake_agent(&mut command);
    let output = command.output().unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let launches = status_json(&server);
    let original = launches
        .iter()
        .find(|launch| launch["branch"] == "feat/f")
        .unwrap();
    let fix = launches
        .iter()
        .find(|launch| launch["branch"] == "fix/qa-7")
        .unwrap();
    assert_eq!(original["group"], "clone__feat-f");
    assert_eq!(fix["group"], "clone__feat-f");
    assert_eq!(fix["template"], "fix");
    let prompt = repo.worktree("fix-qa-7").join(".agent-prompt");
    wait_for("the fix agent to start", || prompt.exists());
    let prompt = std::fs::read_to_string(prompt).unwrap();
    assert!(
        prompt.contains(&format!("the QA of {PR_URL} failed")),
        "{prompt}"
    );
}

#[test]
fn a_second_fix_gets_a_branch_of_its_own() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let pane = agent_in_worktree(&server, &repo, "feat/f");
    assert!(follow(&server, &repo, &pane, "feat/f").status.success());

    for _ in 0..2 {
        let mut command = server.aside_command(&["fix", PR_URL]);
        with_fake_agent(&mut command);
        assert!(command.output().unwrap().status.success());
    }

    let branches: Vec<String> = status_json(&server)
        .iter()
        .map(|launch| launch["branch"].as_str().unwrap().to_string())
        .collect();
    assert!(branches.contains(&"fix/qa-7".to_string()), "{branches:?}");
    assert!(branches.contains(&"fix/qa-7-2".to_string()), "{branches:?}");
}

fn own_template(server: &TmuxServer, name: &str, text: &str) {
    let templates = server.home.path("config/tmux-aside/templates");
    std::fs::create_dir_all(&templates).unwrap();
    std::fs::write(templates.join(format!("{name}.md")), text).unwrap();
}

fn launched_ai(server: &TmuxServer, repo: &Repo, template: &str, branch: &str) -> Value {
    launch_ok(
        server,
        repo,
        &["--template", template, "--task", "x", "--branch", branch],
    );
    let prompt = repo
        .worktree(&branch.replace('/', "-"))
        .join(".agent-prompt");
    wait_for("the agent to start", || prompt.exists());
    status_json(server)
        .into_iter()
        .find(|launch| launch["branch"] == branch)
        .unwrap()["ai"]
        .clone()
}

#[test]
fn a_template_chooses_the_ai_over_the_config() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    server.home.write_config("ai: agy\n");
    own_template(&server, "codex-work", "---\nai: codex\n---\nDo ${task}.\n");

    assert_eq!(
        launched_ai(&server, &repo, "codex-work", "feat/codex"),
        "codex"
    );
    assert_eq!(
        std::fs::read_to_string(repo.worktree("feat-codex").join(".agent-prompt")).unwrap(),
        "Do x.\n"
    );
    assert_eq!(launched_ai(&server, &repo, "implement", "feat/agy"), "agy");
}

#[test]
fn antigravity_gets_the_prompt_as_an_interactive_one() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    own_template(&server, "agy-work", "---\nai: agy\n---\nDo ${task}.\n");

    launched_ai(&server, &repo, "agy-work", "feat/agy");

    let prompt = std::fs::read_to_string(repo.worktree("feat-agy").join(".agent-prompt")).unwrap();
    assert_eq!(prompt, "Do x.\n");
}

#[test]
fn an_unsupported_ai_fails_with_a_clear_message_and_creates_nothing() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    own_template(&server, "aider-work", "---\nai: aider\n---\nDo ${task}.\n");

    let output = launch(
        &server,
        &repo,
        &[
            "--template",
            "aider-work",
            "--task",
            "x",
            "--branch",
            "feat/aider",
        ],
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("unsupported AI: aider"), "{stderr}");
    assert_nothing_created(&server, &repo, "feat/aider");
}

#[test]
fn an_unsupported_ai_in_the_config_fails_too() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    server.home.write_config("ai: aider\n");

    let output = launch(&server, &repo, &implement("feat/aider"));

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported AI: aider"));
    assert_nothing_created(&server, &repo, "feat/aider");
}

/// A Pack in the scratch home with `templates`, each as name and text, named in the config.
fn use_pack(server: &TmuxServer, manifest: &str, templates: &[(&str, &str)]) {
    let pack = server.home.path("team-pack");
    std::fs::create_dir_all(pack.join("templates")).unwrap();
    std::fs::write(pack.join("pack.yaml"), manifest).unwrap();
    for (name, text) in templates {
        std::fs::write(pack.join(format!("templates/{name}.md")), text).unwrap();
    }
    server
        .home
        .write_config(&format!("pack: \"{}\"\n", pack.display()));
}

fn launched_prompt(server: &TmuxServer, repo: &Repo, template: &str, branch: &str) -> String {
    launch_ok(
        server,
        repo,
        &["--template", template, "--task", "x", "--branch", branch],
    );
    let prompt = repo
        .worktree(&branch.replace('/', "-"))
        .join(".agent-prompt");
    wait_for("the agent to start", || prompt.exists());
    std::fs::read_to_string(prompt).unwrap()
}

#[test]
fn templates_resolve_from_the_user_then_the_pack_then_the_bundled_ones() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    use_pack(
        &server,
        "templates: templates\n",
        &[
            ("implement", "Pack implements ${task}.\n"),
            ("review", "Pack reviews ${task}.\n"),
        ],
    );
    own_template(&server, "review", "I review ${task}.\n");

    assert_eq!(
        launched_prompt(&server, &repo, "implement", "feat/a"),
        "Pack implements x.\n"
    );
    assert_eq!(
        launched_prompt(&server, &repo, "review", "feat/b"),
        "I review x.\n"
    );
    assert!(launched_prompt(&server, &repo, "adopt", "feat/c").contains("taking over"));
}

#[test]
fn an_invalid_pack_is_reported_and_the_defaults_are_used() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    use_pack(
        &server,
        "criteria:\n  merged:\n    command: x\n",
        &[("implement", "Pack implements ${task}.\n")],
    );

    let output = launch_ok(&server, &repo, &implement("feat/a"));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid pack manifest"), "{stderr}");
    assert!(stderr.contains("using the default"), "{stderr}");
    let prompt = repo.worktree("feat-a").join(".agent-prompt");
    wait_for("the agent to start", || prompt.exists());
    assert!(
        std::fs::read_to_string(prompt)
            .unwrap()
            .contains("Add a dark theme")
    );
}

#[test]
fn follow_accepts_a_merged_pull_request_and_keeps_its_worktree() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let pane = agent_in_worktree(&server, &repo, "feat/f");
    let mut command = server.aside_command(&["follow", PR_URL]);
    command.env("TMUX_PANE", &pane);
    with_fake_agent(&mut command);
    answer_pull_request(
        &mut command,
        &repo,
        "feat/f",
        serde_json::json!({"state": "MERGED"}),
    );

    let output = command.output().unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(status_json(&server)[0]["keeps_worktree"], true);
}

#[test]
fn follow_refuses_a_closed_pull_request() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let pane = agent_in_worktree(&server, &repo, "feat/f");
    let mut command = server.aside_command(&["follow", PR_URL]);
    command.env("TMUX_PANE", &pane);
    with_fake_agent(&mut command);
    answer_pull_request(
        &mut command,
        &repo,
        "feat/f",
        serde_json::json!({"state": "CLOSED"}),
    );

    let output = command.output().unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("is not open"));
}

#[test]
fn qa_of_a_discovered_pull_request_opens_in_the_current_directory() {
    let server = TmuxServer::start(1);
    let here = server.home.path("here");
    std::fs::create_dir_all(&here).unwrap();
    let launches = launches_dir(&server.home);
    std::fs::create_dir_all(&launches).unwrap();
    std::fs::write(
        launches.join("shop__pr-7.json"),
        serde_json::json!({
            "id": "shop__pr-7", "repository": "", "branch": "feat/f", "worktree": "",
            "session": "", "window": "", "pane": "", "ai": "claude", "template": null,
            "task": PR_URL, "opens_pull_request": true, "pull_request": PR_URL,
            "keeps_worktree": true
        })
        .to_string(),
    )
    .unwrap();

    let mut command = server.aside_command(&["qa", PR_URL]);
    command.current_dir(&here);
    with_fake_agent(&mut command);
    let output = command.output().unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let prompt = here.join(".agent-prompt");
    wait_for("the QA agent to start", || prompt.exists());
    assert!(std::fs::read_to_string(prompt).unwrap().contains(PR_URL));
    let qa = status_json(&server)
        .into_iter()
        .find(|launch| launch["template"] == "qa")
        .unwrap();
    assert!(
        qa["id"].as_str().unwrap().starts_with("shop__qa-7-"),
        "{qa}"
    );
}

#[test]
fn status_shows_the_stage_the_watcher_recorded() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    launch_ok(&server, &repo, &implement("feat/staged"));
    std::fs::write(
        server.home.path("state/tmux-aside/stages.json"),
        serde_json::json!({"clone__feat-staged": {
            "stage": "qa", "stage_name": "QA", "summary": "waiting for QA"
        }})
        .to_string(),
    )
    .unwrap();

    let text = String::from_utf8(server.aside(&["status"]).stdout).unwrap();
    let launch = &status_json(&server)[0];

    assert!(text.contains("QA · waiting for QA"), "{text}");
    assert_eq!(launch["stage"], "qa");
    assert_eq!(launch["summary"], "waiting for QA");
}

#[test]
fn status_without_a_recorded_stage_shows_a_dash() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    launch_ok(&server, &repo, &implement("feat/unstaged"));

    let launch = &status_json(&server)[0];

    assert!(launch.get("stage").is_none());
}
