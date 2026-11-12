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
    command.env("PATH", format!("{}:{path}", fixture("agent")))
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
