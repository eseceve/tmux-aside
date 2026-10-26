use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use tmux_aside::protocol::{PLUGIN_PROTOCOL, PluginMessage};
use tmux_aside_clock::Sort;

/// Sidebar section showing the time in several time zones.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// IANA time zones, optionally followed by `=Label` (e.g. America/Santiago=Chile); the
    /// system zone when omitted
    zones: Vec<String>,
    /// Order of the clocks
    #[arg(long, value_enum, default_value_t = SortArg::Alphabetical)]
    sort: SortArg,
}

#[derive(Clone, Copy, ValueEnum)]
enum SortArg {
    /// By label
    Alphabetical,
    /// The zone where it is earliest first
    Earliest,
    /// The zone where it is latest first
    Latest,
    /// In the order the zones are given
    Given,
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
    let mut zones = cli.zones;
    if zones.is_empty() {
        let system = TimeZone::system();
        let name = system
            .iana_name()
            .context("pass IANA zones as arguments: the system zone has no name")?;
        zones.push(name.to_string());
    }
    let sort = match cli.sort {
        SortArg::Alphabetical => Sort::Alphabetical,
        SortArg::Earliest => Sort::Earliest,
        SortArg::Latest => Sort::Latest,
        SortArg::Given => Sort::Given,
    };
    let rows = tmux_aside_clock::rows(Timestamp::now(), &zones, sort)?;
    print(&PluginMessage::Hello {
        protocol: PLUGIN_PROTOCOL,
        features: vec![],
    })?;
    print(&PluginMessage::Snapshot {
        title: Some("Clocks".to_string()),
        rows,
    })
}

fn print(message: &PluginMessage) -> Result<()> {
    println!("{}", serde_json::to_string(message)?);
    Ok(())
}
