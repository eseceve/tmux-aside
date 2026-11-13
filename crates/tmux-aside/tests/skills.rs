use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use tmux_aside::setup::SKILLS;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// A temporary home with Claude Code's and Codex's config folders inside.
struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tmux-aside-skills-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Home(dir)
    }

    fn claude_skills(&self) -> PathBuf {
        self.0.join("claude/skills")
    }

    fn codex_skills(&self) -> PathBuf {
        self.0.join("codex/skills")
    }

    fn setup(&self, arguments: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_tmux-aside"))
            .args(["setup", "--yes", "--next", "-", "--config"])
            .arg(self.0.join("tmux.conf"))
            .args(arguments)
            .env_remove("TMUX")
            .env("TMUX_ASIDE_LANG", "en")
            .env("HOME", &self.0)
            .env("CLAUDE_CONFIG_DIR", self.0.join("claude"))
            .env("CODEX_HOME", self.0.join("codex"))
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XDG_DATA_HOME", self.0.join("data"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn names(dir: &PathBuf) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn the_skills_are_installed_for_each_chosen_ai_that_reads_skills() {
    let home = Home::new();

    let output = home.setup(&["--hooks", "claude,codex,agy", "--skills"]);

    for dir in [home.claude_skills(), home.codex_skills()] {
        assert_eq!(names(&dir), ["tmux-aside-launch", "tmux-aside-qa"]);
        for (name, text) in SKILLS {
            assert_eq!(
                std::fs::read_to_string(dir.join(name).join("SKILL.md")).unwrap(),
                text
            );
        }
    }
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Antigravity: skills are not installed")
    );
}

#[test]
fn no_skills_installs_none() {
    let home = Home::new();

    home.setup(&["--hooks", "claude,codex", "--no-skills"]);

    assert!(names(&home.claude_skills()).is_empty());
    assert!(names(&home.codex_skills()).is_empty());
}

#[test]
fn only_the_chosen_ais_get_the_skills() {
    let home = Home::new();

    home.setup(&["--hooks", "codex", "--skills"]);

    assert!(names(&home.claude_skills()).is_empty());
    assert_eq!(names(&home.codex_skills()).len(), 2);
}

#[test]
fn a_second_run_replaces_the_skills_and_leaves_the_users_own_alone() {
    let home = Home::new();
    let own = home.claude_skills().join("my-skill/SKILL.md");
    std::fs::create_dir_all(own.parent().unwrap()).unwrap();
    std::fs::write(&own, "mine").unwrap();
    home.setup(&["--hooks", "claude", "--skills"]);
    let installed = home.claude_skills().join("tmux-aside-launch/SKILL.md");
    std::fs::write(&installed, "edited").unwrap();
    let stale = home.claude_skills().join("tmux-aside-launch/notes.md");
    std::fs::write(&stale, "old").unwrap();

    home.setup(&["--hooks", "claude", "--skills"]);

    assert_eq!(
        names(&home.claude_skills()),
        ["my-skill", "tmux-aside-launch", "tmux-aside-qa"]
    );
    assert_eq!(std::fs::read_to_string(own).unwrap(), "mine");
    assert_eq!(std::fs::read_to_string(installed).unwrap(), SKILLS[0].1);
    assert!(!stale.exists());
}

#[test]
fn each_skill_is_named_after_its_folder_and_reads_the_status_first() {
    for (name, text) in SKILLS {
        assert!(text.contains(&format!("\nname: {name}\n")), "{name}");
        assert!(text.contains("tmux-aside status --json"), "{name}");
    }
}

#[test]
fn without_asking_the_skills_are_installed_for_the_chosen_ais() {
    let home = Home::new();

    home.setup(&["--hooks", "claude"]);

    assert_eq!(names(&home.claude_skills()).len(), 2);
}

#[test]
fn skills_without_an_ai_say_that_none_was_installed() {
    let home = Home::new();

    let output = home.setup(&["--no-hooks", "--skills"]);

    assert!(String::from_utf8_lossy(&output.stdout).contains("no skills were installed"));
}
