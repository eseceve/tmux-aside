use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use tmux_aside::protocol::{PLUGIN_PROTOCOL, PluginMessage};

/// Sidebar section showing the time in several time zones.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// IANA time zones, optionally followed by `=Label` (e.g. America/Santiago=Chile); the
    /// system zone when omitted
    zones: Vec<String>,
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
    let rows = tmux_aside_clock::rows(Timestamp::now(), &zones)?;
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
