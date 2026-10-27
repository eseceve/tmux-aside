use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use crate::config::{Mode, SectionConfig};
use crate::i18n::{LANGUAGE_VAR, Language};
use crate::protocol::{PLUGIN_PROTOCOL, PluginMessage, Row};
use crate::title;

const POLL: Duration = Duration::from_millis(20);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
const STREAM_RESTART: Duration = Duration::from_secs(1);
const STABLE_STREAM: Duration = Duration::from_secs(5);

/// New content for one section, produced by its plugin or describing its failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub section: usize,
    pub title: String,
    pub rows: Vec<Row>,
}

/// Name shown for a configured plugin: the file name for paths, the plugin name otherwise.
pub fn display_name(plugin: &str) -> String {
    Path::new(plugin)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| plugin.to_string())
}

/// Supervises the plugin of section `index` on a background thread until `stop` is set,
/// reporting every snapshot or failure through `publish`.
pub fn spawn(
    index: usize,
    section: SectionConfig,
    log: PathBuf,
    language: Language,
    stop: Arc<AtomicBool>,
    publish: impl Fn(Update) + Send + 'static,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let supervisor = Supervisor {
            index,
            name: display_name(&section.plugin),
            section,
            log,
            language,
            stop,
        };
        match supervisor.section.mode {
            Mode::Interval => supervisor.run_interval(&publish),
            Mode::Stream => supervisor.run_stream(&publish),
        }
    })
}

struct Supervisor {
    index: usize,
    name: String,
    section: SectionConfig,
    log: PathBuf,
    language: Language,
    stop: Arc<AtomicBool>,
}

impl Supervisor {
    fn run_interval(&self, publish: &impl Fn(Update)) {
        let mut failures = 0;
        while !self.stopped() {
            let update = match self.run_once() {
                Ok(update) => {
                    failures = 0;
                    update
                }
                Err(error) => {
                    failures += 1;
                    self.error(error)
                }
            };
            if self.stopped() {
                return;
            }
            publish(update);
            self.sleep(backoff(self.section.interval, failures));
        }
    }

    fn run_stream(&self, publish: &impl Fn(Update)) {
        let mut failures = 0;
        while !self.stopped() {
            let started = Instant::now();
            let error = self.stream_once(publish).unwrap_or_else(|error| error);
            if self.stopped() {
                return;
            }
            if started.elapsed() >= STABLE_STREAM {
                failures = 0;
            }
            failures += 1;
            publish(self.error(error));
            self.sleep(backoff(STREAM_RESTART, failures));
        }
    }

    fn stream_once(&self, publish: &impl Fn(Update)) -> Result<anyhow::Error> {
        let log = File::options().create(true).append(true).open(&self.log)?;
        let mut child = self.start(log)?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let (lines_tx, lines_rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if lines_tx.send(line).is_err() {
                    return;
                }
            }
        });

        let hello_deadline = Instant::now() + self.section.timeout;
        let mut greeted = false;
        loop {
            match lines_rx.recv_timeout(POLL) {
                Ok(line) if line.trim().is_empty() => {}
                Ok(line) if !greeted => {
                    if let Err(error) = expect_hello(Some(&line), self.language) {
                        kill_group(&mut child);
                        return Ok(error);
                    }
                    greeted = true;
                }
                Ok(line) => match parse_message(&line, self.language) {
                    Ok(PluginMessage::Snapshot { title, rows }) => {
                        publish(self.update(title, rows))
                    }
                    Ok(_) => {}
                    Err(error) => publish(self.error(error)),
                },
                Err(RecvTimeoutError::Timeout) => {
                    if self.stopped() {
                        drop(child.stdin.take());
                        kill_group(&mut child);
                        return Ok(anyhow!(self.language.messages().stopped));
                    }
                    if !greeted && Instant::now() >= hello_deadline {
                        kill_group(&mut child);
                        return Ok(anyhow!((self.language.messages().no_hello)(
                            &format_duration(self.section.timeout)
                        )));
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let status = child.wait()?;
                    return Ok(anyhow!("{}{}", self.describe(status), self.stderr_tail()));
                }
            }
        }
    }

    fn run_once(&self) -> Result<Update> {
        let mut child = self.start(File::create(&self.log)?)?;
        drop(child.stdin.take());
        let stdout = child.stdout.take().expect("stdout is piped");
        let (output_tx, output_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut output = String::new();
            let _ = BufReader::new(stdout).read_to_string(&mut output);
            let _ = output_tx.send(output);
        });

        let deadline = Instant::now() + self.section.timeout;
        let status = self.wait(&mut child, deadline)?;
        let output = loop {
            match output_rx.recv_timeout(POLL) {
                Ok(output) => break output,
                Err(RecvTimeoutError::Timeout) if Instant::now() < deadline && !self.stopped() => {}
                _ => {
                    kill_group(&mut child);
                    bail!(self.timed_out());
                }
            }
        };
        if !status.success() {
            bail!("{}{}", self.describe(status), self.stderr_tail());
        }
        self.parse(&output)
    }

    fn start(&self, stderr: File) -> Result<Child> {
        let executable = resolve(&self.section.plugin, self.language)?;
        let mut child = Command::new(&executable)
            .args(&self.section.args)
            .env(LANGUAGE_VAR, self.language.code())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .process_group(0)
            .spawn()
            .with_context(|| {
                (self.language.messages().failed_to_start)(&executable.display().to_string())
            })?;
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(hello_line().as_bytes());
        }
        Ok(child)
    }

    fn wait(&self, child: &mut Child, deadline: Instant) -> Result<ExitStatus> {
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline || self.stopped() {
                kill_group(child);
                bail!(self.timed_out());
            }
            thread::sleep(POLL);
        }
    }

    fn parse(&self, output: &str) -> Result<Update> {
        let mut lines = output.lines().filter(|line| !line.trim().is_empty());
        expect_hello(lines.next(), self.language)?;
        let mut update = None;
        for line in lines {
            if let PluginMessage::Snapshot { title, rows } = parse_message(line, self.language)? {
                update = Some(self.update(title, rows));
            }
        }
        update.ok_or_else(|| anyhow!(self.language.messages().no_snapshot))
    }

    fn update(&self, title: Option<String>, rows: Vec<Row>) -> Update {
        Update {
            section: self.index,
            title: match &self.section.title {
                Some(template) => title::expand(template, self.language),
                None => title.unwrap_or_else(|| self.name.clone()),
            },
            rows,
        }
    }

    fn error(&self, error: anyhow::Error) -> Update {
        self.update(None, vec![Row::new(format!("! {error:#}"))])
    }

    fn timed_out(&self) -> String {
        (self.language.messages().timed_out)(&format_duration(self.section.timeout))
    }

    fn describe(&self, status: ExitStatus) -> String {
        let messages = self.language.messages();
        match status.code() {
            Some(code) => (messages.exit_status)(code),
            None => messages.killed.to_string(),
        }
    }

    fn stderr_tail(&self) -> String {
        std::fs::read_to_string(&self.log)
            .ok()
            .and_then(|log| {
                log.lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .map(str::to_string)
            })
            .map(|line| format!(": {line}"))
            .unwrap_or_default()
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn sleep(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while !self.stopped() && Instant::now() < deadline {
            thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
        }
    }
}

pub(crate) fn resolve(plugin: &str, language: Language) -> Result<PathBuf> {
    if plugin.contains('/') {
        return Ok(PathBuf::from(plugin));
    }
    let name = format!("tmux-aside-{plugin}");
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&name)));
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join(&name));
    sibling
        .into_iter()
        .chain(on_path)
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| anyhow!((language.messages().not_found)(&name)))
}

fn hello_line() -> String {
    let hello = PluginMessage::Hello {
        protocol: PLUGIN_PROTOCOL,
        features: vec![],
    };
    format!(
        "{}\n",
        serde_json::to_string(&hello).expect("hello serializes")
    )
}

fn expect_hello(line: Option<&str>, language: Language) -> Result<()> {
    let messages = language.messages();
    match line.map(|line| parse_message(line, language)).transpose()? {
        Some(PluginMessage::Hello { protocol, .. }) if protocol == PLUGIN_PROTOCOL => Ok(()),
        Some(PluginMessage::Hello { protocol, .. }) => {
            bail!((messages.wrong_protocol)(protocol, PLUGIN_PROTOCOL))
        }
        _ => bail!(messages.expected_hello),
    }
}

fn parse_message(line: &str, language: Language) -> Result<PluginMessage> {
    serde_json::from_str(line).map_err(|_| {
        let preview: String = line.chars().take(40).collect();
        anyhow!((language.messages().invalid_message)(&preview))
    })
}

fn kill_group(child: &mut Child) {
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{}", child.id())])
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

fn backoff(interval: Duration, failures: u32) -> Duration {
    if failures == 0 {
        return interval;
    }
    let factor = 1u32 << (failures - 1).min(16);
    interval
        .saturating_mul(factor)
        .min(MAX_BACKOFF.max(interval))
}

fn format_duration(duration: Duration) -> String {
    if duration.subsec_millis() != 0 {
        format!("{}ms", duration.as_millis())
    } else if duration.as_secs().is_multiple_of(60) {
        format!("{}m", duration.as_secs() / 60)
    } else {
        format!("{}s", duration.as_secs())
    }
}
