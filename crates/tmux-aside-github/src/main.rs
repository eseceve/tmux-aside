use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};
use clap::Parser;
use jiff::Timestamp;
use tmux_aside::protocol::{PLUGIN_PROTOCOL, PluginMessage};
use tmux_aside_github::{Part, RowOptions, SEARCH_QUERY, parse_search, rows};

/// Sidebar section listing GitHub pull requests that match a search.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// GitHub search selecting the pull requests; add `repo:owner/name` qualifiers to watch
    /// only some repositories
    #[arg(long, default_value = "is:pr is:open author:@me archived:false")]
    query: String,
    /// Section title
    #[arg(long, default_value = "Pull requests")]
    title: String,
    /// Maximum number of pull requests
    #[arg(long, default_value_t = 50)]
    limit: u32,
    /// Part of each row colored by the pull request's state; repeat for several
    #[arg(long, value_enum, default_values_t = [Part::Icon, Part::Number])]
    state_color: Vec<Part>,
    /// Add a line with each pull request's author and age
    #[arg(long)]
    byline: bool,
    /// Add a line with each pull request's unresolved review threads and approvals
    #[arg(long)]
    merge_info: bool,
    /// Show the lines each pull request adds and removes, as +N -M
    #[arg(long)]
    diff: bool,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let output = Command::new("gh")
        .args(["api", "graphql", "-f"])
        .arg(format!("query={SEARCH_QUERY}"))
        .arg("-f")
        .arg(format!("q={}", cli.query))
        .arg("-F")
        .arg(format!("limit={}", cli.limit))
        .output()
        .context("failed to run gh")?;
    if !output.status.success() {
        bail!("gh: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    let pull_requests = parse_search(&String::from_utf8_lossy(&output.stdout))?;
    let rows = rows(
        &pull_requests,
        &RowOptions {
            colored: cli.state_color,
            byline_at: cli.byline.then(Timestamp::now),
            merge_info: cli.merge_info,
            diff: cli.diff,
        },
    );
    print(&PluginMessage::Hello {
        protocol: PLUGIN_PROTOCOL,
        features: vec![],
    })?;
    print(&PluginMessage::Snapshot {
        title: Some(cli.title),
        rows,
    })
}

fn print(message: &PluginMessage) -> Result<()> {
    println!("{}", serde_json::to_string(message)?);
    Ok(())
}
