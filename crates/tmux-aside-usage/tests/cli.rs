use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use tmux_aside::protocol::PluginMessage;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tmux-aside-usage-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::create_dir_all(dir.join("state/tmux-aside")).unwrap();
        std::fs::create_dir_all(dir.join("claude")).unwrap();
        Home(dir)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tmux-aside-usage"));
        command
            .args(args)
            .env("TMUX_ASIDE_LANG", "en")
            .env("XDG_STATE_HOME", self.0.join("state"))
            .env("CLAUDE_CONFIG_DIR", self.0.join("claude"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.0.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            );
        command
    }

    fn run_with_stdin(&self, args: &[&str], stdin: &str) -> Output {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn state(&self, name: &str) -> PathBuf {
        self.0.join("state/tmux-aside").join(name)
    }

    fn fake_codex(&self, used_percent: u64) {
        let script = self.0.join("bin/codex");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho called >> \"$(dirname \"$0\")/codex-calls\"\nwhile read line; do\n  case \"$line\" in\n    *'\"id\":1'*) echo '{{\"id\":1,\"result\":{{}}}}' ;;\n    *'\"id\":2'*) echo '{{\"id\":2,\"result\":{{\"rateLimits\":{{\"primary\":{{\"usedPercent\":{used_percent},\"windowDurationMins\":10080,\"resetsAt\":9999999999}},\"secondary\":null}}}}}}' ;;\n  esac\ndone\n"
            ),
        )
        .unwrap();
        Command::new("chmod")
            .arg("+x")
            .arg(&script)
            .status()
            .unwrap();
    }

    fn codex_calls(&self) -> usize {
        std::fs::read_to_string(self.0.join("bin/codex-calls"))
            .map(|calls| calls.lines().count())
            .unwrap_or(0)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn snapshot_rows(output: &Output) -> Vec<tmux_aside::protocol::Row> {
    let messages: Vec<PluginMessage> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    match &messages[1] {
        PluginMessage::Snapshot { rows, .. } => rows.clone(),
        _ => panic!("expected a snapshot"),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn the_statusline_wrapper_records_claude_limits_and_passes_its_input_through() {
    let home = Home::new();
    let input = json!({"model": {"id": "x"}, "rate_limits": {"five_hour": {"used_percentage": 40, "resets_at": now() + 600}}}).to_string();

    let output = home.run_with_stdin(&["statusline", "cat"], &input);

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), input);
    let cached: Value =
        serde_json::from_str(&std::fs::read_to_string(home.state("claude-limits.json")).unwrap())
            .unwrap();
    assert_eq!(cached["rate_limits"]["five_hour"]["used_percentage"], 40);
}

#[test]
fn the_statusline_wrapper_keeps_the_last_limits_when_the_input_has_none() {
    let home = Home::new();
    std::fs::write(
        home.state("claude-limits.json"),
        r#"{"rate_limits":{"keep":true}}"#,
    )
    .unwrap();

    home.run_with_stdin(&["statusline", "cat"], r#"{"model":{}}"#);

    assert!(
        std::fs::read_to_string(home.state("claude-limits.json"))
            .unwrap()
            .contains("keep")
    );
}

#[test]
fn the_plugin_shows_claude_and_codex_windows() {
    let home = Home::new();
    let resets = now() + 3_600;
    std::fs::write(
        home.state("claude-limits.json"),
        json!({"rate_limits": {"five_hour": {"used_percentage": 40, "resets_at": resets}}})
            .to_string(),
    )
    .unwrap();
    home.fake_codex(25);

    let rows = snapshot_rows(&home.command(&[]).output().unwrap());

    let summary: Vec<(String, String, String)> = rows
        .iter()
        .map(|row| {
            (
                row.group.clone().unwrap(),
                row.text.trim_end().to_string(),
                row.detail
                    .clone()
                    .unwrap_or_default()
                    .trim_start()
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(summary[0].0, "Claude");
    assert!(summary[0].2.starts_with("40% · "), "{summary:?}");
    assert_eq!(
        (summary[1].0.as_str(), summary[1].1.as_str()),
        ("Codex", "Weekly")
    );
    assert!(summary[1].2.starts_with("25% · "), "{summary:?}");
}

#[test]
fn codex_limits_are_cached_for_the_configured_interval() {
    let home = Home::new();
    home.fake_codex(25);

    home.command(&["--codex-interval", "10m"]).output().unwrap();
    home.command(&["--codex-interval", "10m"]).output().unwrap();

    assert_eq!(home.codex_calls(), 1);
}

#[test]
fn setup_wraps_the_claude_statusline_once_and_keeps_the_rest() {
    let home = Home::new();
    let settings = home.0.join("claude/settings.json");
    std::fs::write(
        &settings,
        json!({"model": "opus", "statusLine": {"type": "command", "command": "node powerline"}})
            .to_string(),
    )
    .unwrap();

    assert!(
        home.command(&["setup", "claude"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        home.command(&["setup", "claude"])
            .status()
            .unwrap()
            .success()
    );

    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let command = written["statusLine"]["command"].as_str().unwrap();
    assert_eq!(
        tmux_aside_usage::wrapped_statusline(command).as_deref(),
        Some("node powerline")
    );
    assert_eq!(written["model"], "opus");
    assert!(settings.with_extension("json.bak").exists());
}
