use jiff::Timestamp;
use jiff::tz::TimeZone;
use tmux_aside::protocol::{Action, Row, Tone};

const MEETING_HOSTS: [&str; 6] = [
    "meet.google.com",
    "zoom.us",
    "teams.microsoft.com",
    "teams.live.com",
    "webex.com",
    "whereby.com",
];

/// One occurrence of a calendar event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub title: String,
    pub start: Timestamp,
    pub end: Timestamp,
    pub all_day: bool,
    /// The user declined the invitation.
    pub declined: bool,
    /// Video call link found in the event, if any.
    pub link: Option<String>,
}

/// Which events are listed and when one counts as starting soon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Events starting within this many seconds show a countdown.
    pub soon: i64,
    pub limit: usize,
    pub all_day: bool,
    pub declined: bool,
}

/// One row per upcoming event, sorted by start and skipping the ones that ended. Events of
/// later days are grouped by day. The right side shows when the event ends after `now` for an
/// event in progress, a countdown for one starting within `options.soon` or its start time,
/// as `in 12m–10:15`, and the bar is colored accordingly. Enter joins the video call when the event has one.
pub fn rows(events: &[Event], now: Timestamp, tz: &TimeZone, options: Options) -> Vec<Row> {
    let mut events: Vec<&Event> = events
        .iter()
        .filter(|event| event.end > now)
        .filter(|event| options.all_day || !event.all_day)
        .filter(|event| options.declined || !event.declined)
        .collect();
    events.sort_by_key(|event| event.start);
    let today = now.to_zoned(tz.clone()).date();
    events
        .into_iter()
        .take(options.limit)
        .map(|event| {
            let start = event.start.to_zoned(tz.clone());
            let end = event.end.to_zoned(tz.clone()).strftime("%H:%M");
            let until = event.start.as_second() - now.as_second();
            let (detail, tone) = match until {
                _ if event.all_day => ("all day".to_string(), Tone::Muted),
                ..=0 => (format!("now–{end}"), Tone::Accent),
                until if until <= options.soon => {
                    (format!("in {}m–{end}", (until + 59) / 60), Tone::Warning)
                }
                _ => (format!("{}–{end}", start.strftime("%H:%M")), Tone::Muted),
            };
            let group = match start.date() {
                date if date <= today => None,
                date if date == today.tomorrow().unwrap_or(today) => Some("Tomorrow".to_string()),
                _ => Some(start.strftime("%A %-d").to_string()),
            };
            Row {
                group,
                detail_tone: tone,
                bar_tone: tone,
                actions: event
                    .link
                    .iter()
                    .map(|link| Action {
                        label: Some("join the call".to_string()),
                        ..Action::new("enter", &["open", link])
                    })
                    .collect(),
                ..Row::with_detail(event.title.clone(), detail)
            }
        })
        .collect()
}

/// The first video call link (Meet, Zoom, Teams, Webex or Whereby) found in `texts`.
pub fn meeting_link<'a>(texts: impl IntoIterator<Item = &'a str>) -> Option<String> {
    texts
        .into_iter()
        .flat_map(|text| text.split(|c: char| c.is_whitespace() || "<>\"'()[]".contains(c)))
        .map(|word| word.trim_end_matches(['.', ',', ';']))
        .find(|word| {
            word.starts_with("https://") && MEETING_HOSTS.iter().any(|host| word.contains(host))
        })
        .map(str::to_string)
}

/// `title` without emoji, symbols drawn as emoji and the joiners and selectors between them,
/// with the spaces they leave collapsed. Terminals and tmux disagree on how wide some emoji
/// are, which misaligns the row.
pub fn without_emoji(title: &str) -> String {
    let is_emoji = |c: char| {
        matches!(
            u32::from(c),
            0x2300..=0x23FF
                | 0x2600..=0x27BF
                | 0x2B00..=0x2BFF
                | 0x1F000..=0x1FAFF
                | 0x200D
                | 0xFE00..=0xFE0F
                | 0xE0020..=0xE007F
        )
    };
    title
        .chars()
        .filter(|c| !is_emoji(*c))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
