use serde::{Deserialize, Serialize};

use crate::view::{Cursor, Key, View};

/// Message a sidebar client sends to the daemon, one JSON object per line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientMessage {
    Key {
        key: Key,
    },
    /// The client's pane gained focus in tmux `window`.
    Focus {
        window: String,
    },
    /// Reread the config and restart every plugin.
    Reload,
    /// A line was clicked: selects it and activates it.
    Click {
        cursor: Cursor,
    },
    /// A key without a sidebar meaning was pressed; runs the selected row's action for it.
    Action {
        key: String,
    },
}

/// Message the daemon broadcasts to every sidebar client, one JSON object per line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DaemonMessage {
    Frame {
        view: View,
    },
    /// Command the receiving client runs from its own pane, answering an action key.
    Run {
        command: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
}

/// Protocol version spoken between the daemon and plugins.
pub const PLUGIN_PROTOCOL: u32 = 1;

/// Message exchanged with a plugin over its stdin/stdout, one JSON object per line.
///
/// The first line in each direction is a `hello`. Unknown message types and fields are
/// ignored so later protocol versions can add them without breaking older plugins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PluginMessage {
    Hello {
        protocol: u32,
        #[serde(default)]
        features: Vec<String>,
    },
    Snapshot {
        #[serde(default)]
        title: Option<String>,
        rows: Vec<Row>,
    },
    #[serde(other)]
    Unknown,
}

/// One line of a section: `text` on the left and an optional `detail` aligned to the right.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub text: String,
    /// Text aligned to the right; `spinner` is animated by the sidebar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub align: Align,
    #[serde(default, skip_serializing_if = "is_default")]
    pub tone: Tone,
    #[serde(default, skip_serializing_if = "is_default")]
    pub detail_tone: Tone,
    /// One-column glyph drawn before the text; `spinner` is animated by the sidebar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub icon_tone: Tone,
    /// Consecutive rows sharing a group are shown under one group line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    /// Extra lines drawn muted below `text`, aligned with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    /// Color of a bar drawn at the left of every line of the row; `plain` draws none.
    #[serde(default, skip_serializing_if = "is_default")]
    pub bar_tone: Tone,
    /// tmux window id the row belongs to; the sidebar shown in that window highlights it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// Percent from 0 to 100 drawn as a bar between the text and the detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meter: Option<u8>,
    /// Color of the filled part of the meter.
    #[serde(default, skip_serializing_if = "is_default")]
    pub meter_tone: Tone,
}

/// Command the sidebar runs when `key` is pressed on the row, from the sidebar's own pane.
///
/// `key` is `enter` or a single character. The sidebar keeps `j`, `k`, `h`, `l`, `o`, `?` and
/// space for itself, so actions bound to them never run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub key: String,
    pub command: Vec<String>,
    /// What the action does, shown in the sidebar's help.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Directory the command runs in; the sidebar's own directory when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

impl Action {
    pub fn new(key: &str, command: &[&str]) -> Self {
        Action {
            key: key.to_string(),
            command: command.iter().map(|part| part.to_string()).collect(),
            label: None,
            cwd: None,
        }
    }
}

/// Horizontal placement of a row's text. Unknown values fall back to `left`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Center,
    #[default]
    #[serde(other)]
    Left,
}

/// Semantic color of a piece of text; the user's theme maps each tone to a color.
/// Unknown values fall back to `plain`, the terminal's default color.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    Muted,
    Accent,
    Success,
    Warning,
    Danger,
    #[default]
    #[serde(other)]
    Plain,
}

impl Row {
    pub fn new(text: impl Into<String>) -> Self {
        Row {
            text: text.into(),
            detail: None,
            align: Align::Left,
            tone: Tone::Plain,
            detail_tone: Tone::Plain,
            icon: None,
            icon_tone: Tone::Plain,
            group: None,
            actions: Vec::new(),
            lines: Vec::new(),
            bar_tone: Tone::Plain,
            window: None,
            meter: None,
            meter_tone: Tone::Plain,
        }
    }

    /// Command bound to `key` on this row, if any.
    pub fn action(&self, key: &str) -> Option<&[String]> {
        self.find_action(key)
            .map(|action| action.command.as_slice())
    }

    /// Action bound to `key` on this row, if any.
    pub fn find_action(&self, key: &str) -> Option<&Action> {
        self.actions.iter().find(|action| action.key == key)
    }

    pub fn with_detail(text: impl Into<String>, detail: impl Into<String>) -> Self {
        Row {
            detail: Some(detail.into()),
            ..Row::new(text)
        }
    }
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}
