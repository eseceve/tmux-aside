use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::TmuxServer;
use super::daemon::{Home, fixture};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// A clone of a local origin, with one local commit that origin does not have.
pub struct Repo {
    pub dir: PathBuf,
}

impl Repo {
    pub fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tmux-aside-repo-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(dir.join("origin")).unwrap();
        let repo = Repo { dir };
        git_in(&repo.origin(), &["init", "-q", "-b", "main"]);
        git_in(
            &repo.origin(),
            &["commit", "-q", "--allow-empty", "-m", "first"],
        );
        git_in(&repo.dir, &["clone", "-q", "origin", "clone"]);
        let origin = repo.origin();
        git_in(
            &repo.clone(),
            &["remote", "set-url", "origin", "git@github.com:org/shop.git"],
        );
        git_in(
            &repo.clone(),
            &[
                "config",
                &format!("url.{}.insteadOf", origin.display()),
                "git@github.com:org/shop.git",
            ],
        );
        git_in(
            &repo.clone(),
            &["commit", "-q", "--allow-empty", "-m", "local"],
        );
        repo
    }

    pub fn origin(&self) -> PathBuf {
        self.dir.join("origin")
    }

    pub fn clone(&self) -> PathBuf {
        self.dir.join("clone")
    }

    pub fn worktree(&self, handle: &str) -> PathBuf {
        self.dir.join("clone__worktrees").join(handle)
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

pub fn with_fake_agent(command: &mut Command) -> &mut Command {
    let path = std::env::var("PATH").unwrap_or_default();
    command.env(
        "PATH",
        format!("{}:{}:{path}", fixture("agent"), fixture("gh")),
    )
}

pub fn launch(server: &TmuxServer, repo: &Repo, extra: &[&str]) -> Output {
    let clone = repo.clone();
    let mut args = vec!["launch", clone.to_str().unwrap()];
    args.extend_from_slice(extra);
    with_fake_agent(&mut server.aside_command(&args))
        .output()
        .unwrap()
}

pub fn launch_ok(server: &TmuxServer, repo: &Repo, extra: &[&str]) -> Output {
    let output = launch(server, repo, extra);
    assert!(
        output.status.success(),
        "launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

pub fn implement(branch: &str) -> Vec<&str> {
    vec![
        "--template",
        "implement",
        "--task",
        "Add a dark theme",
        "--branch",
        branch,
    ]
}

pub fn launches_dir(home: &Home) -> PathBuf {
    home.path("state/tmux-aside/launches")
}
