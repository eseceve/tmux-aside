use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

struct AgentDir {
    dir: PathBuf,
    agent: &'static str,
    env: &'static str,
    file: &'static str,
}

impl AgentDir {
    fn claude() -> Self {
        Self::new("claude", "CLAUDE_CONFIG_DIR", "settings.json")
    }

    fn codex() -> Self {
        Self::new("codex", "CODEX_HOME", "hooks.json")
    }

    fn new(agent: &'static str, env: &'static str, file: &'static str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tmux-aside-{agent}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        AgentDir {
            dir,
            agent,
            env,
            file,
        }
    }

    fn settings(&self) -> PathBuf {
        self.dir.join(self.file)
    }

    fn write(&self, value: &Value) {
        std::fs::write(
            self.settings(),
            serde_json::to_string_pretty(value).unwrap(),
        )
        .unwrap();
    }

    fn read(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.settings()).unwrap()).unwrap()
    }

    fn run(&self) -> Output {
        Command::new(exe())
            .args(["setup", self.agent])
            .env(self.env, &self.dir)
            .output()
            .unwrap()
    }

    fn setup(&self) -> String {
        let output = self.run();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}

impl Drop for AgentDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn commands(settings: &Value, event: &str) -> Vec<String> {
    settings["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| entry["hooks"].as_array().cloned().unwrap_or_default())
        .filter_map(|hook| hook["command"].as_str().map(str::to_string))
        .collect()
}

fn exe() -> &'static str {
    env!("CARGO_BIN_EXE_tmux-aside-agents")
}

#[test]
fn creates_settings_with_every_status_hook() {
    let claude = AgentDir::claude();

    claude.setup();

    let settings = claude.read();
    let expected = [
        ("SessionStart", "register"),
        ("UserPromptSubmit", "set-status working"),
        ("PermissionRequest", "set-status waiting"),
        ("Notification", "set-status waiting"),
        ("PostToolUse", "set-status working"),
        ("Stop", "set-status done"),
    ];
    for (event, arguments) in expected {
        assert_eq!(
            commands(&settings, event),
            [format!("'{}' {arguments}", exe())],
            "{event}"
        );
    }
    assert_eq!(
        settings["hooks"]["Notification"][0]["matcher"],
        "permission_prompt|elicitation_dialog"
    );
    assert!(!claude.settings().with_extension("json.bak").exists());
}

#[test]
fn keeps_existing_keys_and_foreign_hooks_and_backs_up_the_file() {
    let claude = AgentDir::claude();
    let original = json!({
        "model": "opus",
        "hooks": {
            "Stop": [{ "hooks": [{ "type": "command", "command": "workmux set-window-status done" }] }]
        },
        "theme": "dark"
    });
    claude.write(&original);

    claude.setup();

    let settings = claude.read();
    assert_eq!(settings["model"], "opus");
    assert_eq!(settings["theme"], "dark");
    assert_eq!(
        commands(&settings, "Stop"),
        [
            "workmux set-window-status done".to_string(),
            format!("'{}' set-status done", exe())
        ]
    );
    let keys: Vec<&String> = settings.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["model", "hooks", "theme"]);
    let backup: Value = serde_json::from_str(
        &std::fs::read_to_string(claude.settings().with_extension("json.bak")).unwrap(),
    )
    .unwrap();
    assert_eq!(backup, original);
}

#[test]
fn running_setup_twice_changes_nothing() {
    let claude = AgentDir::claude();
    claude.setup();
    let first = std::fs::read_to_string(claude.settings()).unwrap();

    claude.setup();

    assert_eq!(std::fs::read_to_string(claude.settings()).unwrap(), first);
}

#[test]
fn a_hook_pointing_at_an_old_binary_path_is_moved_instead_of_duplicated() {
    let claude = AgentDir::claude();
    claude.write(&json!({
        "hooks": {
            "Stop": [{ "hooks": [{ "type": "command", "command": "'/old/bin/tmux-aside-agents' set-status done" }] }]
        }
    }));

    claude.setup();

    assert_eq!(
        commands(&claude.read(), "Stop"),
        [format!("'{}' set-status done", exe())]
    );
}

#[test]
fn invalid_json_is_left_untouched() {
    let claude = AgentDir::claude();
    std::fs::write(claude.settings(), "{ not json").unwrap();

    let output = claude.run();

    assert!(!output.status.success());
    assert_eq!(
        std::fs::read_to_string(claude.settings()).unwrap(),
        "{ not json"
    );
    assert!(!Path::new(&claude.settings().with_extension("json.bak")).exists());
}

#[test]
fn codex_hooks_cover_every_status_and_ask_to_be_trusted() {
    let codex = AgentDir::codex();

    let stdout = codex.setup();

    let settings = codex.read();
    let expected = [
        ("SessionStart", "register"),
        ("UserPromptSubmit", "set-status working"),
        ("PermissionRequest", "set-status waiting"),
        ("PostToolUse", "set-status working"),
        ("Stop", "set-status done"),
    ];
    for (event, arguments) in expected {
        assert_eq!(
            commands(&settings, event),
            [format!("'{}' {arguments}", exe())],
            "{event}"
        );
    }
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["matcher"],
        "startup|resume|clear|fork"
    );
    assert!(stdout.contains("/hooks"), "{stdout}");
}

#[test]
fn codex_hooks_are_added_after_the_existing_ones() {
    let codex = AgentDir::codex();
    codex.write(&json!({
        "hooks": {
            "Stop": [{ "hooks": [{ "type": "command", "command": "workmux set-window-status done" }] }]
        }
    }));

    codex.setup();
    let first = std::fs::read_to_string(codex.settings()).unwrap();
    codex.setup();

    assert_eq!(std::fs::read_to_string(codex.settings()).unwrap(), first);
    assert_eq!(
        commands(&codex.read(), "Stop"),
        [
            "workmux set-window-status done".to_string(),
            format!("'{}' set-status done", exe())
        ]
    );
}
