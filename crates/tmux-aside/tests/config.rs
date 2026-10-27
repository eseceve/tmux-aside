use std::time::Duration;

use tmux_aside::config::{Config, EnterAction, Mode, Pin, Theme, parse, parse_duration};
use tmux_aside::i18n::Language;

#[test]
fn parses_a_full_section() {
    let config = parse(
        r#"
sections:
  - plugin: clock
    args: ["America/Santiago", "Europe/Madrid"]
    mode: interval
    interval: 1s
    timeout: 500ms
    max_height: 5
"#,
    )
    .unwrap();

    let section = &config.sections[0];
    assert_eq!(section.plugin, "clock");
    assert_eq!(section.args, ["America/Santiago", "Europe/Madrid"]);
    assert_eq!(section.mode, Mode::Interval);
    assert_eq!(section.interval, Duration::from_secs(1));
    assert_eq!(section.timeout, Duration::from_millis(500));
    assert_eq!(section.max_height, Some(5));
}

#[test]
fn applies_defaults() {
    let config = parse("sections:\n  - plugin: github\n").unwrap();

    let section = &config.sections[0];
    assert!(section.args.is_empty());
    assert_eq!(section.mode, Mode::Interval);
    assert_eq!(section.interval, Duration::from_secs(30));
    assert_eq!(section.timeout, Duration::from_secs(10));
    assert_eq!(section.max_height, None);
}

#[test]
fn parses_stream_mode() {
    let config = parse("sections:\n  - plugin: agents\n    mode: stream\n").unwrap();

    assert_eq!(config.sections[0].mode, Mode::Stream);
}

#[test]
fn an_empty_file_has_no_sections() {
    assert_eq!(
        parse("").unwrap(),
        Config {
            sections: vec![],
            theme: Theme::default(),
            auto_reload: false,
            enter: EnterAction::Focus,
            language: None,
        }
    );
}

#[test]
fn parses_the_language() {
    let config = parse("language: es\n").unwrap();

    assert_eq!(config.language, Some(Language::Es));
}

#[test]
fn unsupported_languages_are_rejected() {
    assert!(parse("language: klingon\n").is_err());
}

#[test]
fn errors_name_the_offending_section() {
    let error = parse("sections:\n  - plugin: clock\n  - plugin: github\n    interval: soon\n")
        .unwrap_err()
        .to_string();

    assert!(error.contains("section 2 (github)"), "{error}");
    assert!(error.contains("soon"), "{error}");
}

#[test]
fn unknown_keys_are_rejected_to_catch_typos() {
    let error = parse("sections:\n  - plugin: clock\n    intervl: 1s\n")
        .unwrap_err()
        .to_string();

    assert!(error.contains("intervl"), "{error}");
}

#[test]
fn durations_accept_milliseconds_seconds_and_minutes() {
    assert_eq!(parse_duration("200ms").unwrap(), Duration::from_millis(200));
    assert_eq!(parse_duration("45s").unwrap(), Duration::from_secs(45));
    assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
    assert!(parse_duration("5").is_err());
    assert!(parse_duration("0s").is_err());
    assert!(parse_duration("1h").is_err());
}

#[test]
fn sections_can_hide_their_header_and_pin_to_the_bottom() {
    let config =
        parse("sections:\n  - plugin: clock\n    header: false\n    pin: bottom\n").unwrap();

    assert!(!config.sections[0].header);
    assert_eq!(config.sections[0].pin, Pin::Bottom);
}

#[test]
fn sections_have_a_header_and_sit_at_the_top_by_default() {
    let config = parse("sections:\n  - plugin: clock\n").unwrap();

    assert!(config.sections[0].header);
    assert_eq!(config.sections[0].pin, Pin::Top);
}

#[test]
fn theme_colors_override_the_defaults_by_name_or_hex() {
    let config = parse("theme:\n  accent: magenta\n  muted: \"#6c7086\"\n").unwrap();

    assert_eq!(config.theme.accent, "magenta");
    assert_eq!(config.theme.muted, "#6c7086");
    assert_eq!(config.theme.success, Theme::default().success);
}

#[test]
fn invalid_theme_colors_are_rejected_by_key() {
    let error = parse("theme:\n  accent: notacolor\n")
        .unwrap_err()
        .to_string();

    assert!(error.contains("theme.accent"), "{error}");
    assert!(error.contains("notacolor"), "{error}");
}

#[test]
fn sections_can_rename_themselves_and_hide_their_counts() {
    let config =
        parse("sections:\n  - plugin: usage\n    title: Uso\n    counts: false\n").unwrap();

    assert_eq!(config.sections[0].title.as_deref(), Some("Uso"));
    assert!(!config.sections[0].counts);
}

#[test]
fn sections_keep_the_plugin_title_and_show_counts_by_default() {
    let config = parse("sections:\n  - plugin: usage\n").unwrap();

    assert_eq!(config.sections[0].title, None);
    assert!(config.sections[0].counts);
}

#[test]
fn theme_can_color_the_selection_and_the_current_window() {
    let config = parse("theme:\n  selection: \"#161616\"\n  current: \"#78a9ff\"\n").unwrap();

    assert_eq!(config.theme.selection.as_deref(), Some("#161616"));
    assert_eq!(config.theme.current.as_deref(), Some("#78a9ff"));
    assert_eq!(Theme::default().selection, None);
    assert!(parse("theme:\n  selection: nope\n").is_err());
}

#[test]
fn sections_can_leave_a_gap_between_groups() {
    let config = parse("sections:\n  - plugin: github\n    group_gap: true\n").unwrap();

    assert!(config.sections[0].group_gap);
    assert!(!parse("sections:\n  - plugin: github\n").unwrap().sections[0].group_gap);
}

#[test]
fn auto_reload_is_off_unless_enabled() {
    assert!(!parse("sections: []\n").unwrap().auto_reload);
    assert!(parse("auto_reload: true\n").unwrap().auto_reload);
}

#[test]
fn enter_focuses_by_default_and_can_select() {
    assert_eq!(parse("").unwrap().enter, EnterAction::Focus);
    assert_eq!(parse("enter: select\n").unwrap().enter, EnterAction::Select);
    assert!(parse("enter: jump\n").is_err());
}

#[test]
fn sections_are_compact_unless_comfortable_is_asked() {
    use tmux_aside::config::Density;
    let density = |yaml: &str| parse(yaml).unwrap().sections[0].density;

    assert_eq!(density("sections:\n  - plugin: github\n"), Density::Compact);
    assert_eq!(
        density("sections:\n  - plugin: github\n    density: comfortable\n"),
        Density::Comfortable
    );
}

#[test]
fn sections_start_unfolded_unless_configured_collapsed() {
    let collapsed = |yaml: &str| parse(yaml).unwrap().sections[0].collapsed;

    assert!(!collapsed("sections:\n  - plugin: github\n"));
    assert!(collapsed(
        "sections:\n  - plugin: github\n    collapsed: true\n"
    ));
}

#[test]
fn the_example_config_is_valid() {
    let config = parse(include_str!("../../../examples/config.yaml")).unwrap();

    assert_eq!(config.sections.len(), 6);
}
