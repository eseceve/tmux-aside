#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use tmux_aside::protocol::PluginMessage;
use tmux_aside_agents::AgentState;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// Isolated tmux server whose first pane runs `sleep` inside a git repository named `repo`.
pub struct Tmux {
    name: String,
    socket: String,
    pub dir: PathBuf,
}

impl Tmux {
    pub fn start() -> Self {
        let id = format!(
            "{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        );
        let dir = std::env::temp_dir().join(format!("tmux-aside-agents-{id}"));
        std::fs::create_dir_all(dir.join("repo/src")).unwrap();
        let init = Command::new("git")
            .args(["init", "-q", "-b", "feature/sidebar"])
            .arg(dir.join("repo"))
            .status()
            .unwrap();
        assert!(init.success());
        let commit = Command::new("git")
            .arg("-C")
            .arg(dir.join("repo"))
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ])
            .status()
            .unwrap();
        assert!(commit.success());
        let mut tmux = Tmux {
            name: format!("tmux-aside-agents-{id}"),
            socket: String::new(),
            dir,
        };
        let repo = tmux.dir.join("repo/src");
        tmux.tmux(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "main",
            "-n",
            "work",
            "-c",
            repo.to_str().unwrap(),
            "sleep 1000",
        ]);
        tmux.socket = tmux.tmux(&["display-message", "-p", "#{socket_path}"]);
        tmux.fake_gh("[]");
        tmux
    }

    pub fn tmux(&self, args: &[&str]) -> String {
        let output = Command::new("tmux")
            .args(["-L", &self.name])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "tmux {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    pub fn split(&self) -> String {
        let repo = self.dir.join("repo/src");
        self.tmux(&[
            "split-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            "main",
            "-c",
            repo.to_str().unwrap(),
            "sleep 1000",
        ])
    }

    /// Puts a fake `gh` first on the plugin's PATH that prints `pr_list_json`.
    pub fn fake_gh(&self, pr_list_json: &str) {
        let bin = self.dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = bin.join("gh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ncat <<'JSON'\n{pr_list_json}\nJSON\n"),
        )
        .unwrap();
        Command::new("chmod")
            .arg("+x")
            .arg(&script)
            .status()
            .unwrap();
    }

    pub fn agents(&self, pane: Option<&str>) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tmux-aside-agents"));
        command
            .env("TMUX", format!("{},0,0", self.socket))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("TMUX_ASIDE_LANG", "en")
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.dir.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env_remove("TMUX_PANE");
        if let Some(pane) = pane {
            command.env("TMUX_PANE", pane);
        }
        command
    }

    pub fn run(&self, pane: Option<&str>, args: &[&str]) -> Output {
        self.agents(pane)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    pub fn state_file(&self, pane: &str) -> PathBuf {
        self.dir
            .join("state/tmux-aside/agents")
            .join(format!("{}.json", pane.trim_start_matches('%')))
    }

    pub fn state(&self, pane: &str) -> AgentState {
        serde_json::from_str(&std::fs::read_to_string(self.state_file(pane)).unwrap()).unwrap()
    }

    pub fn write_state(&self, pane: &str, state: &AgentState) {
        std::fs::write(self.state_file(pane), serde_json::to_string(state).unwrap()).unwrap();
    }
}

impl Drop for Tmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.name, "kill-server"])
            .output();
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The agents binary running as a stream plugin.
pub struct Plugin {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    last_rows: Vec<tmux_aside::protocol::Row>,
}

impl Plugin {
    pub fn start(tmux: &Tmux, args: &[&str]) -> Self {
        let mut child = tmux
            .agents(None)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, r#"{{"type":"hello","protocol":1,"features":[]}}"#).unwrap();
        let stdout: ChildStdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Plugin {
            child,
            stdin: Some(stdin),
            lines,
            last_rows: Vec::new(),
        }
    }

    pub fn next(&mut self) -> PluginMessage {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(5))
            .expect("no output from the plugin within 5 seconds");
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("invalid line: {line:?}"))
    }

    /// Reads snapshots until one's rows satisfy `condition`, failing after 5 seconds.
    pub fn snapshot_where(
        &mut self,
        what: &str,
        condition: impl Fn(&[tmux_aside::protocol::Row]) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let Ok(line) = self.lines.recv_timeout(remaining) else {
                panic!(
                    "timed out waiting for {what}; last rows: {:?}",
                    self.last_rows
                );
            };
            if let Ok(PluginMessage::Snapshot { rows, .. }) = serde_json::from_str(&line) {
                if condition(&rows) {
                    return;
                }
                self.last_rows = rows;
            }
        }
    }

    pub fn close_stdin(&mut self) {
        self.stdin.take();
    }

    pub fn exited_within(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.child.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn wait_for(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn exists(path: &Path) -> bool {
    path.exists()
}
