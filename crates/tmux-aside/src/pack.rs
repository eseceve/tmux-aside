use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use crate::config::{self, Config, Criteria, CriterionCommand, DeployedAction};

/// The manifest every Pack holds at its root.
pub const MANIFEST: &str = "pack.yaml";
const DEFAULT_TEMPLATES: &str = "templates";

/// A private bundle of Criteria and Templates for one organization, read from its manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    /// The Criteria that replace the defaults, with commands resolved inside the Pack.
    pub criteria: Criteria,
    /// The folder of the Pack's Templates.
    pub templates: PathBuf,
    /// What the Watcher does once pull requests are deployed, unless the config says.
    pub on_deployed: Option<DeployedAction>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    #[serde(default)]
    criteria: Criteria,
    templates: Option<String>,
    on_deployed: Option<DeployedAction>,
}

/// Reads the manifest of the Pack in `dir`. Criterion commands and the Templates folder are
/// relative to `dir`.
pub fn load(dir: &Path) -> Result<Pack> {
    let path = dir.join(MANIFEST);
    let yaml = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let manifest: Manifest = serde_saphyr::from_str(&yaml)
        .map_err(|error| anyhow!("invalid pack manifest {}: {error}", path.display()))?;
    let inside = |criterion: Option<CriterionCommand>| {
        criterion.map(|criterion| CriterionCommand {
            command: criterion
                .command
                .map(|command| dir.join(command).to_string_lossy().into_owned()),
            ..criterion
        })
    };
    Ok(Pack {
        criteria: Criteria {
            ready_to_merge: inside(manifest.criteria.ready_to_merge),
            qa: inside(manifest.criteria.qa),
            ready_to_deploy: inside(manifest.criteria.ready_to_deploy),
            deployed: inside(manifest.criteria.deployed),
        },
        templates: dir.join(manifest.templates.as_deref().unwrap_or(DEFAULT_TEMPLATES)),
        on_deployed: manifest.on_deployed,
    })
}

/// The Pack the config points to, if any, a relative `pack` being relative to the config's
/// folder. An error means the `pack` key names no valid Pack.
pub fn configured(config: &Config) -> Result<Option<Pack>> {
    let Some(dir) = config.pack.as_deref() else {
        return Ok(None);
    };
    let folder = config::default_path()?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    load(&folder.join(dir)).map(Some)
}

/// The Criterion of each stage: the config's, then the Pack's, then none (the default).
pub fn criteria(config: &Config, pack: Option<&Pack>) -> Criteria {
    let from_pack = pack.map(|pack| pack.criteria.clone()).unwrap_or_default();
    let configured = config.criteria.clone();
    Criteria {
        ready_to_merge: configured.ready_to_merge.or(from_pack.ready_to_merge),
        qa: configured.qa.or(from_pack.qa),
        ready_to_deploy: configured.ready_to_deploy.or(from_pack.ready_to_deploy),
        deployed: configured.deployed.or(from_pack.deployed),
    }
}

/// What the Watcher does once pull requests are deployed: the config's, then the Pack's.
pub fn on_deployed(config: &Config, pack: Option<&Pack>) -> Option<DeployedAction> {
    config
        .on_deployed
        .clone()
        .or_else(|| pack.and_then(|pack| pack.on_deployed.clone()))
}

/// Whether `source` is a git URL (`https://...`, `file://...`, `git@host:path`, `host:path`)
/// rather than a path.
pub fn is_url(source: &str) -> bool {
    source.contains("://")
        || source
            .split_once(':')
            .is_some_and(|(host, _)| !host.is_empty() && !host.contains('/'))
}

/// Makes the Pack at `source` available and returns its folder: a path is used in place; a
/// URL is cloned once into `packs` with the user's git credentials, and pulled fast-forward
/// only when it was cloned before. The Pack must have a valid manifest.
pub fn install(source: &str, packs: &Path) -> Result<PathBuf> {
    let dir = if is_url(source) {
        let name = source
            .trim_end_matches('/')
            .rsplit(['/', ':'])
            .next()
            .map(|name| name.trim_end_matches(".git"))
            .filter(|name| !name.is_empty())
            .ok_or_else(|| anyhow!("cannot name a pack from {source}"))?;
        let dir = packs.join(name);
        if dir.exists() {
            let origin = git(&["-C", &dir.to_string_lossy(), "remote", "get-url", "origin"])?;
            if origin != source {
                bail!(
                    "{} was cloned from {origin}, not {source}; remove it to clone {source}",
                    dir.display()
                );
            }
            git(&["-C", &dir.to_string_lossy(), "pull", "-q", "--ff-only"])?;
        } else {
            std::fs::create_dir_all(packs)?;
            git(&["clone", "-q", source, &dir.to_string_lossy()])?;
        }
        dir
    } else {
        std::fs::canonicalize(source).with_context(|| format!("no pack at {source}"))?
    };
    load(&dir)?;
    Ok(dir)
}

/// Directory of the Packs cloned from a URL: `$XDG_DATA_HOME/tmux-aside/packs`.
pub fn packs_dir() -> Result<PathBuf> {
    Ok(config::data_dir()?.join("packs"))
}

/// `content` of a config file with its top-level `pack` key set to `dir`, or removed when
/// `dir` is `None`, leaving every other line as it was.
pub fn with_pack_key(content: &str, dir: Option<&Path>) -> String {
    let line = dir.map(|dir| format!("pack: {}", yaml_string(&dir.to_string_lossy())));
    let mut replaced = false;
    let mut in_value = false;
    let mut lines: Vec<String> = Vec::new();
    for existing in content.lines() {
        if existing.starts_with("pack:") {
            if let Some(line) = line.as_ref().filter(|_| !replaced) {
                lines.push(line.clone());
            }
            replaced = true;
            in_value = true;
        } else if in_value && existing.starts_with([' ', '\t']) {
            continue;
        } else {
            in_value = false;
            lines.push(existing.to_string());
        }
    }
    if let (false, Some(line)) = (replaced, line) {
        lines.push(line);
    }
    lines.iter().map(|line| format!("{line}\n")).collect()
}

fn yaml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn git(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .context("failed to spawn git")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
