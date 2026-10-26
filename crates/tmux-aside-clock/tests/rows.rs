use jiff::Timestamp;
use tmux_aside::protocol::Row;
use tmux_aside_clock::rows;

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
    let rows = rows(at("2026-10-23T12:00:00Z"), &specs(&["America/New_York"])).unwrap();

    assert_eq!(rows[0], clock("New York", "8:00 AM"));
}

#[test]
fn an_unknown_zone_is_reported_by_name() {
    let error = rows(at("2026-10-23T12:00:00Z"), &specs(&["Mars/Olympus=Marte"]))
        .unwrap_err()
        .to_string();

    assert!(error.contains("Mars/Olympus"), "{error}");
}

#[test]
fn afternoon_times_use_pm() {
    let rows = rows(
        at("2026-10-23T20:05:00Z"),
        &specs(&["America/Santiago=Chile"]),
    )
    .unwrap();

    assert_eq!(rows[0], clock("Chile", "5:05 PM"));
}
