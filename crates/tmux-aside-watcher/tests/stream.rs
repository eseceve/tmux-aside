use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tmux_aside::launch::Launch;
use tmux_aside::protocol::{PluginMessage, Row};
use tmux_aside::setup::Ai;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// A scratch directory with a state home, a fake `gh` that logs every call and answers with
/// the contents of `response`, and git clones whose origin is on GitHub.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tmux-aside-watcher-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let gh = dir.join("bin/gh");
        std::fs::write(
            &gh,
            "#!/bin/sh\ndir=$(dirname \"$0\")/..\nprintf '%s\\n' \"$*\" >> \"$dir/calls\"\ncat \"$dir/response\"\n",
        )
        .unwrap();
        Command::new("chmod").arg("+x").arg(&gh).status().unwrap();
        let scratch = Scratch(dir);
        scratch.respond(json!({"data": {}}));
        scratch
    }

    fn respond(&self, response: Value) {
        let path = self.0.join("response");
        std::fs::write(path.with_extension("tmp"), response.to_string()).unwrap();
        std::fs::rename(path.with_extension("tmp"), path).unwrap();
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.0.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn repository(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q"]);
        git(
            &path,
            &[
                "remote",
                "add",
                "origin",
                &format!("git@github.com:org/{name}.git"),
            ],
        );
        path
    }

    fn record(&self, repository: &Path, branch: &str, opens_pull_request: bool) {
        let name = repository.file_name().unwrap().to_str().unwrap();
        let handle = branch.replace('/', "-");
        let launch = Launch {
            id: format!("{name}__{handle}"),
            repository: repository.to_path_buf(),
            branch: branch.to_string(),
            worktree: repository.with_file_name(format!("{name}__worktrees/{handle}")),
            session: "main".to_string(),
            window: "@1".to_string(),
            pane: "%1".to_string(),
            ai: Ai::Claude,
            template: "implement".to_string(),
            task: format!("Task of {branch}"),
            opens_pull_request,
            pull_request: None,
        };
        let dir = self.0.join("state/tmux-aside/launches");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{}.json", launch.id)),
            serde_json::to_string(&launch).unwrap(),
        )
        .unwrap();
    }

    fn recorded(&self, id: &str) -> Launch {
        let path = self.0.join(format!("state/tmux-aside/launches/{id}.json"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn start(&self, interval: &str) -> Plugin {
        let path = std::env::var("PATH").unwrap_or_default();
        let mut child = Command::new(env!("CARGO_BIN_EXE_tmux-aside-watcher"))
            .args(["--interval", interval])
            .env("PATH", format!("{}:{path}", self.0.join("bin").display()))
            .env("XDG_STATE_HOME", self.0.join("state"))
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("TMUX_ASIDE_LANG", "en")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, r#"{{"type":"hello","protocol":1,"features":[]}}"#).unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Plugin {
            child,
            _stdin: stdin,
            lines,
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success());
}

struct Plugin {
    child: Child,
    _stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Plugin {
    /// Reads snapshots until one satisfies `condition`, failing after 5 seconds.
    fn rows_where(&self, what: &str, condition: impl Fn(&[Row]) -> bool) -> Vec<Row> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut last = None;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            let Ok(line) = self.lines.recv_timeout(left) else {
                break;
            };
            if let Ok(PluginMessage::Snapshot { rows, .. }) = serde_json::from_str(&line) {
                if condition(&rows) {
                    return rows;
                }
                last = Some(rows);
            }
        }
        panic!("timed out waiting for {what}; last rows: {last:?}");
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pull_request(number: u64, branch_ready: bool) -> Value {
    json!({
        "number": number,
        "title": format!("PR {number}"),
        "url": format!("https://github.com/org/shop/pull/{number}"),
        "isDraft": false,
        "reviewDecision": if branch_ready { "APPROVED" } else { "REVIEW_REQUIRED" },
        "mergeable": "MERGEABLE",
        "createdAt": "2026-11-10T10:00:00Z",
        "updatedAt": "2026-11-11T10:00:00Z",
        "author": {"login": "octocat"},
        "repository": {"name": "shop", "nameWithOwner": "org/shop"},
        "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "SUCCESS"}}}]}
    })
}

fn found(pull_requests: &[Option<Value>]) -> Value {
    let data: serde_json::Map<String, Value> = pull_requests
        .iter()
        .enumerate()
        .map(|(index, pr)| {
            let nodes: Vec<Value> = pr.iter().cloned().collect();
            (
                format!("p{index}"),
                json!({"pullRequests": {"nodes": nodes}}),
            )
        })
        .collect();
    json!({ "data": data })
}

fn stage(row: &Row) -> &str {
    row.lines[0].split(" · ").next().unwrap()
}

#[test]
fn a_launched_pull_request_shows_its_stage() {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    scratch.record(&shop, "feat/ready", true);
    scratch.respond(found(&[Some(pull_request(12, true))]));

    let plugin = scratch.start("30s");

    let rows = plugin.rows_where("a ready row", |rows| {
        rows.len() == 1 && stage(&rows[0]) == "Ready to merge"
    });
    assert_eq!(rows[0].text, "PR 12");
    assert_eq!(rows[0].detail.as_deref(), Some("#12"));
}

#[test]
fn a_launch_without_a_pull_request_is_implementing_until_its_branch_has_one() {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    scratch.record(&shop, "feat/new", true);
    scratch.respond(found(&[None]));
    let plugin = scratch.start("200ms");
    let rows = plugin.rows_where("an implementing row", |rows| rows.len() == 1);
    assert_eq!(rows[0].lines, ["Implementing · no pull request yet"]);
    assert_eq!(rows[0].text, "Task of feat/new");

    scratch.respond(found(&[Some(pull_request(13, false))]));

    let rows = plugin.rows_where("the pull request", |rows| {
        rows[0].detail.as_deref() == Some("#13")
    });
    assert_eq!(rows[0].lines, ["Implementing · waiting for approval"]);
    assert_eq!(
        scratch.recorded("shop__feat-new").pull_request.as_deref(),
        Some("https://github.com/org/shop/pull/13"),
        "the registry learns the pull request"
    );
}

#[test]
fn launches_that_open_no_pull_request_are_not_shown() {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    scratch.record(&shop, "research/cache", false);
    scratch.record(&shop, "feat/shown", true);
    scratch.respond(found(&[None]));

    let plugin = scratch.start("30s");

    let rows = plugin.rows_where("one row", |rows| !rows.is_empty());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text, "Task of feat/shown");
}

#[test]
fn every_pull_request_is_fetched_in_one_query_per_poll() {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    let api = scratch.repository("api");
    scratch.record(&shop, "feat/one", true);
    scratch.record(&api, "feat/two", true);
    scratch.respond(found(&[None, None]));

    let plugin = scratch.start("30s");
    plugin.rows_where("two rows", |rows| rows.len() == 2);

    let calls = scratch.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    for expected in ["n0=api", "b0=feat/two", "n1=shop", "b1=feat/one"] {
        assert!(
            calls[0].contains(expected),
            "{expected} missing: {}",
            calls[0]
        );
    }
}

#[test]
fn a_pull_request_that_stops_being_ready_falls_back_to_implementing() {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    scratch.record(&shop, "feat/flaky", true);
    scratch.respond(found(&[Some(pull_request(14, true))]));
    let plugin = scratch.start("200ms");
    plugin.rows_where("ready", |rows| {
        rows.len() == 1 && stage(&rows[0]) == "Ready to merge"
    });

    scratch.respond(found(&[Some(pull_request(14, false))]));

    plugin.rows_where("implementing again", |rows| {
        stage(&rows[0]) == "Implementing"
    });
}
