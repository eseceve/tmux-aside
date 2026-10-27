use jiff::Timestamp;
use jiff::tz::TimeZone;
use tmux_aside::i18n::Language;
use tmux_aside::protocol::{Row, Tone};
use tmux_aside_clock::{Sort, rows, summary};

fn at(instant: &str) -> Timestamp {
    instant.parse().unwrap()
}

fn clock(label: &str, time: &str) -> Row {
    Row::with_detail(label, time)
}

fn specs(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn shows_labelled_clocks_sorted_alphabetically() {
    let rows = rows(
        at("2026-10-23T12:00:00Z"),
        &specs(&[
            "America/Santiago=Chile",
            "America/Bogota=Colombia",
            "America/Argentina/Buenos_Aires=Argentina",
            "America/Mexico_City=México",
        ]),
        Sort::Alphabetical,
        None,
        Language::En,
    )
    .unwrap();

    assert_eq!(
        rows,
        [
            clock("Argentina", "9:00 AM"),
            clock("Chile", "9:00 AM"),
            clock("Colombia", "7:00 AM"),
            clock("México", "6:00 AM"),
        ]
    );
}

#[test]
fn zones_without_a_label_show_their_city_name() {
    let rows = rows(
        at("2026-10-23T12:00:00Z"),
        &specs(&["America/New_York"]),
        Sort::Alphabetical,
        None,
        Language::En,
    )
    .unwrap();

    assert_eq!(rows[0], clock("New York", "8:00 AM"));
}

#[test]
fn an_unknown_zone_is_reported_by_name() {
    let error = rows(
        at("2026-10-23T12:00:00Z"),
        &specs(&["Mars/Olympus=Marte"]),
        Sort::Alphabetical,
        None,
        Language::En,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("Mars/Olympus"), "{error}");
}

#[test]
fn afternoon_times_use_pm() {
    let rows = rows(
        at("2026-10-23T20:05:00Z"),
        &specs(&["America/Santiago=Chile"]),
        Sort::Alphabetical,
        None,
        Language::En,
    )
    .unwrap();

    assert_eq!(rows[0], clock("Chile", "5:05 PM"));
}

#[test]
fn clocks_can_be_sorted_by_how_late_it_is() {
    let zones = specs(&[
        "America/Mexico_City=México",
        "America/Santiago=Chile",
        "America/Caracas=Venezuela",
    ]);
    let order = |sort| -> Vec<String> {
        rows(at("2026-10-26T13:00:00Z"), &zones, sort, None, Language::En)
            .unwrap()
            .into_iter()
            .map(|row| row.text)
            .collect()
    };

    assert_eq!(order(Sort::Latest), ["Chile", "Venezuela", "México"]);
    assert_eq!(order(Sort::Earliest), ["México", "Venezuela", "Chile"]);
    assert_eq!(order(Sort::Given), ["México", "Chile", "Venezuela"]);
}

#[test]
fn the_local_zone_is_highlighted() {
    let rows = rows(
        at("2026-10-23T12:00:00Z"),
        &specs(&["America/Santiago=Chile", "America/Caracas=Venezuela"]),
        Sort::Given,
        Some("America/Santiago"),
        Language::En,
    )
    .unwrap();

    let tones: Vec<(Tone, Tone)> = rows.iter().map(|row| (row.tone, row.detail_tone)).collect();
    assert_eq!(
        tones,
        [(Tone::Accent, Tone::Accent), (Tone::Plain, Tone::Plain)]
    );
}

#[test]
fn spanish_clocks_use_the_24_hour_time() {
    let rows = rows(
        at("2026-10-23T20:05:00Z"),
        &specs(&["America/Santiago=Chile"]),
        Sort::Alphabetical,
        None,
        Language::Es,
    )
    .unwrap();

    assert_eq!(rows[0], clock("Chile", "17:05"));
}

#[test]
fn the_summary_is_the_time_in_the_local_zone_in_the_sidebar_language() {
    let santiago = TimeZone::get("America/Santiago").unwrap();
    let now = at("2026-10-27T17:20:00Z");

    assert_eq!(summary(now, &santiago, Language::Es), "14:20");
    assert_eq!(summary(now, &santiago, Language::En), "2:20 PM");
}
