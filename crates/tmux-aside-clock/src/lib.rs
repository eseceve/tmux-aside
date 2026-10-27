use anyhow::{Context, Result};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use tmux_aside::i18n::Language;
use tmux_aside::protocol::{Row, Tone};

/// Text the clock section shows, in one language.
pub struct Messages {
    pub title: &'static str,
    /// `strftime` format of the time.
    pub time: &'static str,
}

const EN: Messages = Messages {
    title: "Clocks",
    time: "%-I:%M %p",
};

const ES: Messages = Messages {
    title: "Relojes",
    time: "%H:%M",
};

/// The clock section's text in `language`.
pub fn messages(language: Language) -> &'static Messages {
    match language {
        Language::En => &EN,
        Language::Es => &ES,
    }
}

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

/// Builds one `label  h:mm AM` row per zone for `now` (`label  13:05` in Spanish), in `sort`
/// order; zones at the same time are ordered by label.
///
/// Each spec is an IANA zone, optionally followed by `=Label`; without a label the zone's
/// city name is shown. The clock of the `local` zone is highlighted.
pub fn rows(
    now: Timestamp,
    specs: &[String],
    sort: Sort,
    local: Option<&str>,
    language: Language,
) -> Result<Vec<Row>> {
    let mut clocks = Vec::new();
    for spec in specs {
        let (zone, label) = match spec.split_once('=') {
            Some((zone, label)) => (zone, label.to_string()),
            None => (spec.as_str(), city(spec)),
        };
        let tz = TimeZone::get(zone).with_context(|| format!("unknown time zone '{zone}'"))?;
        let time = now.to_zoned(tz);
        let offset = time.offset().seconds();
        let mut row = Row::with_detail(label, time.strftime(messages(language).time).to_string());
        if local == Some(zone) {
            row.tone = Tone::Accent;
            row.detail_tone = Tone::Accent;
        }
        clocks.push((offset, row));
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
