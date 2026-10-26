use tmux_aside::width::{Width, resolve_width};

#[test]
fn default_is_ten_percent_of_window() {
    assert_eq!(resolve_width(None, 400), 40);
}

#[test]
fn default_is_clamped_between_25_and_50_columns() {
    assert_eq!(resolve_width(None, 100), 25);
    assert_eq!(resolve_width(None, 1000), 50);
}

#[test]
fn explicit_columns_skip_the_clamp() {
    assert_eq!(resolve_width(Some(Width::Columns(12)), 200), 12);
    assert_eq!(resolve_width(Some(Width::Columns(70)), 200), 70);
}

#[test]
fn explicit_percent_is_relative_to_window_and_skips_the_clamp() {
    assert_eq!(resolve_width(Some(Width::Percent(5)), 200), 10);
    assert_eq!(resolve_width(Some(Width::Percent(30)), 200), 60);
}

#[test]
fn explicit_width_is_at_least_one_column() {
    assert_eq!(resolve_width(Some(Width::Percent(1)), 50), 1);
    assert_eq!(resolve_width(Some(Width::Columns(0)), 200), 1);
}

#[test]
fn widths_above_80_columns_fall_back_to_default() {
    assert_eq!(resolve_width(Some(Width::Columns(120)), 400), 40);
    assert_eq!(resolve_width(Some(Width::Percent(50)), 400), 40);
}

#[test]
fn parses_columns_and_percentages() {
    assert_eq!("40".parse::<Width>(), Ok(Width::Columns(40)));
    assert_eq!("15%".parse::<Width>(), Ok(Width::Percent(15)));
    assert!("abc".parse::<Width>().is_err());
    assert!("150%".parse::<Width>().is_err());
}
