use jiff::civil::date;
use tmux_aside::i18n::Language;

fn from(vars: &[(&str, &str)]) -> Language {
    Language::from_vars(|name| {
        vars.iter()
            .find(|(var, _)| *var == name)
            .map(|(_, value)| value.to_string())
    })
}

#[test]
fn locales_are_read_by_their_language_code() {
    assert_eq!(Language::from_tag("es_CL.UTF-8"), Some(Language::Es));
    assert_eq!(Language::from_tag("es-CL"), Some(Language::Es));
    assert_eq!(Language::from_tag("EN"), Some(Language::En));
    assert_eq!(Language::from_tag("C"), None);
    assert_eq!(Language::from_tag("fr_FR.UTF-8"), None);
}

#[test]
fn the_sidebar_language_variable_wins_over_the_locale() {
    assert_eq!(
        from(&[("TMUX_ASIDE_LANG", "en"), ("LANG", "es_CL.UTF-8")]),
        Language::En
    );
}

#[test]
fn the_locale_follows_posix_precedence() {
    assert_eq!(
        from(&[("LC_ALL", "es_ES.UTF-8"), ("LANG", "en_US.UTF-8")]),
        Language::Es
    );
    assert_eq!(
        from(&[("LC_ALL", ""), ("LC_MESSAGES", "es_MX"), ("LANG", "en_US")]),
        Language::Es
    );
}

#[test]
fn unsupported_or_missing_locales_fall_back_to_english() {
    assert_eq!(from(&[("LANG", "fr_FR.UTF-8")]), Language::En);
    assert_eq!(from(&[]), Language::En);
}

#[test]
fn dates_are_written_out_in_each_language() {
    let date = date(2026, 10, 27);

    assert_eq!(Language::En.long_date(date), "Tuesday, October 27");
    assert_eq!(Language::Es.long_date(date), "Martes 27 de Octubre");
}
