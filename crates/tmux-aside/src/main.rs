use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use tmux_aside::setup::{self, Action, Ai};
use tmux_aside::width::Width;
use tmux_aside::{daemon, render, sidebar};

#[derive(Parser)]
#[command(version, about = "A tmux sidebar with pluggable sections")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the sidebar in every window
    On {
        /// Width in columns (N) or percent of the window (N%)
        #[arg(long)]
        width: Option<Width>,
    },
    /// Remove the sidebar from every window
    Off,
    /// Turn the sidebar off if it is shown, on otherwise
    Toggle {
        /// Width in columns (N) or percent of the window (N%)
        #[arg(long)]
        width: Option<Width>,
    },
    /// Reread the config and restart every plugin
    Reload,
    /// Bind keys in the tmux config and install the AI hooks, asking for each
    Setup {
        /// tmux config to write [default: the one tmux reads, or ~/.tmux.conf]
        #[arg(long)]
        config: Option<PathBuf>,
        /// Key that toggles the sidebar, as written after bind-key ("-" leaves it unbound)
        #[arg(long, allow_hyphen_values = true)]
        toggle: Option<String>,
        /// Key that jumps to the agent waiting the longest
        #[arg(long, allow_hyphen_values = true)]
        next: Option<String>,
        /// Key that reloads the config
        #[arg(long, allow_hyphen_values = true)]
        reload: Option<String>,
        /// AIs whose hooks to install
        #[arg(long, value_delimiter = ',', conflicts_with = "no_hooks")]
        hooks: Option<Vec<AiArg>>,
        /// Install no AI hooks
        #[arg(long)]
        no_hooks: bool,
        /// Take the default of everything not given, without asking
        #[arg(long)]
        yes: bool,
    },
    #[command(name = "_sync", hide = true)]
    Sync {
        #[arg(long)]
        window: String,
    },
    #[command(name = "_select", hide = true)]
    Select {
        #[arg(long)]
        window: String,
        #[arg(last = true)]
        fallback: Vec<String>,
    },
    #[command(name = "_reap", hide = true)]
    Reap,
    #[command(name = "_render", hide = true)]
    Render {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    #[command(name = "_daemon", hide = true)]
    Daemon {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long, default_value_t = 10_000)]
        idle_timeout_ms: u64,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum AiArg {
    Claude,
    Codex,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::On { width } => sidebar::on(width),
        Command::Off => sidebar::off(),
        Command::Toggle { width } => sidebar::toggle(width),
        Command::Reload => reload(),
        Command::Setup {
            config,
            toggle,
            next,
            reload,
            hooks,
            no_hooks,
            yes,
        } => setup::run(setup::Options {
            config,
            keys: [
                (Action::Toggle, toggle),
                (Action::Next, next),
                (Action::Reload, reload),
            ]
            .into_iter()
            .filter_map(|(action, key)| key.map(|key| (action, key)))
            .collect(),
            ais: if no_hooks {
                Some(vec![])
            } else {
                hooks.map(|ais| {
                    ais.into_iter()
                        .map(|ai| match ai {
                            AiArg::Claude => Ai::Claude,
                            AiArg::Codex => Ai::Codex,
                        })
                        .collect()
                })
            },
            defaults: yes,
        }),
        Command::Sync { window } => sidebar::sync_window(&window),
        Command::Reap => sidebar::reap(),
        Command::Select { window, fallback } => sidebar::select(&window, &fallback),
        Command::Render { socket } => render::run(socket),
        Command::Daemon {
            socket,
            idle_timeout_ms,
        } => run_daemon(socket, Duration::from_millis(idle_timeout_ms)),
    }
}

fn reload() -> Result<()> {
    if !daemon::reload(&daemon::default_socket()?)? {
        eprintln!("tmux-aside is not running");
    }
    Ok(())
}

fn run_daemon(socket: Option<PathBuf>, idle_timeout: Duration) -> Result<()> {
    let socket = match socket {
        Some(socket) => socket,
        None => daemon::default_socket()?,
    };
    daemon::run(&socket, idle_timeout)
}
