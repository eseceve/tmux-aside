//! Bounded decoding of Codex's OSC 7501 status, independent of terminal rendering.
use serde::{Deserialize, Serialize};

use crate::Status;

/// Advisory state emitted by a terminal client, not proof of a conversation's identity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    #[default]
    Unknown,
    Idle,
    Working,
    Waiting,
    Done,
    Error,
}

impl TerminalStatus {
    /// The sidebar status; idle, errors and missing evidence must not imply completion.
    pub fn status(self) -> Option<Status> {
        match self {
            Self::Working => Some(Status::Working),
            Self::Waiting => Some(Status::Waiting),
            Self::Done => Some(Status::Done),
            _ => None,
        }
    }
}

/// Last observation of one client process, refreshed only while its capture is connected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalObservation {
    pub state: TerminalStatus,
    pub process_id: u32,
    pub server: String,
    pub revision: u64,
    pub checked_at: u64,
}

/// Streaming parser retaining at most 256 bytes, never a terminal transcript.
#[derive(Default)]
pub struct OscParser {
    buffer: Vec<u8>,
    escape: bool,
    osc: bool,
}

impl OscParser {
    /// Consume arbitrary chunks, returning only complete, valid Codex status records.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<TerminalStatus> {
        let mut found = Vec::new();
        for &byte in bytes {
            if self.escape {
                self.escape = false;
                if self.osc && byte == b'\\' {
                    if let Some(state) = decode(&self.buffer) {
                        found.push(state);
                    }
                    self.osc = false;
                    self.buffer.clear();
                    continue;
                }
                self.osc = byte == b']';
                self.buffer.clear();
                if byte == 0x1b {
                    self.escape = true;
                }
                continue;
            }
            if byte == 0x1b {
                self.escape = true;
            } else if self.osc && byte == 7 {
                if let Some(state) = decode(&self.buffer) {
                    found.push(state);
                }
                self.osc = false;
                self.buffer.clear();
            } else if self.osc {
                if self.buffer.len() == 256 {
                    self.osc = false;
                    self.buffer.clear();
                } else {
                    self.buffer.push(byte);
                }
            }
        }
        found
    }
}

fn decode(bytes: &[u8]) -> Option<TerminalStatus> {
    let text = std::str::from_utf8(bytes).ok()?.strip_prefix("7501;")?;
    // Codex's cleanup record intentionally has no app field.
    if text == "state=clear" {
        return Some(TerminalStatus::Unknown);
    }
    let mut state = None;
    let mut app = None;
    for field in text.split(':') {
        let (key, value) = field.split_once('=')?;
        match key {
            "state" if state.is_none() => state = Some(value),
            "app" if app.is_none() => app = Some(value),
            "kind" => {}
            _ => return None,
        }
    }
    if app != Some("codex") {
        return None;
    }
    match state? {
        "idle" => Some(TerminalStatus::Idle),
        "working" => Some(TerminalStatus::Working),
        "blocked" => Some(TerminalStatus::Waiting),
        "done" => Some(TerminalStatus::Done),
        "error" => Some(TerminalStatus::Error),
        _ => None,
    }
}
