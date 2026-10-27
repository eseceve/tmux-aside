#![allow(dead_code)]

pub mod daemon;

use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

pub struct TmuxServer {
    name: String,
    socket_path: String,
    pub home: daemon::Home,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub window_id: String,
    pub pane_id: String,
    pub is_sidebar: bool,
    pub width: u16,
    pub active: bool,
}

impl TmuxServer {
    pub fn start(windows: usize) -> Self {
        Self::start_with(windows, |_| {
            format!(
                "sections:\n  - plugin: \"{}\"\n    args: [\"Fixture\", \"30\"]\n    interval: 60s\n    max_height: 10\n",
                daemon::fixture("rows")
            )
        })
    }

    /// Starts a server whose sidebar config is produced by `config` from the scratch home.
    pub fn start_with(windows: usize, config: impl Fn(&daemon::Home) -> String) -> Self {
        let name = format!(
            "tmux-aside-test-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        );
        let home = daemon::Home::new("tmux");
        home.write_config(&config(&home));
        let mut server = TmuxServer {
            name,
            socket_path: String::new(),
            home,
        };
        server.tmux(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "main",
            "-x",
            "200",
            "-y",
            "50",
        ]);
        for _ in 1..windows {
            server.tmux(&["new-window", "-t", "main"]);
        }
        server.socket_path = server.tmux(&["display-message", "-p", "#{socket_path}"]);
        server
    }

    pub fn tmux(&self, args: &[&str]) -> String {
        let output = Command::new("tmux")
            .args(["-L", &self.name])
            .args(args)
            .envs(self.home.envs())
            .output()
            .expect("failed to run tmux");
        assert!(
            output.status.success(),
            "tmux {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    pub fn aside(&self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_tmux-aside"))
            .env("TMUX", format!("{},0,0", self.socket_path))
            .env_remove("TMUX_PANE")
            .env("TMUX_ASIDE_LANG", "en")
            .envs(self.home.envs())
            .args(args)
            .output()
            .expect("failed to run tmux-aside");
        assert!(
            output.status.success(),
            "tmux-aside {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    pub fn panes(&self) -> Vec<Pane> {
        self.tmux(&[
            "list-panes",
            "-a",
            "-F",
            "#{window_id} #{pane_id} #{?@tmux_aside,1,0} #{pane_width} #{pane_active}",
        ])
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            Pane {
                window_id: fields[0].to_string(),
                pane_id: fields[1].to_string(),
                is_sidebar: fields[2] == "1",
                width: fields[3].parse().unwrap(),
                active: fields[4] == "1",
            }
        })
        .collect()
    }

    pub fn window_ids(&self) -> Vec<String> {
        self.tmux(&["list-windows", "-a", "-F", "#{window_id}"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    pub fn sidebars(&self) -> Vec<Pane> {
        self.panes()
            .into_iter()
            .filter(|pane| pane.is_sidebar)
            .collect()
    }

    pub fn capture(&self, pane_id: &str) -> String {
        self.tmux(&["capture-pane", "-p", "-t", pane_id])
    }

    /// Sends the focus-in sequence tmux emits when a pane gains focus.
    pub fn focus(&self, pane_id: &str) {
        self.tmux(&["send-keys", "-H", "-t", pane_id, "1b", "5b", "49"]);
    }

    /// Text of the line drawn in reverse video (the selection), if any.
    pub fn selected_line(&self, pane_id: &str) -> Option<String> {
        self.tmux(&["capture-pane", "-p", "-e", "-t", pane_id])
            .lines()
            .find(|line| is_reversed(line))
            .map(strip_escapes)
    }

    pub fn daemon_socket(&self) -> std::path::PathBuf {
        format!("{}-aside.sock", self.socket_path).into()
    }

    pub fn global_hooks(&self) -> String {
        format!(
            "{}\n{}",
            self.tmux(&["show-hooks", "-g"]),
            self.tmux(&["show-hooks", "-gw"])
        )
    }

    pub fn wait_until(&self, what: &str, condition: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for TmuxServer {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.name, "kill-server"])
            .output();
        for suffix in ["", "-aside.sock", "-aside.sock.lock"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.socket_path));
        }
    }
}

fn is_reversed(line: &str) -> bool {
    line.split("\x1b[").skip(1).any(|sequence| {
        sequence
            .split_once('m')
            .is_some_and(|(params, _)| params.split(';').any(|param| param == "7"))
    })
}

fn strip_escapes(line: &str) -> String {
    let mut text = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            text.push(c);
        }
    }
    text
}
