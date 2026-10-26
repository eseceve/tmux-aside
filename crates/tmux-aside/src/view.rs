use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::protocol::{Action, Row, Tone};

/// Selectable line of a section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum Target {
    Header,
    Group(String),
    Row(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub section: usize,
    pub target: Target,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Key {
    Down,
    Up,
    Toggle,
    Activate,
    Collapse,
    Expand,
    /// Runs the selected row's `enter` action even when Enter only shows its window.
    Open,
}

/// How much space a section leaves between its rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    /// Rows follow each other and the next section starts right after.
    #[default]
    Compact,
    /// A line before every row but the first, or before its group line when it starts a group, and a blank
    /// line after the section. Between rows the line carries a bar, in the rows' color when
    /// both share it and muted otherwise.
    Comfortable,
}

/// Edge of the sidebar a section is stacked against.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pin {
    #[default]
    Top,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub title: String,
    pub rows: Vec<Row>,
    pub collapsed: bool,
    pub max_height: Option<usize>,
    pub scroll: usize,
    pub header: bool,
    #[serde(default = "shown")]
    pub counts: bool,
    #[serde(default)]
    pub group_gap: bool,
    #[serde(default)]
    pub density: Density,
    pub pin: Pin,
    #[serde(default)]
    pub collapsed_groups: Vec<String>,
}

impl Section {
    pub fn new(title: &str, rows: Vec<Row>, max_height: Option<usize>) -> Self {
        Section {
            title: title.to_string(),
            rows,
            collapsed: false,
            max_height,
            scroll: 0,
            header: true,
            counts: true,
            group_gap: false,
            density: Density::Compact,
            pin: Pin::Top,
            collapsed_groups: Vec::new(),
        }
    }

    /// Rows currently inside the section's viewport.
    pub fn visible_rows(&self) -> Range<usize> {
        match self.max_height {
            Some(height) => self.scroll..(self.scroll + height).min(self.rows.len()),
            None => 0..self.rows.len(),
        }
    }

    /// Number of rows in `group`.
    pub fn group_size(&self, group: &str) -> usize {
        self.rows
            .iter()
            .filter(|row| row.group.as_deref() == Some(group))
            .count()
    }

    /// Whether the rows of `group` are folded under its group line.
    pub fn is_group_collapsed(&self, group: &str) -> bool {
        self.collapsed_groups.iter().any(|g| g == group)
    }

    fn scroll_to(&mut self, row: usize) {
        let Some(height) = self.max_height.filter(|height| *height > 0) else {
            return;
        };
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll + height {
            self.scroll = row + 1 - height;
        }
    }
}

/// Colors the user assigned to each tone, as color names or `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    pub muted: String,
    pub accent: String,
    pub success: String,
    pub warning: String,
    pub danger: String,
    /// Background of the selected line; without it the selected line is drawn reversed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<String>,
    /// Text color of the rows of the tmux window the sidebar is shown in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            muted: "darkgray".to_string(),
            accent: "cyan".to_string(),
            success: "green".to_string(),
            warning: "yellow".to_string(),
            danger: "red".to_string(),
            selection: None,
            current: None,
        }
    }
}

impl Theme {
    /// Color configured for `tone`, or `None` for the terminal's default color.
    pub fn color(&self, tone: Tone) -> Option<&str> {
        match tone {
            Tone::Muted => Some(&self.muted),
            Tone::Accent => Some(&self.accent),
            Tone::Success => Some(&self.success),
            Tone::Warning => Some(&self.warning),
            Tone::Danger => Some(&self.danger),
            Tone::Plain => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub sections: Vec<Section>,
    pub selected: Option<Cursor>,
    #[serde(default)]
    pub theme: Theme,
}

impl View {
    /// Creates a view with its first line selected.
    pub fn new(sections: Vec<Section>) -> Self {
        let mut view = View {
            sections,
            selected: None,
            theme: Theme::default(),
        };
        view.selected = view.lines().into_iter().next();
        view
    }

    /// Applies a navigation key, keeping the selected row inside its section viewport.
    pub fn apply(&mut self, key: Key) {
        match key {
            Key::Activate => {
                self.activate();
            }
            Key::Down => self.step(1),
            Key::Up => self.step(-1),
            Key::Toggle => self.toggle(),
            Key::Collapse => self.fold(true),
            Key::Expand => self.fold(false),
            Key::Open => {}
        }
        self.scroll_to_selection();
    }

    /// Selects `cursor` if it is a visible line; returns whether it did.
    pub fn select(&mut self, cursor: Cursor) -> bool {
        if !self.lines().contains(&cursor) {
            return false;
        }
        self.selected = Some(cursor);
        self.scroll_to_selection();
        true
    }

    /// Selects `selected` again after the sections changed, falling back to a nearby line when
    /// it no longer exists.
    pub fn restore_selection(&mut self, selected: Option<Cursor>) {
        self.selected = selected;
        self.keep_selection_valid();
        self.scroll_to_selection();
    }

    /// Moves the selection to the first visible row of tmux `window`, as when the sidebar of
    /// that window takes focus while one of its agents was active. Without such a row the
    /// selection stays where it is.
    pub fn follow_window(&mut self, window: &str) {
        let host = self.lines().into_iter().find(|cursor| match cursor.target {
            Target::Row(row) => {
                self.sections[cursor.section].rows[row].window.as_deref() == Some(window)
            }
            _ => false,
        });
        if let Some(host) = host {
            self.selected = Some(host);
            self.scroll_to_selection();
        }
    }

    /// Collapses or expands the selected header or group; collapsing on a row moves the
    /// selection to the line that folds it (its group, else its section header).
    fn fold(&mut self, collapse: bool) {
        let Some(Cursor {
            section: index,
            target,
        }) = self.selected.clone()
        else {
            return;
        };
        let section = &mut self.sections[index];
        match target {
            Target::Header if section.header => section.collapsed = collapse,
            Target::Group(group) => {
                let position = section.collapsed_groups.iter().position(|g| *g == group);
                match (collapse, position) {
                    (true, None) => section.collapsed_groups.push(group),
                    (false, Some(position)) => {
                        section.collapsed_groups.remove(position);
                    }
                    _ => {}
                }
            }
            Target::Row(row) if collapse => {
                let parent = match &section.rows[row].group {
                    Some(group) => Some(Target::Group(group.clone())),
                    None => section.header.then_some(Target::Header),
                };
                if let Some(target) = parent {
                    self.selected = Some(Cursor {
                        section: index,
                        target,
                    });
                }
            }
            _ => {}
        }
    }

    /// Replaces a section's title and rows, keeping it inside the viewport, its collapsed
    /// groups limited to groups that still exist, and the selection on an existing line.
    pub fn set_section(&mut self, index: usize, title: &str, rows: Vec<Row>) {
        let Some(section) = self.sections.get_mut(index) else {
            return;
        };
        section.title = title.to_string();
        section.rows = rows;
        let groups: Vec<&str> = section
            .rows
            .iter()
            .filter_map(|row| row.group.as_deref())
            .collect();
        let kept: Vec<String> = section
            .collapsed_groups
            .iter()
            .filter(|group| groups.contains(&group.as_str()))
            .cloned()
            .collect();
        section.collapsed_groups = kept;
        let height = section.max_height.unwrap_or(section.rows.len());
        section.scroll = section
            .scroll
            .min(section.rows.len().saturating_sub(height));
        self.keep_selection_valid();
        self.scroll_to_selection();
    }

    /// Activates the selection: a header or group toggles, a row returns its `enter` action
    /// for the sidebar to run.
    pub fn activate(&mut self) -> Option<Action> {
        let Cursor { target, .. } = self.selected.clone()?;
        match target {
            Target::Header | Target::Group(_) => {
                self.toggle();
                None
            }
            Target::Row(_) => self.selected_action("enter"),
        }
    }

    /// The selected row, if the selection is on a row.
    pub fn selected_row(&self) -> Option<&Row> {
        match self.selected.as_ref()? {
            Cursor {
                section,
                target: Target::Row(row),
            } => self.sections[*section].rows.get(*row),
            _ => None,
        }
    }

    /// Action bound to `key` on the selected row, if a row is selected and has one.
    pub fn selected_action(&self, key: &str) -> Option<Action> {
        let Cursor {
            section,
            target: Target::Row(row),
        } = self.selected.clone()?
        else {
            return None;
        };
        self.sections[section].rows[row].find_action(key).cloned()
    }

    fn toggle(&mut self) {
        let Some(Cursor { section, target }) = self.selected.clone() else {
            return;
        };
        let section = &mut self.sections[section];
        match target {
            Target::Header if section.header => section.collapsed = !section.collapsed,
            Target::Group(group) => {
                if let Some(position) = section.collapsed_groups.iter().position(|g| *g == group) {
                    section.collapsed_groups.remove(position);
                } else {
                    section.collapsed_groups.push(group);
                }
            }
            _ => {}
        }
    }

    fn scroll_to_selection(&mut self) {
        if let Some(Cursor {
            section,
            target: Target::Row(row),
        }) = self.selected
        {
            self.sections[section].scroll_to(row);
        }
    }

    fn keep_selection_valid(&mut self) {
        let lines = self.lines();
        let Some(selected) = self.selected.take() else {
            self.selected = lines.into_iter().next();
            return;
        };
        if lines.contains(&selected) {
            self.selected = Some(selected);
            return;
        }
        let in_section = |target: Target| {
            let cursor = Cursor {
                section: selected.section,
                target,
            };
            lines.contains(&cursor).then_some(cursor)
        };
        let section = self.sections.get(selected.section);
        let row_fallback = match (&selected.target, section) {
            (Target::Row(row), Some(section)) if !section.rows.is_empty() => {
                let row = (*row).min(section.rows.len() - 1);
                in_section(Target::Row(row)).or_else(|| {
                    section.rows[row]
                        .group
                        .clone()
                        .and_then(|group| in_section(Target::Group(group)))
                })
            }
            _ => None,
        };
        self.selected = row_fallback
            .or_else(|| in_section(Target::Header))
            .or_else(|| {
                lines
                    .iter()
                    .find(|line| line.section == selected.section)
                    .cloned()
            })
            .or_else(|| {
                lines
                    .iter()
                    .rev()
                    .find(|line| line.section < selected.section)
                    .cloned()
            })
            .or_else(|| lines.first().cloned());
    }

    fn step(&mut self, delta: isize) {
        let lines = self.lines();
        let Some(current) = lines
            .iter()
            .position(|line| Some(line) == self.selected.as_ref())
        else {
            return;
        };
        let next = current.saturating_add_signed(delta).min(lines.len() - 1);
        self.selected = Some(lines[next].clone());
    }

    /// Selectable lines in display order: headers, group lines and the rows of expanded
    /// sections and groups.
    fn lines(&self) -> Vec<Cursor> {
        let mut lines = Vec::new();
        for (index, section) in self.sections.iter().enumerate() {
            let cursor = |target| Cursor {
                section: index,
                target,
            };
            if section.header {
                lines.push(cursor(Target::Header));
            }
            if section.collapsed {
                continue;
            }
            let mut current_group: Option<&str> = None;
            for (row, data) in section.rows.iter().enumerate() {
                let group = data.group.as_deref();
                if group != current_group {
                    current_group = group;
                    if let Some(group) = group {
                        lines.push(cursor(Target::Group(group.to_string())));
                    }
                }
                if group.is_none_or(|group| !section.is_group_collapsed(group)) {
                    lines.push(cursor(Target::Row(row)));
                }
            }
        }
        lines
    }
}

fn shown() -> bool {
    true
}
