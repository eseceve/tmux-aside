use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use tmux_aside::config::{self, parse};
use tmux_aside::pack::{self, MANIFEST, is_url, with_pack_key};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tmux-aside-pack-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Dir(std::fs::canonicalize(dir).unwrap())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn config(&self) -> PathBuf {
        self.path("config/tmux-aside/config.yaml")
    }

    fn setup(&self, pack: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tmux-aside"))
            .args(["setup", "--yes", "--no-hooks", "--next", "-", "--config"])
            .arg(self.path("tmux.conf"))
            .args(["--pack", pack])
            .env_remove("TMUX")
            .env("TMUX_ASIDE_LANG", "en")
            .env("HOME", self.path("home"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .output()
            .unwrap()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .env("HOME", dir)
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

fn write_pack(dir: &Path, manifest: &str) {
    std::fs::create_dir_all(dir.join("templates")).unwrap();
    std::fs::write(dir.join(MANIFEST), manifest).unwrap();
    std::fs::write(dir.join("templates/review.md"), "Review ${task}.\n").unwrap();
}

#[test]
fn the_pack_key_is_set_replaced_or_removed_leaving_the_rest() {
    let config = "language: es\n# my sections\nsections: []\n";

    let set = with_pack_key(config, Some(Path::new("/packs/team")));
    assert_eq!(set, format!("{config}pack: \"/packs/team\"\n"));
    assert_eq!(
        with_pack_key(&set, Some(Path::new("/packs/other"))),
        format!("{config}pack: \"/packs/other\"\n")
    );
    assert_eq!(with_pack_key(&set, None), config);
    assert_eq!(
        with_pack_key("pack:\n  /old\nlanguage: es\n", Some(Path::new("/new"))),
        "pack: \"/new\"\nlanguage: es\n"
    );
}

#[test]
fn git_urls_are_told_apart_from_paths() {
    for url in [
        "git@github.com:org/pack.git",
        "https://github.com/org/pack",
        "file:///tmp/pack.git",
        "github.com:org/pack.git",
    ] {
        assert!(is_url(url), "{url}");
    }
    for path in ["../pack", "/home/me/pack", "pack"] {
        assert!(!is_url(path), "{path}");
    }
}

#[test]
fn a_manifest_resolves_its_criteria_and_templates_inside_the_pack() {
    let dir = Dir::new();
    write_pack(
        &dir.0,
        "criteria:\n  ready_to_merge:\n    command: bin/ready\n    args: [\"--team\", \"web\"]\n  ready_to_deploy:\n    args: [\"--passed-label\", \"qa:ok\"]\n",
    );

    let pack = pack::load(&dir.0).unwrap();

    let ready = pack.criteria.ready_to_merge.unwrap();
    assert_eq!(
        ready.command,
        Some(dir.path("bin/ready").to_string_lossy().into_owned())
    );
    assert_eq!(ready.args, ["--team", "web"]);
    assert_eq!(pack.criteria.ready_to_deploy.unwrap().command, None);
    assert_eq!(pack.templates, dir.path("templates"));
}

#[test]
fn the_configs_criteria_win_over_the_packs() {
    let dir = Dir::new();
    write_pack(
        &dir.0,
        "criteria:\n  ready_to_merge:\n    command: bin/ready\n  qa:\n    command: bin/qa\n",
    );
    let config = parse("criteria:\n  qa:\n    command: /opt/qa\n").unwrap();

    let criteria = pack::criteria(&config, Some(&pack::load(&dir.0).unwrap()));

    assert_eq!(
        criteria.ready_to_merge.unwrap().command,
        Some(dir.path("bin/ready").to_string_lossy().into_owned())
    );
    assert_eq!(criteria.qa.unwrap().command.as_deref(), Some("/opt/qa"));
}

#[test]
fn the_configs_deployed_action_wins_over_the_packs() {
    let dir = Dir::new();
    write_pack(&dir.0, "on_deployed:\n  template: sync\n");
    let pack = pack::load(&dir.0).unwrap();

    let from_pack = pack::on_deployed(&parse("").unwrap(), Some(&pack)).unwrap();
    let configured = parse("on_deployed:\n  window: true\n").unwrap();
    let from_config = pack::on_deployed(&configured, Some(&pack)).unwrap();

    assert_eq!(
        (from_pack.template.as_str(), from_pack.window),
        ("sync", false)
    );
    assert_eq!(
        (from_config.template.as_str(), from_config.window),
        ("deployed", true)
    );
    assert_eq!(pack::on_deployed(&parse("").unwrap(), None), None);
}

/// Loads a config of `yaml` that names a Pack with the manifest `manifest`.
fn load_with_pack(yaml: &str, manifest: &str) -> config::Config {
    let dir = Dir::new();
    write_pack(&dir.path("pack"), manifest);
    std::fs::create_dir_all(dir.config().parent().unwrap()).unwrap();
    std::fs::write(
        dir.config(),
        format!("pack: \"{}\"\n{yaml}", dir.path("pack").display()),
    )
    .unwrap();
    config::load(&dir.config()).unwrap().unwrap()
}

#[test]
fn a_packs_settings_fill_in_what_the_config_leaves_out() {
    let config = load_with_pack(
        "theme:\n  accent: \"#111111\"\n",
        "sections:\n  - plugin: agents\n  - plugin: usage\n    pin: bottom\ntheme:\n  accent: \"#222222\"\n  muted: \"#333333\"\nauto_qa: true\nai: codex\n",
    );

    let plugins: Vec<&str> = config.sections.iter().map(|s| s.plugin.as_str()).collect();
    assert_eq!(plugins, ["agents", "usage"]);
    assert_eq!(config.theme.accent, "#111111");
    assert_eq!(config.theme.muted, "#333333");
    assert_eq!(config.theme.success, config::Theme::default().success);
    assert!(config.auto_qa);
    assert_eq!(config.ai.as_deref(), Some("codex"));
}

#[test]
fn the_configs_sections_replace_the_packs_whole_and_its_false_wins() {
    let config = load_with_pack(
        "sections:\n  - plugin: clock\nauto_qa: false\n",
        "sections:\n  - plugin: agents\n  - plugin: usage\nauto_qa: true\n",
    );

    let plugins: Vec<&str> = config.sections.iter().map(|s| s.plugin.as_str()).collect();
    assert_eq!(plugins, ["clock"]);
    assert!(!config.auto_qa);
}

#[test]
fn a_packs_plugin_and_discovery_paths_are_relative_to_the_pack() {
    let config = load_with_pack(
        "",
        "sections:\n  - plugin: bin/status\n  - plugin: agents\ndiscover:\n  - command: bin/list\n",
    );

    assert!(
        config.sections[0].plugin.ends_with("/pack/bin/status"),
        "{}",
        config.sections[0].plugin
    );
    assert_eq!(config.sections[1].plugin, "agents");
    match &config.discover[0] {
        config::DiscoverySource::Command { command, .. } => {
            assert!(command.ends_with("/pack/bin/list"), "{command}")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_manifest_cannot_name_another_pack_nor_a_config_its_templates() {
    let dir = Dir::new();
    write_pack(&dir.0, "pack: /elsewhere\n");

    assert!(pack::load(&dir.0).is_err());
    assert!(parse("templates: mine\n").is_err());
}

#[test]
fn an_invalid_manifest_is_reported_clearly() {
    let dir = Dir::new();
    write_pack(&dir.0, "criteria:\n  merged:\n    command: bin/x\n");

    let error = format!("{:#}", pack::load(&dir.0).unwrap_err());

    assert!(error.contains("invalid pack manifest"), "{error}");
    assert!(error.contains("pack.yaml"), "{error}");
}

#[test]
fn setup_with_a_pack_path_writes_the_key_without_copying_anything() {
    let dir = Dir::new();
    let team = dir.path("team-pack");
    write_pack(&team, "templates: templates\n");
    std::fs::create_dir_all(dir.config().parent().unwrap()).unwrap();
    std::fs::write(dir.config(), "language: es\n").unwrap();

    let output = dir.setup(team.to_str().unwrap());

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(dir.config()).unwrap(),
        format!("language: es\npack: \"{}\"\n", team.display())
    );
    assert!(!dir.path("data").exists());
}

#[test]
fn setup_with_a_pack_url_clones_it_once_and_pulls_it_fast_forward_after() {
    let dir = Dir::new();
    let work = dir.path("work");
    write_pack(&work, "templates: templates\n");
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "pack"]);
    git(&dir.0, &["clone", "-q", "--bare", "work", "team-pack.git"]);
    let url = format!("file://{}", dir.path("team-pack.git").display());

    let first = dir.setup(&url);

    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let cloned = dir.path("data/tmux-aside/packs/team-pack");
    assert!(cloned.join(MANIFEST).exists());
    assert!(
        std::fs::read_to_string(dir.config())
            .unwrap()
            .contains(&format!("pack: \"{}\"", cloned.display()))
    );

    std::fs::write(work.join("templates/fix.md"), "Fix ${task}.\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "fix template"]);
    git(
        &work,
        &[
            "push",
            "-q",
            dir.path("team-pack.git").to_str().unwrap(),
            "main",
        ],
    );

    let second = dir.setup(&url);

    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(cloned.join("templates/fix.md").exists());
}

#[test]
fn setup_refuses_a_pack_without_a_valid_manifest_and_writes_no_key() {
    let dir = Dir::new();
    let empty = dir.path("not-a-pack");
    std::fs::create_dir_all(&empty).unwrap();

    let output = dir.setup(empty.to_str().unwrap());

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("pack.yaml"));
    assert!(!dir.config().exists());
}

#[test]
fn a_clone_from_another_url_is_not_reused() {
    let dir = Dir::new();
    let work = dir.path("work");
    write_pack(&work, "templates: templates\n");
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "pack"]);
    git(&dir.0, &["clone", "-q", "--bare", "work", "team-pack.git"]);
    std::fs::create_dir_all(dir.path("other")).unwrap();
    git(
        &dir.0,
        &["clone", "-q", "--bare", "work", "other/team-pack.git"],
    );
    assert!(
        dir.setup(&format!("file://{}", dir.path("team-pack.git").display()))
            .status
            .success()
    );

    let output = dir.setup(&format!(
        "file://{}",
        dir.path("other/team-pack.git").display()
    ));

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("was cloned from"));
}
