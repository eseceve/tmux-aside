use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// A temporary home holding the tmux config, tmux-aside's config and the AIs' settings.
struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tmux-aside-uninstall-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Home(std::fs::canonicalize(dir).unwrap())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn run(&self, arguments: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_tmux-aside"))
            .args(arguments)
            .arg("--config")
            .arg(self.path("tmux.conf"))
            .env_remove("TMUX")
            .env("TMUX_ASIDE_LANG", "en")
            .env("HOME", &self.0)
            .env("CLAUDE_CONFIG_DIR", self.path("claude"))
            .env("CODEX_HOME", self.path("codex"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn setup(&self, pack: &str) {
        self.run(&[
            "setup",
            "--yes",
            "--hooks",
            "claude,codex,agy",
            "--skills",
            "--pack",
            pack,
        ]);
    }

    fn read(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.path(name)).ok()
    }

    fn json(&self, name: &str) -> Option<Value> {
        self.read(name)
            .map(|text| serde_json::from_str(&text).unwrap())
    }

    fn write(&self, name: &str, content: &str) {
        let path = self.path(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_pack(dir: &Path) {
    std::fs::create_dir_all(dir.join("templates")).unwrap();
    std::fs::write(dir.join("pack.yaml"), "templates: templates\n").unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn uninstall_after_setup_leaves_every_config_as_it_was() {
    let home = Home::new();
    home.write("tmux.conf", "set -g mouse on\n");
    home.write("config/tmux-aside/config.yaml", "language: es\n");
    let settings =
        json!({"theme": "dark", "statusLine": {"type": "command", "command": "my-statusline"}});
    home.write("claude/settings.json", &settings.to_string());
    home.write("claude/skills/my-skill/SKILL.md", "mine");
    let pack = home.path("team-pack");
    write_pack(&pack);
    home.setup(pack.to_str().unwrap());
    assert!(home.read("tmux.conf").unwrap().contains("tmux-aside"));
    assert!(home.path("claude/skills/tmux-aside-launch").exists());
    let installed = home.read("claude/settings.json").unwrap();
    assert!(installed.contains("tmux-aside-agents"), "{installed}");
    assert!(installed.contains("tmux-aside-usage"), "{installed}");
    assert!(
        home.read("codex/hooks.json")
            .unwrap()
            .contains("tmux-aside-agents")
    );

    home.run(&["uninstall", "--yes"]);

    assert_eq!(home.read("tmux.conf").as_deref(), Some("set -g mouse on\n"));
    assert_eq!(
        home.read("config/tmux-aside/config.yaml").as_deref(),
        Some("language: es\n")
    );
    assert_eq!(home.json("claude/settings.json"), Some(settings));
    assert_eq!(home.json("codex/hooks.json"), None);
    assert_eq!(home.json(".gemini/config/hooks.json"), None);
    assert!(!home.path("claude/skills/tmux-aside-launch").exists());
    assert!(!home.path("codex/skills/tmux-aside-qa").exists());
    assert_eq!(
        home.read("claude/skills/my-skill/SKILL.md").as_deref(),
        Some("mine")
    );
    assert!(
        pack.join("pack.yaml").exists(),
        "a pack given by path is never deleted"
    );
}

#[test]
fn uninstall_without_a_previous_setup_does_nothing() {
    let home = Home::new();
    home.write("tmux.conf", "set -g mouse on\n");

    home.run(&["uninstall", "--yes"]);

    assert_eq!(home.read("tmux.conf").as_deref(), Some("set -g mouse on\n"));
    assert!(!home.path("config").exists());
    assert!(!home.path("claude").exists());
}

fn cloned_pack(home: &Home) -> PathBuf {
    let work = home.path("work");
    write_pack(&work);
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "pack"]);
    git(&home.0, &["clone", "-q", "--bare", "work", "team-pack.git"]);
    home.setup(&format!("file://{}", home.path("team-pack.git").display()));
    let cloned = home.path("data/tmux-aside/packs/team-pack");
    assert!(cloned.exists());
    cloned
}

#[test]
fn a_cloned_pack_is_kept_unless_the_user_agrees_to_delete_it() {
    let home = Home::new();
    let cloned = cloned_pack(&home);

    home.run(&["uninstall", "--yes"]);

    assert!(cloned.exists());
    assert!(
        !home
            .read("config/tmux-aside/config.yaml")
            .unwrap_or_default()
            .contains("pack:")
    );
}

#[test]
fn a_cloned_pack_is_deleted_when_the_user_agrees() {
    let home = Home::new();
    let cloned = cloned_pack(&home);

    home.run(&["uninstall", "--yes", "--delete-pack"]);

    assert!(!cloned.exists());
}

#[test]
fn a_pack_given_by_path_is_never_deleted() {
    let home = Home::new();
    let pack = home.path("team-pack");
    write_pack(&pack);
    home.setup(pack.to_str().unwrap());

    home.run(&["uninstall", "--yes", "--delete-pack"]);

    assert!(pack.join("pack.yaml").exists());
}

#[test]
fn a_pack_key_that_climbs_out_of_the_packs_folder_deletes_nothing() {
    let home = Home::new();
    cloned_pack(&home);
    home.write(
        "config/tmux-aside/config.yaml",
        &format!(
            "pack: \"{}\"\n",
            home.path("data/tmux-aside/packs/team-pack/../..").display()
        ),
    );

    home.run(&["uninstall", "--yes", "--delete-pack"]);

    assert!(
        home.path("data/tmux-aside/packs/team-pack/pack.yaml")
            .exists()
    );
}

#[test]
fn uninstall_keeps_the_backups_setup_left_and_can_run_twice() {
    let home = Home::new();
    home.write("tmux.conf", "set -g mouse on\n");
    let pack = home.path("team-pack");
    write_pack(&pack);
    home.setup(pack.to_str().unwrap());

    home.run(&["uninstall", "--yes"]);
    home.run(&["uninstall", "--yes"]);

    assert_eq!(
        home.read("tmux.conf.bak").as_deref(),
        Some("set -g mouse on\n")
    );
    assert_eq!(home.read("tmux.conf").as_deref(), Some("set -g mouse on\n"));
}
