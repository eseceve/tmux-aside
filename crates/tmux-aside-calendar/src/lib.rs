use jiff::Timestamp;
use jiff::tz::TimeZone;
use tmux_aside::i18n::Language;
use tmux_aside::protocol::{Action, Row, Tone};

const MEETING_HOSTS: [&str; 6] = [
    "meet.google.com",
    "zoom.us",
    "teams.microsoft.com",
    "teams.live.com",
    "webex.com",
    "whereby.com",
];

/// Text the calendar section shows, in one language.
pub struct Messages {
    pub title: &'static str,
    pub all_day: &'static str,
    pub now_until: fn(&str) -> String,
    pub in_minutes_until: fn(i64, &str) -> String,
    pub tomorrow: &'static str,
    pub join_call: &'static str,
    pub no_access: &'static str,
    pub macos_only: &'static str,
}

const EN: Messages = Messages {
    title: "Calendar",
    all_day: "all day",
    now_until: |end| format!("now–{end}"),
    in_minutes_until: |minutes, end| format!("in {minutes}m–{end}"),
    tomorrow: "Tomorrow",
    join_call: "join the call",
    no_access: "no access to calendars: allow it in System Settings > Privacy & Security > Calendars",
    macos_only: "the calendar section reads the macOS Calendar and only runs on macOS",
};

const ES: Messages = Messages {
    title: "Calendario",
    all_day: "todo el día",
    now_until: |end| format!("ahora–{end}"),
    in_minutes_until: |minutes, end| format!("en {minutes}m–{end}"),
    tomorrow: "Mañana",
    join_call: "unirse a la llamada",
    no_access: "sin acceso a los calendarios: permítelo en Ajustes del Sistema > Privacidad y seguridad > Calendarios",
    macos_only: "la sección de calendario lee el Calendario de macOS y solo funciona en macOS",
};

/// The calendar section's text in `language`.
pub fn messages(language: Language) -> &'static Messages {
    match language {
        Language::En => &EN,
        Language::Es => &ES,
    }
}

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
/// Text is written in `language`.
pub fn rows(
    events: &[Event],
    now: Timestamp,
    tz: &TimeZone,
    options: Options,
    language: Language,
) -> Vec<Row> {
    let messages = messages(language);
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
                _ if event.all_day => (messages.all_day.to_string(), Tone::Muted),
                ..=0 => ((messages.now_until)(&end.to_string()), Tone::Accent),
                until if until <= options.soon => (
                    (messages.in_minutes_until)((until + 59) / 60, &end.to_string()),
                    Tone::Warning,
                ),
                _ => (format!("{}–{end}", start.strftime("%H:%M")), Tone::Muted),
            };
            let group = match start.date() {
                date if date <= today => None,
                date if date == today.tomorrow().unwrap_or(today) => {
                    Some(messages.tomorrow.to_string())
                }
                date => Some(format!("{} {}", language.weekday(date), date.day())),
            };
            Row {
                group,
                detail_tone: tone,
                bar_tone: tone,
                actions: event
                    .link
                    .iter()
                    .map(|link| Action {
                        label: Some(messages.join_call.to_string()),
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
