mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use common::TmuxServer;
use common::daemon::{Home, fixture, wait_for};
use serde_json::Value;
use tmux_aside::launch::Template;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// A clone of a local origin, with one local commit that origin does not have.
struct Repo {
    dir: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tmux-aside-repo-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(dir.join("origin")).unwrap();
        let repo = Repo { dir };
        git_in(&repo.origin(), &["init", "-q", "-b", "main"]);
        git_in(
            &repo.origin(),
            &["commit", "-q", "--allow-empty", "-m", "first"],
        );
        git_in(&repo.dir, &["clone", "-q", "origin", "clone"]);
        let origin = repo.origin();
        git_in(
            &repo.clone(),
            &["remote", "set-url", "origin", "git@github.com:org/shop.git"],
        );
        git_in(
            &repo.clone(),
            &[
                "config",
                &format!("url.{}.insteadOf", origin.display()),
                "git@github.com:org/shop.git",
            ],
        );
        git_in(
            &repo.clone(),
            &["commit", "-q", "--allow-empty", "-m", "local"],
        );
        repo
    }

    fn origin(&self) -> PathBuf {
        self.dir.join("origin")
    }

    fn clone(&self) -> PathBuf {
        self.dir.join("clone")
    }

    fn worktree(&self, handle: &str) -> PathBuf {
        self.dir.join("clone__worktrees").join(handle)
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn with_fake_agent(command: &mut Command) -> &mut Command {
    let path = std::env::var("PATH").unwrap_or_default();
    command.env(
        "PATH",
        format!("{}:{}:{path}", fixture("agent"), fixture("gh")),
    )
}

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

fn launch(server: &TmuxServer, repo: &Repo, extra: &[&str]) -> Output {
    let clone = repo.clone();
    let mut args = vec!["launch", clone.to_str().unwrap()];
    args.extend_from_slice(extra);
    with_fake_agent(&mut server.aside_command(&args))
        .output()
        .unwrap()
}

fn launch_ok(server: &TmuxServer, repo: &Repo, extra: &[&str]) -> Output {
    let output = launch(server, repo, extra);
    assert!(
        output.status.success(),
        "launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn implement(branch: &str) -> Vec<&str> {
    vec![
        "--template",
        "implement",
        "--task",
        "Add a dark theme",
        "--branch",
        branch,
    ]
}

fn launches_dir(home: &Home) -> PathBuf {
    home.path("state/tmux-aside/launches")
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
