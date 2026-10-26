use jiff::civil::date;
use tmux_aside::title::{expand, expand_on};

#[test]
fn today_expands_to_the_english_date() {
    assert_eq!(
        expand_on("${today}", date(2026, 10, 25)),
        "Sunday, October 25"
    );
}

#[test]
fn today_es_expands_to_the_capitalized_spanish_date() {
    assert_eq!(
        expand_on("${today:es}", date(2026, 10, 25)),
        "Domingo 25 de Octubre"
    );
    assert_eq!(
        expand_on("Hoy: ${today:es}", date(2026, 9, 2)),
        "Hoy: Miércoles 2 de Septiembre"
    );
}

#[test]
fn titles_without_placeholders_are_kept_as_written() {
    assert_eq!(expand("Pull Request"), "Pull Request");
}
