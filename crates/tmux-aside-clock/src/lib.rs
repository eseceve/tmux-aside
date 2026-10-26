use anyhow::{Context, Result};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use tmux_aside::protocol::Row;

/// Builds one `label  h:mm AM` row per zone for `now`, sorted by label.
///
/// Each spec is an IANA zone, optionally followed by `=Label`; without a label the zone's
/// city name is shown.
pub fn rows(now: Timestamp, specs: &[String]) -> Result<Vec<Row>> {
    let mut clocks = Vec::new();
    for spec in specs {
        let (zone, label) = match spec.split_once('=') {
            Some((zone, label)) => (zone, label.to_string()),
            None => (spec.as_str(), city(spec)),
        };
        let tz = TimeZone::get(zone).with_context(|| format!("unknown time zone '{zone}'"))?;
        clocks.push(Row::with_detail(
            label,
            now.to_zoned(tz).strftime("%-I:%M %p").to_string(),
        ));
    }
    clocks.sort_by(|a, b| a.text.cmp(&b.text));
    Ok(clocks)
}

fn city(zone: &str) -> String {
    zone.rsplit('/').next().unwrap_or(zone).replace('_', " ")
}
