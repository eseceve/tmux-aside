use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use crate::i18n::Language;
use crate::launch::{self, Launch, git, github_repository};
use crate::tmux;

/// What removing a Launch's worktree would lose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Worktree {
    /// Not a git worktree any more, such as after it was removed.
    Missing,
    /// A worktree that is not tmux-aside's, such as the clone itself or the one of a followed or
    /// discovered pull request; never removed.
    Main,
    /// Nothing would be lost. Seen after a cleanup, it means the removal failed.
    Clean,
    /// Files changed and not committed.
    Uncommitted,
    /// Commits on no remote branch and not the pull request's head.
    Unpushed,
}

/// What removing `worktree` would lose. Commits are pushed when the branch is at the pull
/// request's `head` or when a remote branch holds them.
pub fn inspect(worktree: &Path, head: Option<&str>) -> Worktree {
    if !worktree.exists() {
        return Worktree::Missing;
    }
    let Ok(changes) = git(worktree, &["status", "--porcelain"]) else {
        return Worktree::Missing;
    };
    if !changes.is_empty() {
        return Worktree::Uncommitted;
    }
    if head.is_some() && git(worktree, &["rev-parse", "HEAD"]).ok().as_deref() == head {
        return Worktree::Clean;
    }
    match git(
        worktree,
        &["rev-list", "--count", "HEAD", "--not", "--remotes"],
    )
    .as_deref()
    {
        Ok("0") => Worktree::Clean,
        _ => Worktree::Unpushed,
    }
}

/// Whether the agent's pane of `launch` is still the one launched in its worktree and window:
/// tmux reuses pane ids after a restart. Ask before removing the worktree.
pub fn agent_still_there(launch: &Launch) -> bool {
    let worktree = launch
        .worktree
        .canonicalize()
        .unwrap_or_else(|_| launch.worktree.clone());
    tmux::run(&[
        "display-message",
        "-p",
        "-t",
        &launch.pane,
        "#{window_id}\t#{pane_current_path}",
    ])
    .ok()
    .and_then(|live| {
        let (window, path) = live.split_once('\t')?;
        Some(window == launch.window && Path::new(path).starts_with(&worktree))
    })
    .unwrap_or(false)
}

/// Removes `worktree` from `repository` with `git worktree remove`, which refuses to lose
/// changes unless `force`.
pub fn remove_worktree(repository: &Path, worktree: &Path, force: bool) -> Result<()> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    let worktree = worktree
        .to_str()
        .ok_or_else(|| anyhow!("{} has no UTF-8 name", worktree.display()))?;
    args.push(worktree);
    git(repository, &args).map(|_| ())
}

/// What `tmux-aside remove` and `prune` may lose.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Remove worktrees with uncommitted changes or unpushed commits too.
    pub force: bool,
    /// Keep the local branch of a removed worktree.
    pub keep_branch: bool,
}

/// Removes the Launch that `target` names (its id, branch or pull request URL; the Launch of the
/// current pane when none): its worktree and local branch when the worktree is its own, its
/// record, and last its agent's pane, which closes its window unless someone split it. A QA
/// Launch, which works in the clone, and a followed pull request only lose their record and
/// pane. Fails without changing anything when the worktree would lose work and `force` is off.
/// Run from the agent's own pane, closing it ends this command before its report is printed.
/// `target` can also be the path of a worktree no Launch follows, removed the same way along
/// with every pane working in it.
pub fn remove(target: Option<&str>, options: Options, language: Language) -> Result<String> {
    let messages = language.messages();
    let launches = launch::list()?;
    let launch = match find_launch(&launches, target, language) {
        Ok(launch) => launch,
        Err(error) => {
            return match target.map(Path::new).filter(|path| path.is_dir()) {
                Some(path) => remove_orphan(path, options, language),
                None => Err(error),
            };
        }
    };
    let agent = agent_still_there(launch);
    let mut report = Vec::new();
    if !launch.keeps_worktree && launch.worktree != launch.repository {
        let mut state = inspect(&launch.worktree, None);
        if state == Worktree::Unpushed
            && let Some(url) = &launch.pull_request
        {
            state = inspect(&launch.worktree, pull_request_head(url).ok().as_deref());
        }
        let what = launch.id.as_str();
        match state {
            Worktree::Uncommitted if !options.force => bail!((messages.kept_uncommitted)(what)),
            Worktree::Unpushed if !options.force => bail!((messages.kept_unpushed)(what)),
            Worktree::Missing | Worktree::Main => {}
            _ => remove_worktree(&launch.repository, &launch.worktree, options.force)?,
        }
        if !options.keep_branch
            && let Some(error) = delete_branch(&launch.repository, &launch.branch)
        {
            report.push((messages.branch_not_deleted)(&launch.branch, &error));
        }
    }
    launch::forget(&launch.id)?;
    report.insert(0, (messages.removed)(&launch.id));
    if agent {
        tmux::run(&["kill-pane", "-t", &launch.pane])?;
    }
    Ok(report.join("\n"))
}

fn find_launch<'a>(
    launches: &'a [Launch],
    target: Option<&str>,
    language: Language,
) -> Result<&'a Launch> {
    let messages = language.messages();
    let Some(target) = target else {
        let pane = std::env::var("TMUX_PANE").unwrap_or_default();
        return launches
            .iter()
            .find(|launch| !pane.is_empty() && launch.pane == pane)
            .ok_or_else(|| anyhow!(messages.not_a_launch_pane));
    };
    let path = canonical(Path::new(target));
    if let Some(launch) = launches
        .iter()
        .find(|launch| launch.id == target || canonical(&launch.worktree) == path)
    {
        return Ok(launch);
    }
    let matching: Vec<&Launch> = launches
        .iter()
        .filter(|launch| launch.branch == target || launch.pull_request.as_deref() == Some(target))
        .collect();
    match matching.as_slice() {
        [launch] => Ok(launch),
        [] => bail!((messages.unknown_launch)(target)),
        _ => {
            let ids: Vec<&str> = matching.iter().map(|launch| launch.id.as_str()).collect();
            bail!((messages.ambiguous_launch)(target, &ids.join(", ")))
        }
    }
}

/// Removes the worktrees in `<clone>__worktrees/` that no Launch follows, such as those of
/// forgotten Launches or of other tools, once they are done: the pull request of their branch
/// was merged or closed at the worktree's commit, or the remote branch they tracked is gone.
/// Each path is a clone, a worktree of one, or a directory of clones. A worktree that would
/// lose work is kept and reported unless `force`; one whose pull request is still open, or that
/// never had one, is kept silently. With `dry_run`, only says what it would remove.
pub fn prune(
    paths: &[PathBuf],
    options: Options,
    dry_run: bool,
    language: Language,
) -> Result<String> {
    let messages = language.messages();
    let followed: BTreeSet<PathBuf> = launch::list()?
        .iter()
        .map(|launch| canonical(&launch.worktree))
        .collect();
    let mut report = Vec::new();
    for repository in clones(paths)? {
        for (worktree, branch) in orphans(&repository, &followed)? {
            let what = worktree.display().to_string();
            let state = match finished_head(&repository, &worktree, &branch) {
                Ok(Some(head)) => inspect(&worktree, head.as_deref()),
                Ok(None) => continue,
                Err(error) => {
                    report.push((messages.github_failed)(&what, &format!("{error:#}")));
                    continue;
                }
            };
            match state {
                Worktree::Uncommitted if !options.force => {
                    report.push((messages.kept_uncommitted)(&what));
                    continue;
                }
                Worktree::Unpushed if !options.force => {
                    report.push((messages.kept_unpushed)(&what));
                    continue;
                }
                Worktree::Missing | Worktree::Main => continue,
                _ => {}
            }
            if dry_run {
                report.push((messages.would_remove)(&what));
                continue;
            }
            match discard(&repository, &worktree, Some(&branch), options, language) {
                Ok(lines) => report.extend(lines),
                Err(error) => report.push(format!("{error:#}")),
            }
        }
        if !dry_run {
            let _ = git(&repository, &["worktree", "prune"]);
        }
    }
    if report.is_empty() {
        report.push(messages.nothing_to_prune.to_string());
    }
    Ok(report.join("\n"))
}

/// Removes the worktree at `path`, which no Launch follows, when it loses nothing or `force`.
fn remove_orphan(path: &Path, options: Options, language: Language) -> Result<String> {
    let messages = language.messages();
    let what = path.display().to_string();
    let repository = clone_of(path)
        .filter(|clone| *clone != canonical(path))
        .ok_or_else(|| anyhow!((messages.unknown_launch)(&what)))?;
    match inspect(path, None) {
        Worktree::Uncommitted if !options.force => bail!((messages.kept_uncommitted)(&what)),
        Worktree::Unpushed if !options.force => bail!((messages.kept_unpushed)(&what)),
        _ => {}
    }
    let branch = git(path, &["branch", "--show-current"]).unwrap_or_default();
    let branch = Some(branch.as_str()).filter(|branch| !branch.is_empty());
    Ok(discard(&repository, path, branch, options, language)?.join("\n"))
}

/// Removes `worktree` and its local `branch` unless `keep_branch`, then closes every pane that
/// worked in it. Returns the lines to report.
fn discard(
    repository: &Path,
    worktree: &Path,
    branch: Option<&str>,
    options: Options,
    language: Language,
) -> Result<Vec<String>> {
    let messages = language.messages();
    let panes = panes_in(worktree);
    remove_worktree(repository, worktree, options.force)?;
    let mut report = vec![(messages.removed)(&worktree.display().to_string())];
    if let Some(branch) = branch.filter(|_| !options.keep_branch)
        && let Some(error) = delete_branch(repository, branch)
    {
        report.push((messages.branch_not_deleted)(branch, &error));
    }
    for pane in panes {
        let _ = tmux::run(&["kill-pane", "-t", &pane]);
    }
    Ok(report)
}

/// The clones `paths` point to: each path's own clone, if it is in one, and the clones directly
/// inside it. No path means the current directory.
fn clones(paths: &[PathBuf]) -> Result<BTreeSet<PathBuf>> {
    let current = [std::env::current_dir()?];
    let paths = if paths.is_empty() {
        &current[..]
    } else {
        paths
    };
    let mut clones = BTreeSet::new();
    for path in paths {
        clones.extend(clone_of(path));
        let entries = std::fs::read_dir(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        for entry in entries.flatten() {
            if entry.path().join(".git").is_dir() {
                clones.extend(clone_of(&entry.path()));
            }
        }
    }
    Ok(clones)
}

fn clone_of(path: &Path) -> Option<PathBuf> {
    let common = git(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    let common = Path::new(&common);
    let clone = common.parent()?;
    (common.file_name()? == ".git").then(|| canonical(clone))
}

/// The worktrees in `<repository>__worktrees/` with a branch that no Launch follows.
fn orphans(repository: &Path, followed: &BTreeSet<PathBuf>) -> Result<Vec<(PathBuf, String)>> {
    let Some(name) = repository.file_name().and_then(|name| name.to_str()) else {
        return Ok(Vec::new());
    };
    let home = repository.with_file_name(format!("{name}__worktrees"));
    let listing = git(repository, &["worktree", "list", "--porcelain"])?;
    Ok(listing
        .split("\n\n")
        .filter_map(|entry| {
            let worktree = PathBuf::from(entry.lines().next()?.strip_prefix("worktree ")?);
            let branch = entry
                .lines()
                .find_map(|line| line.strip_prefix("branch refs/heads/"))?;
            (worktree.is_dir()
                && canonical(&worktree).starts_with(canonical(&home))
                && !followed.contains(&canonical(&worktree)))
            .then(|| (worktree, branch.to_string()))
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestState {
    state: String,
    head_ref_oid: String,
    is_cross_repository: bool,
}

/// Whether the work of `branch` in `worktree` is done: `Some(Some(head))` when a pull request
/// of the branch was merged or closed at the worktree's commit and none is open, `Some(None)`
/// when the remote branch it tracked is gone, `None` otherwise.
fn finished_head(
    repository: &Path,
    worktree: &Path,
    branch: &str,
) -> Result<Option<Option<String>>> {
    let head = git(worktree, &["rev-parse", "HEAD"])?;
    if let Some((owner, name)) = git(repository, &["config", "--get", "remote.origin.url"])
        .ok()
        .and_then(|url| github_repository(&url))
    {
        let output = Command::new("gh")
            .args([
                "pr",
                "list",
                "--repo",
                &format!("{owner}/{name}"),
                "--head",
                branch,
                "--state",
                "all",
                "--limit",
                "5",
                "--json",
                "state,headRefOid,isCrossRepository",
            ])
            .output()
            .context("failed to run gh")?;
        if !output.status.success() {
            bail!("gh: {}", String::from_utf8_lossy(&output.stderr).trim());
        }
        let pull_requests: Vec<PullRequestState> =
            serde_json::from_slice(&output.stdout).context("unexpected answer from gh pr list")?;
        let own: Vec<&PullRequestState> = pull_requests
            .iter()
            .filter(|pr| !pr.is_cross_repository)
            .collect();
        if own.iter().any(|pr| pr.state == "OPEN") {
            return Ok(None);
        }
        if own.iter().any(|pr| pr.head_ref_oid == head) {
            return Ok(Some(Some(head)));
        }
    }
    let upstream = |key: &str| {
        git(
            repository,
            &["config", "--get", &format!("branch.{branch}.{key}")],
        )
    };
    let (Ok(remote), Ok(merge)) = (upstream("remote"), upstream("merge")) else {
        return Ok(None);
    };
    let gone = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["ls-remote", "--exit-code", &remote, &merge])
        .output()
        .is_ok_and(|output| output.status.code() == Some(2));
    Ok(gone.then_some(None))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestHead {
    head_ref_oid: String,
}

fn pull_request_head(url: &str) -> Result<String> {
    let output = Command::new("gh")
        .args(["pr", "view", url, "--json", "headRefOid"])
        .output()
        .context("failed to run gh")?;
    if !output.status.success() {
        bail!("gh: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(serde_json::from_slice::<PullRequestHead>(&output.stdout)?.head_ref_oid)
}

/// Deletes the local `branch`, merged or not, if it exists; the error when it could not.
fn delete_branch(repository: &Path, branch: &str) -> Option<String> {
    let reference = format!("refs/heads/{branch}");
    git(
        repository,
        &["rev-parse", "--verify", "--quiet", &reference],
    )
    .ok()?;
    git(repository, &["branch", "-D", branch])
        .err()
        .map(|error| format!("{error:#}"))
}

fn panes_in(worktree: &Path) -> Vec<String> {
    let worktree = canonical(worktree);
    tmux::run(&["list-panes", "-a", "-F", "#{pane_id}\t#{pane_current_path}"])
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (pane, path) = line.split_once('\t')?;
            Path::new(path)
                .starts_with(&worktree)
                .then(|| pane.to_string())
        })
        .collect()
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
