use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use tmux_aside::config::{parse_duration, state_dir};
use tmux_aside::i18n::Language;
use tmux_aside::protocol::{PLUGIN_PROTOCOL, PluginMessage};
use tmux_aside_usage::{
    claude_windows, codex_windows, messages, rows, statusline_command, wrapped_statusline,
};

const CODEX_TIMEOUT: Duration = Duration::from_secs(20);

/// Sidebar section showing how much of the Claude and Codex plan limits is used.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// How long Codex limits are cached before asking the Codex backend again
    #[arg(long, default_value = "10m")]
    codex_interval: String,
    /// Only show tools with an agent running in a tmux pane
    #[arg(long)]
    only_running: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Claude Code statusline wrapper: records the rate limits it receives, then runs ORIGINAL
    Statusline { original: String },
    /// Wrap a tool's statusline so its limits are recorded
    Setup {
        #[command(subcommand)]
        tool: SetupTool,
    },
}

#[derive(Subcommand)]
enum SetupTool {
    /// Wrap the statusLine command in Claude Code's settings.json
    Claude,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Statusline { original }) => return statusline(&original),
        Some(Command::Setup {
            tool: SetupTool::Claude,
        }) => setup_claude(),
        None => parse_duration(&cli.codex_interval)
            .and_then(|interval| run_plugin(interval, cli.only_running)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn statusline(original: &str) -> ExitCode {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    if let Some(rate_limits) = serde_json::from_slice::<Value>(&input)
        .ok()
        .and_then(|status| status.get("rate_limits").cloned())
    {
        let _ = write_cache(
            "claude-limits.json",
            &json!({ "rate_limits": rate_limits, "captured_at": now() }),
        );
    }
    let child = Process::new("sh")
        .args(["-c", original])
        .stdin(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        return ExitCode::FAILURE;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&input);
    }
    match child.wait() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

fn run_plugin(codex_interval: Duration, only_running: bool) -> Result<()> {
    let now = now();
    let language = Language::from_env();
    let running = only_running.then(running_commands);
    let shown = |command: &str| {
        running
            .as_ref()
            .is_none_or(|running| running.iter().any(|name| name == command))
    };
    let mut tools = Vec::new();
    if shown("claude") {
        let claude = read_cache("claude-limits.json")
            .and_then(|cache| cache.get("rate_limits").cloned())
            .map(|rate_limits| claude_windows(&rate_limits, now, language))
            .unwrap_or_default();
        tools.push(("Claude".to_string(), claude));
    }
    if shown("codex") {
        let codex = codex_limits(codex_interval, now)
            .map(|result| codex_windows(&result, now, language))
            .unwrap_or_default();
        tools.push(("Codex".to_string(), codex));
    }
    let rows = rows(&tools, now, language);
    print(&PluginMessage::Hello {
        protocol: PLUGIN_PROTOCOL,
        features: vec![],
    })?;
    print(&PluginMessage::Snapshot {
        title: Some(messages(language).title.to_string()),
        rows,
    })
}

/// Commands running in the panes of the tmux server.
fn running_commands() -> Vec<String> {
    Process::new("tmux")
        .args(["list-panes", "-a", "-F", "#{pane_current_command}"])
        .output()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn codex_limits(interval: Duration, now: u64) -> Option<Value> {
    let cached = read_cache("codex-limits.json");
    let fresh = cached
        .as_ref()
        .and_then(|cache| cache.get("fetched_at")?.as_u64())
        .is_some_and(|fetched_at| now.saturating_sub(fetched_at) < interval.as_secs());
    if fresh {
        return cached?.get("result").cloned();
    }
    match fetch_codex_limits() {
        Ok(result) => {
            let _ = write_cache(
                "codex-limits.json",
                &json!({ "result": result, "fetched_at": now }),
            );
            Some(result)
        }
        Err(error) => {
            eprintln!("codex limits: {error:#}");
            cached?.get("result").cloned()
        }
    }
}

fn fetch_codex_limits() -> Result<Value> {
    let mut child = Process::new("codex")
        .args(["-s", "read-only", "-a", "never", "app-server"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start codex app-server")?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let stdout = child.stdout.take().expect("stdout is piped");
    let (sender, responses) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(message) = serde_json::from_str::<Value>(&line)
                && message.get("id") == Some(&json!(2))
            {
                let _ = sender.send(message);
                return;
            }
        }
    });
    for message in [
        json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "tmux-aside", "version": env!("CARGO_PKG_VERSION")}}}),
        json!({"method": "initialized"}),
        json!({"id": 2, "method": "account/rateLimits/read"}),
    ] {
        writeln!(stdin, "{message}")?;
    }
    let response = responses.recv_timeout(CODEX_TIMEOUT);
    let _ = child.kill();
    let _ = child.wait();
    let response = response.map_err(|_| anyhow!("codex app-server did not answer"))?;
    if let Some(error) = response.get("error") {
        bail!("codex app-server: {error}");
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("codex app-server returned no result"))
}

fn setup_claude() -> Result<()> {
    let dir = match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => std::env::home_dir()
            .ok_or_else(|| anyhow!("cannot locate the home directory"))?
            .join(".claude"),
    };
    let path = dir.join("settings.json");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let mut settings: Value = serde_json::from_str(&content)
        .with_context(|| format!("{} is not valid JSON; nothing was changed", path.display()))?;
    let command = settings
        .pointer("/statusLine/command")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("{} has no statusLine command to wrap", path.display()))?;
    let original = wrapped_statusline(command).unwrap_or_else(|| command.to_string());
    let exe = std::env::current_exe()?;
    let exe = exe
        .to_str()
        .ok_or_else(|| anyhow!("executable path is not UTF-8"))?;
    settings["statusLine"]["command"] = json!(statusline_command(exe, &original));
    std::fs::write(path.with_extension("json.bak"), &content)?;
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&settings)?),
    )?;
    println!("Claude Code statusline wrapped in {}", path.display());
    Ok(())
}

fn cache_path(name: &str) -> Result<PathBuf> {
    Ok(state_dir()?.join(name))
}

fn read_cache(name: &str) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(cache_path(name).ok()?).ok()?).ok()
}

fn write_cache(name: &str, value: &Value) -> Result<()> {
    let path = cache_path(name)?;
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let temporary = path.with_extension(format!("json.{}", std::process::id()));
    std::fs::write(&temporary, value.to_string())?;
    std::fs::rename(&temporary, &path)?;
    Ok(())
}

fn print(message: &PluginMessage) -> Result<()> {
    println!("{}", serde_json::to_string(message)?);
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}
