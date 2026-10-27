use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use tmux_aside::setup::{
    Binding, block_bindings, describe_key, merge_block, parse_key, render_block, tmux_config_path,
};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tmux-aside-setup-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn config(&self) -> PathBuf {
        self.0.join("tmux.conf")
    }

    fn read(&self) -> String {
        std::fs::read_to_string(self.config()).unwrap()
    }

    fn setup(&self, arguments: &[&str]) {
        let output = self.run_setup(arguments, None);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn run_setup(&self, arguments: &[&str], server: Option<&Server>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tmux-aside"));
        command
            .args(["setup", "--yes", "--no-hooks", "--next", "-", "--config"])
            .arg(self.config())
            .args(arguments)
            .env_remove("TMUX");
        if let Some(server) = server {
            command.env("TMUX", format!("{},0,0", server.socket));
        }
        command.output().unwrap()
    }
}

struct Server {
    name: String,
    socket: String,
}

impl Server {
    fn start() -> Self {
        let name = format!(
            "tmux-aside-setup-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        );
        let mut server = Server {
            name,
            socket: String::new(),
        };
        server.tmux(&["-f", "/dev/null", "new-session", "-d"]);
        server.socket = server.tmux(&["display-message", "-p", "#{socket_path}"]);
        server
    }

    fn tmux(&self, arguments: &[&str]) -> String {
        let output = Command::new("tmux")
            .args(["-L", &self.name])
            .args(arguments)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn binding(&self, table: &str, key: &str) -> String {
        self.tmux(&["list-keys", "-T", table, key])
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.tmux(&["kill-server"]);
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn binding(key: &str, command: &str) -> Binding {
    Binding {
        key: key.to_string(),
        command: command.to_string(),
    }
}

#[test]
fn writes_the_default_keys_to_a_new_config() {
    let dir = Dir::new();

    dir.setup(&[]);

    let exe = env!("CARGO_BIN_EXE_tmux-aside");
    assert_eq!(
        dir.read(),
        format!(
            "# >>> tmux-aside >>>\n\
             bind-key B run-shell \"'{exe}' toggle\"\n\
             bind-key R run-shell \"'{exe}' reload\"\n\
             # <<< tmux-aside <<<\n"
        )
    );
}

#[test]
fn keeps_the_rest_of_the_config_and_backs_it_up() {
    let dir = Dir::new();
    std::fs::write(dir.config(), "set -g mouse on\n").unwrap();

    dir.setup(&[]);

    let content = dir.read();
    assert!(content.starts_with("set -g mouse on\n\n# >>> tmux-aside >>>\n"));
    assert_eq!(
        std::fs::read_to_string(dir.0.join("tmux.conf.bak")).unwrap(),
        "set -g mouse on\n"
    );
}

#[test]
fn a_second_run_replaces_the_block_in_place() {
    let dir = Dir::new();
    std::fs::write(dir.config(), "set -g mouse on\n").unwrap();
    dir.setup(&[]);
    std::fs::write(dir.config(), format!("{}set -g base-index 1\n", dir.read())).unwrap();

    dir.setup(&["--toggle", "-n M-b"]);

    let content = dir.read();
    assert_eq!(content.matches("# >>> tmux-aside >>>").count(), 1);
    assert!(content.contains("bind-key -n M-b run-shell"));
    assert!(!content.contains("bind-key B run-shell"));
    assert!(content.ends_with("# <<< tmux-aside <<<\nset -g base-index 1\n"));
}

#[test]
fn a_dash_leaves_the_action_unbound() {
    let dir = Dir::new();

    dir.setup(&["--reload", "-"]);

    let content = dir.read();
    assert!(content.contains("toggle"));
    assert!(!content.contains("reload"));
}

#[test]
fn an_empty_block_removes_the_marked_one() {
    let existing = "set -g mouse on\n\n# >>> tmux-aside >>>\nbind-key b run-shell \"x\"\n# <<< tmux-aside <<<\nset -g base-index 1\n";

    assert_eq!(
        merge_block(existing, ""),
        "set -g mouse on\n\nset -g base-index 1\n"
    );
}

#[test]
fn commands_are_quoted_for_tmux() {
    assert_eq!(
        render_block(&[binding("b", "'/a \"b\"/tmux-aside' toggle")]),
        "# >>> tmux-aside >>>\nbind-key b run-shell \"'/a \\\"b\\\"/tmux-aside' toggle\"\n# <<< tmux-aside <<<\n"
    );
}

#[test]
fn reads_the_bindings_in_the_block_only() {
    let content = format!(
        "bind-key x run-shell \"other\"\n{}",
        render_block(&[
            binding("B", "'/a \"b\"/tmux-aside' toggle"),
            binding("-n M-w", "next")
        ])
    );

    assert_eq!(
        block_bindings(&content),
        [
            binding("B", "'/a \"b\"/tmux-aside' toggle"),
            binding("-n M-w", "next")
        ]
    );
}

#[test]
fn describes_keys_the_way_people_press_them() {
    assert_eq!(describe_key("B"), "prefix + B");
    assert_eq!(describe_key("-n M-w"), "M-w (no prefix)");
    assert_eq!(describe_key("-T copy-mode-vi y"), "y in copy-mode-vi");
}

#[test]
fn reads_a_key_written_as_bind_key_takes_it_or_as_it_is_described() {
    for key in ["r", "-n M-w", "-T copy-mode-vi y"] {
        assert_eq!(parse_key(key), key);
        assert_eq!(parse_key(&describe_key(key)), key);
    }
    assert_eq!(parse_key("  Prefix + R "), "R");
}

#[test]
fn prefers_the_config_tmux_reads_first() {
    let home = Dir::new();
    let xdg = home.0.join("xdg");
    std::fs::create_dir_all(xdg.join("tmux")).unwrap();

    assert_eq!(
        tmux_config_path(&home.0, Some(&xdg)),
        home.0.join(".tmux.conf")
    );

    std::fs::write(xdg.join("tmux/tmux.conf"), "").unwrap();
    assert_eq!(
        tmux_config_path(&home.0, Some(&xdg)),
        xdg.join("tmux/tmux.conf")
    );

    std::fs::write(home.0.join(".tmux.conf"), "").unwrap();
    assert_eq!(
        tmux_config_path(&home.0, Some(&xdg)),
        home.0.join(".tmux.conf")
    );
}

#[test]
fn inside_tmux_the_keys_take_effect_and_stale_ones_are_removed() {
    let dir = Dir::new();
    let server = Server::start();
    assert!(dir.run_setup(&[], Some(&server)).status.success());
    assert!(server.binding("prefix", "B").contains("toggle"));

    let output = dir.run_setup(&["--toggle", "-n M-b"], Some(&server));

    assert!(output.status.success());
    assert!(server.binding("root", "M-b").contains("toggle"));
    assert!(server.binding("prefix", "B").is_empty());
    assert!(server.binding("prefix", "R").contains("reload"));
}

#[test]
fn a_key_tmux_rejects_changes_nothing() {
    let dir = Dir::new();
    let server = Server::start();
    assert!(dir.run_setup(&[], Some(&server)).status.success());
    let before = dir.read();

    let output = dir.run_setup(&["--toggle", "-x Q"], Some(&server));

    assert!(!output.status.success());
    assert_eq!(dir.read(), before);
    assert!(server.binding("prefix", "B").contains("toggle"));
    assert!(server.binding("prefix", "R").contains("reload"));
}
