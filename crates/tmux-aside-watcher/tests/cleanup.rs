mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::{Plugin, Scratch, Tmux, found, launch, node};
use tmux_aside::launch::Launch;
use tmux_aside::protocol::{Row, Tone};

/// A launch of `feat/x` with a real worktree and its agent in a window of a private tmux
/// server, followed by a Watcher.
struct Setup {
    scratch: Scratch,
    tmux: Tmux,
    worktree: PathBuf,
    window: String,
}

impl Setup {
    fn new() -> Self {
        let scratch = Scratch::new();
        let tmux = Tmux::start();
        let shop = scratch.repository("shop");
        commit(&shop, "first");
        let launch = launch(&shop, "feat/x");
        support::git(
            &shop,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/x",
                launch.worktree.to_str().unwrap(),
            ],
        );
        let pane = tmux.agent_pane(&scratch.0.join("typed"), &launch.worktree);
        let window = tmux.run(&["display-message", "-p", "-t", &pane, "#{window_id}"]);
        scratch.save(&Launch {
            pane,
            window: window.clone(),
            ..launch.clone()
        });
        Setup {
            scratch,
            tmux,
            worktree: launch.worktree,
            window,
        }
    }

    /// Records the pull request in the Launch, as the Watcher does once it sees it open.
    fn record_pull_request(&self, url: &str) {
        let mut launch = self.scratch.recorded("shop__feat-x");
        launch.pull_request = Some(url.to_string());
        self.scratch.save(&launch);
    }

    fn head(&self) -> String {
        git_output(&self.worktree, &["rev-parse", "HEAD"])
    }

    fn respond(&self, pull_request: Value) {
        self.scratch.respond(found(&[Some(pull_request)]));
    }

    fn start(&self) -> Plugin {
        self.scratch.start_in("200ms", Some(&self.tmux))
    }

    fn window_open(&self) -> bool {
        self.tmux
            .run(&["list-windows", "-a", "-F", "#{window_id}"])
            .lines()
            .any(|window| window == self.window)
    }

    /// Waits until the Watcher fetched `more` times from now.
    fn wait_for_fetches(&self, more: usize) {
        let target = self.scratch.calls().len() + more;
        wait_until(&format!("{more} more fetches"), || {
            self.scratch.calls().len() >= target
        });
    }
}

fn commit(dir: &Path, message: &str) {
    support::git(
        dir,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            message,
        ],
    );
}

fn git_output(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn line(rows: &[Row]) -> &str {
    rows.first()
        .and_then(|row| row.lines.first())
        .map_or("", String::as_str)
}

#[test]
fn a_merged_pull_request_with_a_clean_worktree_removes_it_closes_its_window_and_enters_qa() {
    let setup = Setup::new();
    setup.respond(node(json!({"state": "MERGED", "headRefOid": setup.head()})));

    let plugin = setup.start();

    let rows = plugin.rows_where("the cleanup", |rows| line(rows) == "QA · waiting for QA");
    assert!(!setup.worktree.exists());
    assert!(!setup.window_open());
    assert_eq!(rows[0].bar_tone, Tone::Accent);
}

#[test]
fn a_closed_pull_request_is_cleaned_up_the_same_way_and_leaves() {
    let setup = Setup::new();
    setup.respond(node(json!({"state": "CLOSED", "headRefOid": setup.head()})));

    let plugin = setup.start();

    plugin.rows_where("the row to leave", |rows| rows.is_empty());
    assert!(!setup.worktree.exists());
    assert!(!setup.window_open());
}

#[test]
fn uncommitted_changes_keep_the_worktree_and_warn() {
    let setup = Setup::new();
    std::fs::write(setup.worktree.join("notes.txt"), "draft").unwrap();
    setup.respond(node(json!({"state": "MERGED", "headRefOid": setup.head()})));

    let plugin = setup.start();

    let rows = plugin.rows_where("the warning", |rows| {
        line(rows) == "QA · uncommitted changes kept in the worktree"
    });
    assert_eq!(rows[0].bar_tone, Tone::Warning);
    assert!(setup.worktree.exists());
    assert!(setup.window_open());
}

#[test]
fn unpushed_commits_keep_the_worktree_and_warn() {
    let setup = Setup::new();
    let pushed = setup.head();
    commit(&setup.worktree, "not pushed");
    setup.record_pull_request("https://github.com/org/shop/pull/12");
    setup.respond(node(json!({"state": "MERGED", "headRefOid": pushed})));

    let plugin = setup.start();

    let rows = plugin.rows_where("the warning", |rows| {
        line(rows) == "QA · unpushed commits kept in the worktree"
    });
    assert_eq!(rows[0].bar_tone, Tone::Warning);
    assert!(setup.worktree.exists());
    assert!(setup.window_open());
}

#[test]
fn nothing_is_removed_while_the_pull_request_is_open_even_when_ready_to_merge() {
    let setup = Setup::new();
    setup.respond(node(json!({"state": "OPEN", "headRefOid": setup.head()})));

    let plugin = setup.start();
    plugin.rows_where("ready to merge", |rows| {
        line(rows).starts_with("Ready to merge")
    });
    setup.wait_for_fetches(3);

    assert!(setup.worktree.exists());
    assert!(setup.window_open());
}

#[test]
fn an_old_merged_pull_request_of_a_reused_branch_name_removes_nothing() {
    let setup = Setup::new();
    setup.respond(node(
        json!({"state": "MERGED", "headRefOid": "an-old-commit"}),
    ));

    let plugin = setup.start();
    plugin.rows_where("no pull request", |rows| {
        line(rows) == "Implementing · no pull request yet"
    });
    setup.wait_for_fetches(3);

    assert!(setup.worktree.exists());
    assert!(setup.window_open());
}

#[test]
fn a_worktree_that_is_not_tmux_asides_is_never_removed() {
    let setup = Setup::new();
    let mut launch = setup.scratch.recorded("shop__feat-x");
    launch.keeps_worktree = true;
    launch.pull_request = Some("https://github.com/org/shop/pull/12".to_string());
    setup.scratch.save(&launch);
    setup.respond(node(json!({"state": "MERGED", "headRefOid": setup.head()})));

    let plugin = setup.start();
    plugin.rows_where("QA", |rows| line(rows) == "QA · waiting for QA");
    setup.wait_for_fetches(3);

    assert!(setup.worktree.exists());
    assert!(setup.window_open());
}
