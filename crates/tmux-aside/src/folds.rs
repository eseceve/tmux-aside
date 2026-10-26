use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{self, SectionConfig};
use crate::plugin;
use crate::view::View;

/// Whether a section is folded and which of its groups are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fold {
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default)]
    pub groups: Vec<String>,
}

/// Folds by section key, as remembered across daemon restarts.
pub type Folds = BTreeMap<String, Fold>;

/// Identifies a configured section across restarts: its title as written in the config (so
/// `${today}` stays stable), else its plugin name.
pub fn key(section: &SectionConfig) -> String {
    section
        .title
        .clone()
        .unwrap_or_else(|| plugin::display_name(&section.plugin))
}

/// File holding the remembered folds, under the state directory.
pub fn path() -> Option<PathBuf> {
    config::state_dir().ok().map(|dir| dir.join("folds.json"))
}

/// Remembered folds, empty when the file is missing or unreadable.
pub fn read(path: &Path) -> Folds {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub fn write(path: &Path, folds: &Folds) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(folds)?)
}

/// Folds of the sections of `view`, configured by `configs`, leaving out the ones folded as
/// their config starts them.
pub fn of(view: &View, configs: &[SectionConfig]) -> Folds {
    configs
        .iter()
        .zip(&view.sections)
        .filter_map(|(config, section)| {
            let fold = Fold {
                collapsed: section.collapsed,
                groups: section.collapsed_groups.clone(),
            };
            let initial = Fold {
                collapsed: config.collapsed && section.header,
                groups: vec![],
            };
            (fold != initial).then(|| (key(config), fold))
        })
        .collect()
}

/// Folds the sections of `view`, configured by `configs`, as remembered in `folds`.
pub fn apply(view: &mut View, configs: &[SectionConfig], folds: &Folds) {
    for (config, section) in configs.iter().zip(&mut view.sections) {
        if let Some(fold) = folds.get(&key(config)) {
            section.collapsed = fold.collapsed && section.header;
            section.collapsed_groups = fold.groups.clone();
        }
    }
}
