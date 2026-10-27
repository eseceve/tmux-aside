use jiff::Timestamp;
use jiff::tz::TimeZone;
use tmux_aside::i18n::Language;
use tmux_aside::protocol::Tone;
use tmux_aside_calendar::{Event, Options, meeting_link, rows, without_emoji};

const OPTIONS: Options = Options {
    soon: 15 * 60,
    limit: 10,
    all_day: false,
    declined: false,
};

fn at(instant: &str) -> Timestamp {
    instant.parse().unwrap()
}

fn event(title: &str, start: &str, end: &str) -> Event {
    Event {
        title: title.to_string(),
        start: at(start),
        end: at(end),
        all_day: false,
        declined: false,
        link: None,
    }
}

fn summary(events: &[Event], now: &str) -> Vec<(String, String, Tone, Option<String>)> {
    rows(events, at(now), &TimeZone::UTC, OPTIONS, Language::En)
        .into_iter()
        .map(|row| (row.text, row.detail.unwrap(), row.bar_tone, row.group))
        .collect()
}

#[test]
fn events_show_now_a_countdown_or_their_start_time() {
    let events = [
        event("Planning", "2026-10-26T15:00:00Z", "2026-10-26T16:00:00Z"),
        event("Daily", "2026-10-26T14:30:00Z", "2026-10-26T14:45:00Z"),
        event("Focus", "2026-10-26T14:00:00Z", "2026-10-26T14:40:00Z"),
        event("Breakfast", "2026-10-26T12:00:00Z", "2026-10-26T12:30:00Z"),
    ];

    let summary = summary(&events, "2026-10-26T14:18:30Z");

    let row = |title: &str, detail: &str, tone| (title.to_string(), detail.to_string(), tone, None);
    assert_eq!(
        summary,
        [
            row("Focus", "now–14:40", Tone::Accent),
            row("Daily", "in 12m–14:45", Tone::Warning),
            row("Planning", "15:00–16:00", Tone::Muted),
        ]
    );
}

#[test]
fn later_days_are_grouped() {
    let events = [
        event("Review", "2026-10-28T10:00:00Z", "2026-10-28T11:00:00Z"),
        event("Daily", "2026-10-27T14:30:00Z", "2026-10-27T14:45:00Z"),
    ];

    let groups: Vec<Option<String>> = summary(&events, "2026-10-26T20:00:00Z")
        .into_iter()
        .map(|row| row.3)
        .collect();

    assert_eq!(
        groups,
        [
            Some("Tomorrow".to_string()),
            Some("Wednesday 28".to_string())
        ]
    );
}

#[test]
fn all_day_and_declined_events_are_hidden_unless_asked_and_the_list_is_limited() {
    let events = [
        Event {
            all_day: true,
            ..event("Holiday", "2026-10-26T00:00:00Z", "2026-10-27T00:00:00Z")
        },
        Event {
            declined: true,
            ..event("Declined", "2026-10-26T16:00:00Z", "2026-10-26T17:00:00Z")
        },
        event("Kept", "2026-10-26T18:00:00Z", "2026-10-26T19:00:00Z"),
    ];
    let titles = |options| -> Vec<String> {
        rows(
            &events,
            at("2026-10-26T15:00:00Z"),
            &TimeZone::UTC,
            options,
            Language::En,
        )
        .into_iter()
        .map(|row| row.text)
        .collect()
    };

    assert_eq!(titles(OPTIONS), ["Kept"]);
    assert_eq!(
        titles(Options {
            all_day: true,
            declined: true,
            ..OPTIONS
        }),
        ["Holiday", "Declined", "Kept"]
    );
    assert_eq!(
        titles(Options {
            limit: 0,
            ..OPTIONS
        })
        .len(),
        0
    );
}

#[test]
fn enter_joins_the_call_when_there_is_one() {
    let with_link = Event {
        link: Some("https://meet.google.com/abc-defg-hij".to_string()),
        ..event("Daily", "2026-10-26T14:30:00Z", "2026-10-26T14:45:00Z")
    };

    let rows = rows(
        &[
            with_link,
            event("Focus", "2026-10-26T15:00:00Z", "2026-10-26T16:00:00Z"),
        ],
        at("2026-10-26T10:00:00Z"),
        &TimeZone::UTC,
        OPTIONS,
        Language::En,
    );

    assert_eq!(
        rows[0].action("enter"),
        Some(
            &[
                "open".to_string(),
                "https://meet.google.com/abc-defg-hij".to_string()
            ][..]
        )
    );
    assert!(rows[1].actions.is_empty());
}

#[test]
fn meeting_links_are_found_in_notes_or_location() {
    let notes = "Join: <https://meet.google.com/abc-defg-hij>.\nOther https://example.com";

    assert_eq!(
        meeting_link([notes]).as_deref(),
        Some("https://meet.google.com/abc-defg-hij")
    );
    assert_eq!(
        meeting_link(["Room 4", "https://us02web.zoom.us/j/123?pwd=x,"]).as_deref(),
        Some("https://us02web.zoom.us/j/123?pwd=x")
    );
    assert_eq!(meeting_link(["https://example.com"]), None);
}

#[test]
fn emoji_can_be_removed_from_titles() {
    assert_eq!(without_emoji("🔒 ✍ Deep focus"), "Deep focus");
    assert_eq!(without_emoji("🛡\u{fe0f} 🍱 Lunch"), "Lunch");
    assert_eq!(without_emoji("1:1 con Ana 👩‍💻"), "1:1 con Ana");
    assert_eq!(without_emoji("Daily standup"), "Daily standup");
}

#[test]
fn rows_can_be_shown_in_spanish() {
    let events = [
        Event {
            all_day: true,
            ..event("Feriado", "2026-10-26T00:00:00Z", "2026-10-27T00:00:00Z")
        },
        Event {
            link: Some("https://meet.google.com/abc-defg-hij".to_string()),
            ..event("Foco", "2026-10-26T14:00:00Z", "2026-10-26T14:40:00Z")
        },
        event("Daily", "2026-10-26T14:30:00Z", "2026-10-26T14:45:00Z"),
        event("Retro", "2026-10-27T10:00:00Z", "2026-10-27T11:00:00Z"),
        event("Review", "2026-10-28T10:00:00Z", "2026-10-28T11:00:00Z"),
    ];

    let rows = rows(
        &events,
        at("2026-10-26T14:18:30Z"),
        &TimeZone::UTC,
        Options {
            all_day: true,
            ..OPTIONS
        },
        Language::Es,
    );

    let shown: Vec<(String, Option<String>)> = rows
        .iter()
        .map(|row| (row.detail.clone().unwrap(), row.group.clone()))
        .collect();
    let row = |detail: &str, group: Option<&str>| (detail.to_string(), group.map(str::to_string));
    assert_eq!(
        shown,
        [
            row("todo el día", None),
            row("ahora–14:40", None),
            row("en 12m–14:45", None),
            row("10:00–11:00", Some("Mañana")),
            row("10:00–11:00", Some("Miércoles 28")),
        ]
    );
    assert_eq!(
        rows[1].actions[0].label.as_deref(),
        Some("unirse a la llamada")
    );
}
