use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use tmux_aside::i18n::Language;
use tmux_aside::setup::{self, Action, Ai};
use tmux_aside::width::Width;
use tmux_aside::{cleanup, daemon, launch, render, sidebar};

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
    /// Move the focus to the sidebar of the current window, if it is shown
    Focus,
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
        /// Key that moves the focus to the sidebar
        #[arg(long, allow_hyphen_values = true)]
        focus: Option<String>,
        /// Key that jumps to the agent waiting the longest, or done when none waits
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
        /// Install the /tmux-aside-launch and /tmux-aside-qa skills for each chosen AI
        #[arg(long, conflicts_with = "no_skills")]
        skills: bool,
        /// Install no skills
        #[arg(long)]
        no_skills: bool,
        /// Use the Pack at this path, unzip it from this .zip (replacing the one unzipped before),
        /// or clone it from this git URL (pulled again on each run)
        #[arg(long, value_name = "PATH_ZIP_OR_URL")]
        pack: Option<String>,
        /// Take the default of everything not given, without asking
        #[arg(long)]
        yes: bool,
    },
    /// Undo what setup added: key bindings, AI hooks, skills and the pack key
    Uninstall {
        /// tmux config setup wrote to [default: the one tmux reads, or ~/.tmux.conf]
        #[arg(long)]
        config: Option<PathBuf>,
        /// Also delete a Pack tmux-aside cloned or unzipped, without asking
        #[arg(long)]
        delete_pack: bool,
        /// Take the default of everything not given, without asking (keeps a cloned or unzipped Pack)
        #[arg(long)]
        yes: bool,
    },
    /// Start an agent on a task in its own branch, worktree and background window
    Launch {
        #[command(flatten)]
        agent: launch::AgentOptions,
        /// Path to the repository clone
        repository: PathBuf,
        /// Template the agent is launched with
        #[arg(long)]
        template: String,
        /// Task reference filled into the template: a URL, an issue, a sentence [default: the
        /// pull request's URL with --pr]
        #[arg(long, required_unless_present = "pr")]
        task: Option<String>,
        /// Branch to create
        #[arg(long, required_unless_present = "pr", conflicts_with = "pr")]
        branch: Option<String>,
        /// Where the new branch starts [default: origin/HEAD, or HEAD without one]
        #[arg(long, conflicts_with = "pr")]
        base: Option<String>,
        /// Adopt an existing pull request: check out its branch instead of creating one
        #[arg(long, value_name = "URL")]
        pr: Option<String>,
        /// Group whose pull requests are shown and tested together
        #[arg(long)]
        group: Option<String>,
    },
    /// Launch one QA agent, with a fresh context and the qa template, for these pull requests
    Qa {
        #[command(flatten)]
        agent: launch::AgentOptions,
        /// URLs of the pull requests, each followed by a launch
        #[arg(required = true)]
        urls: Vec<String>,
    },
    /// Run the on_deployed action, an AI with the deployed template, for these deployed pull
    /// requests
    Deployed {
        /// URLs of the pull requests
        #[arg(required = true)]
        urls: Vec<String>,
        /// Task reference of a pull request, for ${task} in the template [default: the URLs]
        #[arg(long = "task", value_name = "TASK")]
        tasks: Vec<String>,
    },
    /// Launch an agent that fixes what QA found in a pull request, in its repository and Group
    Fix {
        #[command(flatten)]
        agent: launch::AgentOptions,
        /// URL of the pull request whose QA failed, followed by a launch
        url: String,
    },
    /// Record the AI agent in this pane as the agent of a pull request, so it gets its Nudges
    Follow {
        /// URL of the pull request
        url: String,
    },
    /// Reopen the agent of every launch whose window is gone, such as after tmux or the
    /// computer restarted, resuming its conversation
    Resurrect {
        /// Only list the launches it would reopen
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a launch: its worktree and local branch, its record and its agent's pane
    Remove {
        /// Id, branch, pull request URL or worktree path of the launch, or the path of a worktree no
        /// launch follows [default: the launch of the current pane]
        launch: Option<String>,
        /// Remove the worktree even with uncommitted changes or unpushed commits
        #[arg(long)]
        force: bool,
        /// Keep the local branch
        #[arg(long)]
        keep_branch: bool,
    },
    /// Remove the worktrees next to these clones that no launch follows, once their pull
    /// request was merged or closed or their remote branch is gone
    Prune {
        /// Clones, worktrees or directories of clones [default: the current directory]
        paths: Vec<PathBuf>,
        /// Remove worktrees even with uncommitted changes or unpushed commits
        #[arg(long)]
        force: bool,
        /// Keep the local branches
        #[arg(long)]
        keep_branch: bool,
        /// Only list the worktrees it would remove
        #[arg(long)]
        dry_run: bool,
    },
    /// List every launch with its worktree, window, agent and pull request
    Status {
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    #[command(name = "_agent", hide = true)]
    Agent {
        #[arg(long, value_enum)]
        ai: AiArg,
        #[arg(long)]
        program: PathBuf,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        effort: Option<String>,
        prompt: Option<PathBuf>,
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
    Agy,
}

impl From<AiArg> for Ai {
    fn from(ai: AiArg) -> Self {
        match ai {
            AiArg::Claude => Ai::Claude,
            AiArg::Codex => Ai::Codex,
            AiArg::Agy => Ai::Agy,
        }
    }
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::On { width } => sidebar::on(width),
        Command::Off => sidebar::off(),
        Command::Toggle { width } => sidebar::toggle(width),
        Command::Focus => sidebar::focus(),
        Command::Reload => reload(),
        Command::Setup {
            config,
            toggle,
            focus,
            next,
            reload,
            hooks,
            no_hooks,
            skills,
            no_skills,
            pack,
            yes,
        } => setup::run(setup::Options {
            config,
            keys: [
                (Action::Toggle, toggle),
                (Action::Focus, focus),
                (Action::Next, next),
                (Action::Reload, reload),
            ]
            .into_iter()
            .filter_map(|(action, key)| key.map(|key| (action, key)))
            .collect(),
            ais: if no_hooks {
                Some(vec![])
            } else {
                hooks.map(|ais| ais.into_iter().map(Ai::from).collect())
            },
            pack,
            skills: match (skills, no_skills) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            },
            defaults: yes,
        }),
        Command::Uninstall {
            config,
            delete_pack,
            yes,
        } => setup::uninstall(setup::UninstallOptions {
            config,
            delete_pack: delete_pack.then_some(true),
            defaults: yes,
        }),
        Command::Launch {
            agent,
            repository,
            template,
            task,
            branch,
            base,
            pr,
            group,
        } => {
            let language = Language::detect();
            let source = match (pr, branch) {
                (Some(url), _) => launch::Source::PullRequest(url),
                (None, Some(name)) => launch::Source::Branch { name, base },
                (None, None) => bail!(language.messages().branch_required),
            };
            let launch = launch::run(
                launch::Options {
                    agent,
                    repository,
                    template,
                    task,
                    source,
                    group,
                },
                language,
            )?;
            println!("{}", launch::announce(&launch, language));
            Ok(())
        }
        Command::Qa { urls, agent } => {
            let language = Language::detect();
            let launch = launch::qa_with_options(&urls, agent, language)?;
            println!("{}", launch::announce(&launch, language));
            Ok(())
        }
        Command::Deployed { urls, tasks } => launch::deployed(&urls, &tasks, Language::detect()),
        Command::Fix { url, agent } => {
            let language = Language::detect();
            let launch = launch::fix_with_options(&url, agent, language)?;
            println!("{}", launch::announce(&launch, language));
            Ok(())
        }
        Command::Follow { url } => {
            let language = Language::detect();
            let launch = launch::follow(&url, language)?;
            println!("{}", launch::announce_followed(&launch, language));
            Ok(())
        }
        Command::Resurrect { dry_run } => {
            println!("{}", launch::resurrect(dry_run, Language::detect())?);
            Ok(())
        }
        Command::Remove {
            launch,
            force,
            keep_branch,
        } => {
            let options = cleanup::Options { force, keep_branch };
            println!(
                "{}",
                cleanup::remove(launch.as_deref(), options, Language::detect())?
            );
            Ok(())
        }
        Command::Prune {
            paths,
            force,
            keep_branch,
            dry_run,
        } => {
            let options = cleanup::Options { force, keep_branch };
            println!(
                "{}",
                cleanup::prune(&paths, options, dry_run, Language::detect())?
            );
            Ok(())
        }
        Command::Status { json } => {
            let launches = launch::list()?;
            let progress = launch::progress();
            if json {
                println!("{}", launch::as_json(&launches, &progress)?);
            } else {
                print!(
                    "{}",
                    launch::describe(&launches, &progress, Language::detect())
                );
            }
            Ok(())
        }
        Command::Agent {
            ai,
            program,
            model,
            effort,
            prompt,
        } => launch::start_agent(
            launch::AgentSettings {
                ai: ai.into(),
                model,
                effort,
            },
            &program,
            prompt.as_deref(),
        ),
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
        bail!("tmux-aside is not running; start it with `tmux-aside on`");
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
