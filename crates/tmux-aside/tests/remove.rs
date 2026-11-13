mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::TmuxServer;
use common::daemon::{fixture, wait_for};
use common::repo::{Repo, git_in, implement, launch_ok, launches_dir};

/// Launches an agent on `branch` and waits for it to start, with its prompt file ignored as a
/// real agent's files would be, so that the worktree loses nothing when removed.
fn launched(server: &TmuxServer, repo: &Repo, branch: &str) -> PathBuf {
    launch_ok(server, repo, &implement(branch));
    let worktree = repo.worktree(&branch.replace('/', "-"));
    wait_for("the agent to start", || {
        worktree.join(".agent-prompt").exists()
    });
    std::fs::write(repo.clone().join(".git/info/exclude"), ".agent-prompt\n").unwrap();
    worktree
}

/// A worktree in `clone__worktrees/` that no launch follows, on a new branch at origin's head.
fn orphan(repo: &Repo, branch: &str) -> PathBuf {
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
            "origin/main",
        ],
    );
    worktree
}

/// `tmux-aside` with a fake `gh` that answers every call with `answer`.
fn aside_with_gh(server: &TmuxServer, repo: &Repo, args: &[&str], answer: &str) -> Output {
    let path = repo.dir.join("gh.json");
    std::fs::write(&path, answer).unwrap();
    let mut command = server.aside_command(args);
    let current = std::env::var("PATH").unwrap_or_default();
    command
        .env("PATH", format!("{}:{current}", fixture("gh")))
        .env("FAKE_GH_RESPONSE", path)
        .output()
        .unwrap()
}

fn merged_at(head: &str) -> String {
    serde_json::json!([{"state": "MERGED", "headRefOid": head, "isCrossRepository": false}])
        .to_string()
}

fn succeeded(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn branch_exists(repo: &Repo, branch: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo.clone())
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .unwrap()
        .status
        .success()
}

fn records(server: &TmuxServer) -> usize {
    std::fs::read_dir(launches_dir(&server.home))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                .count()
        })
        .unwrap_or(0)
}

fn head(path: &Path) -> String {
    git_in(path, &["rev-parse", "HEAD"])
}

#[test]
fn removing_a_launch_deletes_its_worktree_branch_record_and_window() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let worktree = launched(&server, &repo, "feat/done");
    let windows = server.window_ids().len();

    let report = succeeded(
        &server
            .aside_command(&["remove", "feat/done"])
            .output()
            .unwrap(),
    );

    assert!(report.contains("removed clone__feat-done"), "{report}");
    assert!(!worktree.exists());
    assert!(!branch_exists(&repo, "feat/done"));
    assert_eq!(records(&server), 0);
    assert_eq!(server.window_ids().len(), windows - 1);
}

#[test]
fn remove_without_a_launch_removes_the_launch_of_the_current_pane() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let worktree = launched(&server, &repo, "feat/here");
    let record = launches_dir(&server.home).join("clone__feat-here.json");
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(record).unwrap()).unwrap();
    let pane = record["pane"].as_str().unwrap().to_string();

    let mut command = server.aside_command(&["remove"]);
    command.env("TMUX_PANE", &pane);
    command.output().unwrap();

    wait_for("the worktree to be removed", || !worktree.exists());
    assert_eq!(records(&server), 0);
}

#[test]
fn a_launch_whose_worktree_has_uncommitted_changes_is_kept_unless_forced() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let worktree = launched(&server, &repo, "feat/dirty");
    std::fs::write(worktree.join("notes.txt"), "work in progress").unwrap();

    let output = server
        .aside_command(&["remove", "feat/dirty"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("uncommitted changes"));
    assert!(worktree.join("notes.txt").exists());
    assert_eq!(records(&server), 1);

    succeeded(
        &server
            .aside_command(&["remove", "feat/dirty", "--force"])
            .output()
            .unwrap(),
    );
    assert!(!worktree.exists());
    assert_eq!(records(&server), 0);
}

#[test]
fn keep_branch_keeps_the_local_branch_of_a_removed_launch() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let worktree = launched(&server, &repo, "feat/keep");

    succeeded(
        &server
            .aside_command(&["remove", "clone__feat-keep", "--keep-branch"])
            .output()
            .unwrap(),
    );

    assert!(!worktree.exists());
    assert!(branch_exists(&repo, "feat/keep"));
}

#[test]
fn removing_a_launch_that_works_in_the_clone_closes_its_window_and_keeps_the_clone() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let clone = repo.clone();
    let opened = server.tmux(&[
        "new-window",
        "-d",
        "-P",
        "-F",
        "#{window_id} #{pane_id}",
        "-c",
        clone.to_str().unwrap(),
    ]);
    let (window, pane) = opened.split_once(' ').unwrap();
    let directory = launches_dir(&server.home);
    std::fs::create_dir_all(&directory).unwrap();
    let record = serde_json::json!({
        "id": "clone__qa-7-1",
        "repository": clone,
        "branch": "main",
        "worktree": clone,
        "session": "main",
        "window": window,
        "pane": pane,
        "ai": "claude",
        "template": "qa",
        "task": "https://github.com/org/shop/pull/7",
        "opens_pull_request": false,
        "pull_request": null,
    });
    std::fs::write(directory.join("clone__qa-7-1.json"), record.to_string()).unwrap();
    let windows = server.window_ids().len();

    succeeded(
        &server
            .aside_command(&["remove", "clone__qa-7-1"])
            .output()
            .unwrap(),
    );

    assert!(clone.join(".git").is_dir());
    assert!(branch_exists(&repo, "main"));
    assert_eq!(records(&server), 0);
    assert_eq!(server.window_ids().len(), windows - 1);
}

#[test]
fn a_worktree_no_launch_follows_is_removed_by_its_path() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let worktree = orphan(&repo, "docs/notes");

    let report = succeeded(
        &server
            .aside_command(&["remove", worktree.to_str().unwrap()])
            .output()
            .unwrap(),
    );

    assert!(report.contains("removed"), "{report}");
    assert!(!worktree.exists());
    assert!(!branch_exists(&repo, "docs/notes"));
}

#[test]
fn prune_removes_an_orphan_worktree_whose_pull_request_merged_at_its_commit() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let worktree = orphan(&repo, "feat/merged");
    let answer = merged_at(&head(&worktree));

    let dry_run = succeeded(&aside_with_gh(
        &server,
        &repo,
        &["prune", "--dry-run", repo.dir.to_str().unwrap()],
        &answer,
    ));
    assert!(dry_run.contains("would remove"), "{dry_run}");
    assert!(worktree.exists());

    let report = succeeded(&aside_with_gh(
        &server,
        &repo,
        &["prune", repo.dir.to_str().unwrap()],
        &answer,
    ));

    assert!(report.contains("removed"), "{report}");
    assert!(!worktree.exists());
    assert!(!branch_exists(&repo, "feat/merged"));
}

#[test]
fn prune_keeps_an_orphan_whose_merged_pull_request_had_another_commit() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let worktree = orphan(&repo, "feat/reused");

    let report = succeeded(&aside_with_gh(
        &server,
        &repo,
        &["prune", repo.clone().to_str().unwrap()],
        &merged_at("0123456789abcdef0123456789abcdef01234567"),
    ));

    assert!(report.contains("nothing to prune"), "{report}");
    assert!(worktree.exists());
}

#[test]
fn prune_keeps_an_orphan_with_an_open_pull_request() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let worktree = orphan(&repo, "feat/open");
    let answer = serde_json::json!([
        {"state": "OPEN", "headRefOid": head(&worktree), "isCrossRepository": false},
        {"state": "MERGED", "headRefOid": head(&worktree), "isCrossRepository": false},
    ]);

    succeeded(&aside_with_gh(
        &server,
        &repo,
        &["prune", repo.clone().to_str().unwrap()],
        &answer.to_string(),
    ));

    assert!(worktree.exists());
}

#[test]
fn prune_reports_an_orphan_with_uncommitted_changes_and_removes_it_when_forced() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let worktree = orphan(&repo, "feat/dirty");
    std::fs::write(worktree.join("notes.txt"), "work in progress").unwrap();
    let answer = merged_at(&head(&worktree));
    let clone = repo.clone();

    let report = succeeded(&aside_with_gh(
        &server,
        &repo,
        &["prune", clone.to_str().unwrap()],
        &answer,
    ));
    assert!(report.contains("uncommitted changes"), "{report}");
    assert!(worktree.join("notes.txt").exists());

    succeeded(&aside_with_gh(
        &server,
        &repo,
        &["prune", "--force", clone.to_str().unwrap()],
        &answer,
    ));
    assert!(!worktree.exists());
}

#[test]
fn prune_never_removes_the_worktree_of_a_launch() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let worktree = launched(&server, &repo, "feat/live");

    succeeded(&aside_with_gh(
        &server,
        &repo,
        &["prune", repo.clone().to_str().unwrap()],
        &merged_at(&head(&worktree)),
    ));

    assert!(worktree.exists());
}

#[test]
fn prune_removes_an_orphan_whose_remote_branch_is_gone() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let worktree = orphan(&repo, "feat/gone");
    git_in(&worktree, &["push", "-q", "-u", "origin", "feat/gone"]);
    git_in(&repo.origin(), &["branch", "-D", "feat/gone"]);

    succeeded(&aside_with_gh(
        &server,
        &repo,
        &["prune", repo.clone().to_str().unwrap()],
        "[]",
    ));

    assert!(!worktree.exists());
}

#[test]
fn a_launch_whose_worktree_and_branch_were_deleted_by_hand_is_still_removed() {
    let server = TmuxServer::start(1);
    let repo = Repo::new();
    let worktree = launched(&server, &repo, "feat/gone-by-hand");
    git_in(
        &repo.clone(),
        &["worktree", "remove", "--force", worktree.to_str().unwrap()],
    );
    git_in(&repo.clone(), &["branch", "-D", "feat/gone-by-hand"]);

    let report = succeeded(
        &server
            .aside_command(&["remove", "clone__feat-gone-by-hand"])
            .output()
            .unwrap(),
    );

    assert_eq!(report.trim(), "removed clone__feat-gone-by-hand");
    assert_eq!(records(&server), 0);
}
