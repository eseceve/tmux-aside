use jiff::civil::date;
use tmux_aside::i18n::Language;
use tmux_aside::title::{expand, expand_on};

#[test]
fn today_expands_to_the_date_in_the_sidebar_language() {
    assert_eq!(
        expand_on("${today}", date(2026, 10, 25), Language::En),
        "Sunday, October 25"
    );
    assert_eq!(
        expand_on("${today}", date(2026, 10, 25), Language::Es),
        "Domingo 25 de Octubre"
    );
}

#[test]
fn today_with_a_language_tag_ignores_the_sidebar_language() {
    assert_eq!(
        expand_on("Hoy: ${today:es}", date(2026, 9, 2), Language::En),
        "Hoy: Miércoles 2 de Septiembre"
    );
    assert_eq!(
        expand_on("${today:en}", date(2026, 10, 25), Language::Es),
        "Sunday, October 25"
    );
}

#[test]
fn titles_without_placeholders_are_kept_as_written() {
    assert_eq!(expand("Pull Request", Language::Es), "Pull Request");
}
