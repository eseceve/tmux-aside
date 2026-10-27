use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tmux_aside::protocol::DaemonMessage;
use tmux_aside::view::View;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// Scratch directory with an isolated XDG config and state home.
pub struct Home {
    pub dir: PathBuf,
}

impl Home {
    pub fn new(kind: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tmux-aside-{kind}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(dir.join("config/tmux-aside")).unwrap();
        Home { dir }
    }

    pub fn write_config(&self, yaml: &str) {
        std::fs::write(self.dir.join("config/tmux-aside/config.yaml"), yaml).unwrap();
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn envs(&self) -> [(&'static str, PathBuf); 2] {
        [
            ("XDG_CONFIG_HOME", self.dir.join("config")),
            ("XDG_STATE_HOME", self.dir.join("state")),
        ]
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

pub struct Daemon {
    pub child: Child,
    pub socket: PathBuf,
    pub home: Home,
}

impl Daemon {
    pub fn start(idle_timeout_ms: u64) -> Self {
        Self::start_in(Home::new("daemon"), idle_timeout_ms)
    }

    pub fn with_config(yaml: &str) -> Self {
        let home = Home::new("daemon");
        home.write_config(yaml);
        Self::start_in(home, 10_000)
    }

    pub fn start_in(home: Home, idle_timeout_ms: u64) -> Self {
        let socket = home.path("aside.sock");
        let child = daemon_command(&home, &socket)
            .args(["--idle-timeout-ms", &idle_timeout_ms.to_string()])
            .spawn()
            .expect("failed to spawn daemon");
        wait_for(&format!("socket {}", socket.display()), || socket.exists());
        Daemon {
            child,
            socket,
            home,
        }
    }

    pub fn connect(&self) -> Client {
        Client::connect(&self.socket)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn daemon_command(home: &Home, socket: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tmux-aside"));
    command
        .args(["_daemon", "--socket"])
        .arg(socket)
        .env_remove("TMUX")
        .env("TMUX_ASIDE_LANG", "en")
        .envs(home.envs())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    pub fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).expect("failed to connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        Client {
            writer: stream.try_clone().unwrap(),
            reader: BufReader::new(stream),
        }
    }

    pub fn next_message(&mut self) -> DaemonMessage {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .expect("no message received");
        serde_json::from_str(&line).expect("invalid message")
    }

    pub fn next_view(&mut self) -> View {
        loop {
            if let DaemonMessage::Frame { view } = self.next_message() {
                return view;
            }
        }
    }

    /// Reads frames until one satisfies `condition`, failing after 5 seconds.
    pub fn view_where(&mut self, what: &str, condition: impl Fn(&View) -> bool) -> View {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let view = self.next_view();
            if condition(&view) {
                return view;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; last view: {view:?}"
            );
        }
    }

    pub fn send(&mut self, line: &str) {
        writeln!(self.writer, "{line}").unwrap();
    }
}

pub fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
