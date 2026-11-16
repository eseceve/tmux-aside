//! Own only our pipe reader: never replace or close another consumer's tmux pipe.
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tmux_aside::tmux;
use tmux_aside_agents::LivePane;
use tmux_aside_agents::terminal::{OscParser, TerminalObservation, TerminalStatus};

pub struct Captures {
    directory: PathBuf,
    server: String,
    next: u64,
    panes: HashMap<String, Capture>,
}

struct Capture {
    process_id: u32,
    pane_pid: u32,
    path: PathBuf,
    listener: UnixListener,
    stream: Option<UnixStream>,
    receiver: Option<Receiver<Option<TerminalStatus>>>,
    state: TerminalStatus,
    revision: u64,
    started: u64,
}

impl Drop for Capture {
    fn drop(&mut self) {
        if let Some(stream) = &self.stream {
            let _ = stream.shutdown(Shutdown::Both);
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Captures {
    pub fn new() -> Result<Self> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        // Keep Unix socket paths short even on macOS, where TMPDIR can be very long.
        let directory = PathBuf::from(format!("/tmp/ta-osc-{}-{nonce:x}", std::process::id()));
        std::fs::create_dir(&directory)?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        let server = tmux::run(&["display-message", "-p", "#{socket_path}"])?;
        Ok(Self {
            directory,
            server,
            next: 0,
            panes: HashMap::new(),
        })
    }

    pub fn poll(&mut self, live: &[LivePane], now: u64) -> HashMap<String, TerminalObservation> {
        self.panes.retain(|pane, capture| {
            live.iter().any(|live| {
                live.pane_id == *pane
                    && live.command == "codex"
                    && live.pane_pid == capture.pane_pid
                    && live.agent_pid == Some(capture.process_id)
            })
        });
        let mut snapshots = HashMap::new();
        let mut disconnected = Vec::new();
        for pane in live.iter().filter(|pane| pane.command == "codex") {
            if !self.panes.contains_key(&pane.pane_id)
                && let Some(pid) = pane.agent_pid
            {
                // The tmux-side condition in attach also guards against a newly installed pipe.
                if tmux::run(&["display-message", "-p", "-t", &pane.pane_id, "#{pane_pipe}"])
                    .is_ok_and(|pipe| pipe == "0")
                {
                    match self.attach(pane, pid, now) {
                        Ok(capture) => {
                            self.panes.insert(pane.pane_id.clone(), capture);
                        }
                        Err(error) => eprintln!("cannot observe {}: {error:#}", pane.pane_id),
                    }
                }
            }
            let mut observation = TerminalObservation {
                state: TerminalStatus::Unknown,
                process_id: pane.agent_pid.unwrap_or_default(),
                server: self.server.clone(),
                revision: 0,
                checked_at: now,
            };
            if let Some(capture) = self.panes.get_mut(&pane.pane_id) {
                if capture.stream.is_none() {
                    match capture.listener.accept() {
                        Ok((stream, _)) => match stream
                            .set_nonblocking(false)
                            .and_then(|()| stream.try_clone())
                        {
                            Ok(reader) => {
                                let (send, receive) = mpsc::sync_channel(64);
                                std::thread::spawn(move || {
                                    for line in BufReader::new(reader).lines() {
                                        let Ok(line) = line else {
                                            break;
                                        };
                                        let Ok(state) = serde_json::from_str(&line) else {
                                            break;
                                        };
                                        if send.send(Some(state)).is_err() {
                                            return;
                                        }
                                    }
                                    let _ = send.send(None);
                                });
                                capture.stream = Some(stream);
                                capture.receiver = Some(receive);
                            }
                            Err(_) => disconnected.push(pane.pane_id.clone()),
                        },
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(_) => disconnected.push(pane.pane_id.clone()),
                    }
                }
                if let Some(receiver) = &capture.receiver {
                    for event in receiver.try_iter() {
                        capture.revision += 1;
                        capture.state = event.unwrap_or_default();
                        if event.is_none() {
                            disconnected.push(pane.pane_id.clone());
                        }
                    }
                } else if now.saturating_sub(capture.started) >= 3 {
                    disconnected.push(pane.pane_id.clone());
                }
                observation.state = capture.state;
                observation.revision = capture.revision;
            }
            snapshots.insert(pane.pane_id.clone(), observation);
        }
        for pane in disconnected {
            self.panes.remove(&pane);
            if let Some(snapshot) = snapshots.get_mut(&pane) {
                snapshot.state = TerminalStatus::Unknown;
            }
        }
        snapshots
    }

    fn attach(&mut self, pane: &LivePane, process_id: u32, now: u64) -> Result<Capture> {
        self.next += 1;
        let path = self.directory.join(self.next.to_string());
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        let capture = Capture {
            process_id,
            pane_pid: pane.pane_pid,
            path,
            listener,
            stream: None,
            receiver: None,
            state: TerminalStatus::Unknown,
            revision: 0,
            started: now,
        };
        let executable = std::env::current_exe()?;
        let command = format!(
            "{} _observe {}",
            shell_quote(&executable.to_string_lossy()),
            shell_quote(&capture.path.to_string_lossy())
        );
        // Do not use pipe-pane -o: it toggles an existing pipe off.
        let pipe = format!(
            "pipe-pane -O -t {} {}",
            shell_quote(&pane.pane_id),
            shell_quote(&command)
        );
        tmux::run(&[
            "if-shell",
            "-F",
            "-t",
            &pane.pane_id,
            "#{==:#{pane_pipe},0}",
            &pipe,
        ])?;
        Ok(capture)
    }
}

impl Drop for Captures {
    fn drop(&mut self) {
        self.panes.clear();
        let _ = std::fs::remove_dir(&self.directory);
    }
}

pub fn observe(path: &Path) -> Result<()> {
    let mut stream = UnixStream::connect(path)?;
    // Parent death/disconnection closes this reader even while the pane is silent.
    let mut lifetime = stream.try_clone()?;
    std::thread::spawn(move || {
        let _ = lifetime.read(&mut [0]);
        std::process::exit(0);
    });
    let mut parser = OscParser::default();
    let mut input = std::io::stdin().lock();
    let mut buffer = [0; 4096];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            return Ok(());
        }
        for state in parser.feed(&buffer[..count]) {
            serde_json::to_writer(&mut stream, &state)?;
            stream.write_all(b"\n")?;
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
