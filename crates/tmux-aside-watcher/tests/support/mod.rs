#![allow(dead_code)]

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

pub static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// A scratch directory with a state home, a fake `gh` that logs every call and answers with
/// the contents of `response`, and git clones whose origin is on GitHub.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new() -> Self {
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

    pub fn respond(&self, response: Value) {
        let path = self.0.join("response");
        std::fs::write(path.with_extension("tmp"), response.to_string()).unwrap();
        std::fs::rename(path.with_extension("tmp"), path).unwrap();
    }

    pub fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.0.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    pub fn repository(&self, name: &str) -> PathBuf {
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

    pub fn record(&self, repository: &Path, branch: &str, opens_pull_request: bool) {
        self.save(&Launch {
            opens_pull_request,
            ..launch(repository, branch)
        });
    }

    pub fn save(&self, launch: &Launch) {
        let dir = self.0.join("state/tmux-aside/launches");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{}.json", launch.id)),
            serde_json::to_string(launch).unwrap(),
        )
        .unwrap();
    }

    pub fn recorded(&self, id: &str) -> Launch {
        let path = self.0.join(format!("state/tmux-aside/launches/{id}.json"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    pub fn start(&self, interval: &str) -> Plugin {
        self.start_in(interval, None)
    }

    /// Starts the Watcher inside the tmux server `tmux`, when given.
    pub fn start_in(&self, interval: &str, tmux: Option<&Tmux>) -> Plugin {
        let path = std::env::var("PATH").unwrap_or_default();
        let mut command = Command::new(env!("CARGO_BIN_EXE_tmux-aside-watcher"));
        match tmux {
            Some(tmux) => command.env("TMUX", format!("{},0,0", tmux.socket)),
            None => command.env_remove("TMUX"),
        };
        let mut child = command
            .env_remove("TMUX_PANE")
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

pub fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success());
}

pub struct Plugin {
    child: Child,
    _stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Plugin {
    /// Reads snapshots until one satisfies `condition`, failing after 5 seconds.
    pub fn rows_where(&self, what: &str, condition: impl Fn(&[Row]) -> bool) -> Vec<Row> {
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

/// A pull request as GitHub answers it: number 12 in `org/shop`, approved, mergeable, with
/// passing checks and head commit `aaa`, with `changes` applied on top.
pub fn node(changes: Value) -> Value {
    let mut node = json!({
        "number": 12,
        "title": "Add a dark theme",
        "url": "https://github.com/org/shop/pull/12",
        "headRefOid": "aaa",
        "isDraft": false,
        "reviewDecision": "APPROVED",
        "mergeable": "MERGEABLE",
        "createdAt": "2026-11-10T10:00:00Z",
        "updatedAt": "2026-11-11T10:00:00Z",
        "author": {"login": "octocat"},
        "repository": {"name": "shop", "nameWithOwner": "org/shop"},
        "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "SUCCESS"}}}]}
    });
    for (key, value) in changes.as_object().unwrap() {
        node[key] = value.clone();
    }
    node
}

/// A Launch of `branch` in `repository`, whose Task reference is `Task of <branch>`.
pub fn launch(repository: &Path, branch: &str) -> Launch {
    let name = repository.file_name().unwrap().to_str().unwrap();
    let handle = branch.replace('/', "-");
    Launch {
        id: format!("{name}__{handle}"),
        repository: repository.to_path_buf(),
        branch: branch.to_string(),
        worktree: repository.with_file_name(format!("{name}__worktrees/{handle}")),
        session: "main".to_string(),
        window: "@1".to_string(),
        pane: "%1".to_string(),
        ai: Ai::Claude,
        template: Some("implement".to_string()),
        task: format!("Task of {branch}"),
        opens_pull_request: true,
        pull_request: None,
        nudges: Vec::new(),
    }
}

pub fn found(pull_requests: &[Option<Value>]) -> Value {
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

pub fn stage(row: &Row) -> &str {
    row.lines[0].split(" · ").next().unwrap()
}

/// A private tmux server whose panes stand in for agents: each one writes the lines typed
/// into it to a file.
pub struct Tmux {
    name: String,
    pub socket: String,
}

impl Tmux {
    pub fn start() -> Self {
        let name = format!(
            "tmux-aside-watcher-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        );
        let mut tmux = Tmux {
            name,
            socket: String::new(),
        };
        tmux.run(&["-f", "/dev/null", "new-session", "-d", "-s", "main"]);
        tmux.socket = tmux.run(&["display-message", "-p", "#{socket_path}"]);
        tmux
    }

    pub fn run(&self, args: &[&str]) -> String {
        let output = Command::new("tmux")
            .args(["-L", &self.name])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "tmux {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Opens a pane in `dir` whose agent writes every line typed into it to `typed`, and
    /// returns its id.
    pub fn agent_pane(&self, typed: &Path, dir: &Path) -> String {
        let pane = self.run(&[
            "new-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-c",
            dir.to_str().unwrap(),
            &format!("exec cat > '{}'", typed.display()),
        ]);
        let command = || {
            self.run(&[
                "display-message",
                "-p",
                "-t",
                &pane,
                "#{pane_current_command}",
            ])
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while command() != "cat" {
            assert!(Instant::now() < deadline, "the agent pane did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        pane
    }

    /// Writes the agent state file of `pane` in `scratch`, as the agents' hooks would.
    pub fn report(&self, scratch: &Scratch, pane: &str, status: &str) {
        let pid = self.run(&["display-message", "-p", "-t", pane, "#{pane_pid}"]);
        let dir = scratch.0.join("state/tmux-aside/agents");
        std::fs::create_dir_all(&dir).unwrap();
        let state = json!({
            "pane_id": pane,
            "pane_pid": pid.parse::<u32>().unwrap(),
            "command": "cat",
            "project": "shop",
            "window": "agent",
            "status": status,
            "status_ts": 0,
            "activity_ts": 0
        });
        std::fs::write(
            dir.join(format!("{}.json", pane.trim_start_matches('%'))),
            state.to_string(),
        )
        .unwrap();
    }
}

impl Drop for Tmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.name, "kill-server"])
            .output();
    }
}
