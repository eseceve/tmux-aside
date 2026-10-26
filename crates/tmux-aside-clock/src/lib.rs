use anyhow::{Context, Result};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use tmux_aside::protocol::Row;

/// Order of the clocks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    /// By label.
    #[default]
    Alphabetical,
    /// The zone where it is earliest first.
    Earliest,
    /// The zone where it is latest first.
    Latest,
    /// In the order the zones are given.
    Given,
}

/// Builds one `label  h:mm AM` row per zone for `now`, in `sort` order; zones at the same
/// time are ordered by label.
///
/// Each spec is an IANA zone, optionally followed by `=Label`; without a label the zone's
/// city name is shown.
pub fn rows(now: Timestamp, specs: &[String], sort: Sort) -> Result<Vec<Row>> {
    let mut clocks = Vec::new();
    for spec in specs {
        let (zone, label) = match spec.split_once('=') {
            Some((zone, label)) => (zone, label.to_string()),
            None => (spec.as_str(), city(spec)),
        };
        let tz = TimeZone::get(zone).with_context(|| format!("unknown time zone '{zone}'"))?;
        let time = now.to_zoned(tz);
        let offset = time.offset().seconds();
        clocks.push((
            offset,
            Row::with_detail(label, time.strftime("%-I:%M %p").to_string()),
        ));
    }
    match sort {
        Sort::Alphabetical => clocks.sort_by(|(_, a), (_, b)| a.text.cmp(&b.text)),
        Sort::Earliest => clocks.sort_by(|(x, a), (y, b)| x.cmp(y).then(a.text.cmp(&b.text))),
        Sort::Latest => clocks.sort_by(|(x, a), (y, b)| y.cmp(x).then(a.text.cmp(&b.text))),
        Sort::Given => {}
    }
    Ok(clocks.into_iter().map(|(_, row)| row).collect())
}

fn city(zone: &str) -> String {
    zone.rsplit('/').next().unwrap_or(zone).replace('_', " ")
}
