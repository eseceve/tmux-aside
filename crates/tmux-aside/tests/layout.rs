use tmux_aside::layout::{LayoutLine, LayoutOptions, LineKind, help, layout};
use tmux_aside::protocol::{Align, Row, Tone};
use tmux_aside::view::{Density, Key, Pin, Section, View};

fn lay(view: &View, width: usize, height: usize) -> Vec<LayoutLine> {
    layout(
        view,
        &LayoutOptions {
            width,
            height,
            spinner_frame: 0,
            current_window: None,
        },
    )
}

fn texts(view: &View, width: usize, height: usize) -> Vec<String> {
    lay(view, width, height)
        .into_iter()
        .map(|line| line.text())
        .collect()
}

fn prs() -> Section {
    Section::new(
        "PRs",
        vec![Row::new("#412 Fix login"), Row::new("#398 Add pets")],
        None,
    )
}

fn clocks() -> Section {
    Section {
        header: false,
        pin: Pin::Bottom,
        ..Section::new(
            "Clocks",
            vec![
                Row::new("jueves 23 de octubre"),
                Row::with_detail("Santiago", "09:18"),
            ],
            None,
        )
    }
}

#[test]
fn top_sections_render_their_title_and_count_on_a_line_and_rows_padded_to_width() {
    let view = View::new(vec![prs()]);

    assert_eq!(
        texts(&view, 20, 4),
        [
            "───── PRs · 2 ──────",
            "#412 Fix login      ",
            "#398 Add pets       ",
            "                    ",
        ]
    );
}

#[test]
fn bottom_pinned_sections_sit_on_the_last_lines_below_a_separator() {
    let view = View::new(vec![prs(), clocks()]);

    assert_eq!(
        texts(&view, 24, 8),
        [
            "─────── PRs · 2 ────────",
            "#412 Fix login          ",
            "#398 Add pets           ",
            "                        ",
            "                        ",
            "────────────────────────",
            "jueves 23 de octubre    ",
            "Santiago           09:18",
        ]
    );
}

#[test]
fn long_text_is_truncated_with_an_ellipsis_and_keeps_its_detail() {
    let view = View::new(vec![Section::new(
        "PRs",
        vec![Row::with_detail("#412 Fix the login flow for pets", "✓")],
        None,
    )]);

    assert_eq!(texts(&view, 20, 2)[1], "#412 Fix the logi… ✓");
}

#[test]
fn collapsed_sections_show_their_collapsed_title_and_summary() {
    let summarized = Section {
        collapsed_title: Some("27 de Oct".to_string()),
        summary: Some("14:20".to_string()),
        counts: false,
        ..Section::new("27 de Octubre", vec![Row::new("Chile")], None)
    };
    let mut view = View::new(vec![summarized]);

    assert_eq!(texts(&view, 30, 1), ["─────── 27 de Octubre ────────"]);
    view.apply(Key::Toggle);
    assert_eq!(texts(&view, 30, 1), ["──── ▸ 27 de Oct · 14:20 ─────"]);
}

#[test]
fn collapsed_sections_show_only_their_header() {
    let mut view = View::new(vec![prs()]);
    view.apply(Key::Toggle);

    assert_eq!(
        texts(&view, 20, 2),
        ["──── ▸ PRs · 2 ─────", "                    "]
    );
}

#[test]
fn only_the_selected_line_is_marked_selected() {
    let mut view = View::new(vec![prs()]);
    view.apply(Key::Down);

    let lines = lay(&view, 20, 3);

    let selected: Vec<bool> = lines.iter().map(|line| line.selected).collect();
    assert_eq!(selected, [false, true, false]);
    assert_eq!(lines[0].kind, LineKind::Header);
    assert_eq!(lines[1].kind, LineKind::Row);
}

#[test]
fn when_space_runs_out_the_bottom_section_stays_and_the_top_is_cut() {
    let view = View::new(vec![prs(), clocks()]);

    assert_eq!(
        texts(&view, 24, 4),
        [
            "─────── PRs · 2 ────────",
            "────────────────────────",
            "jueves 23 de octubre    ",
            "Santiago           09:18",
        ]
    );
}

#[test]
fn rows_respect_the_section_scroll() {
    let mut view = View::new(vec![Section::new(
        "Many",
        (1..=5).map(|row| Row::new(format!("row {row}"))).collect(),
        Some(2),
    )]);
    for _ in 0..4 {
        view.apply(Key::Down);
    }

    assert_eq!(
        texts(&view, 12, 3),
        ["─ Many · 5 ─", "row 3       ", "row 4       "]
    );
}

#[test]
fn centered_rows_are_centered_on_the_full_width() {
    let view = View::new(vec![Section {
        header: false,
        ..Section::new(
            "Clock",
            vec![Row {
                align: Align::Center,
                ..Row::new("viernes")
            }],
            None,
        )
    }]);

    assert_eq!(texts(&view, 21, 1), ["       viernes       "]);
}

#[test]
fn spans_carry_the_row_tones() {
    let view = View::new(vec![Section::new(
        "Clock",
        vec![Row {
            tone: Tone::Muted,
            detail_tone: Tone::Accent,
            ..Row::with_detail("Chile", "9:28 AM")
        }],
        None,
    )]);

    let lines = lay(&view, 24, 2);

    let tone_of = |text: &str| {
        lines[1]
            .spans
            .iter()
            .find(|span| span.text == text)
            .map(|span| span.tone)
    };
    assert_eq!(tone_of("Chile"), Some(Tone::Muted));
    assert_eq!(tone_of("9:28 AM"), Some(Tone::Accent));
}

#[test]
fn header_lines_are_muted() {
    let view = View::new(vec![prs()]);

    let lines = lay(&view, 20, 1);

    assert!(lines[0].spans.iter().all(|span| span.tone == Tone::Muted));
}

#[test]
fn sections_without_counts_show_only_their_title_and_groups() {
    let view = View::new(vec![Section {
        counts: false,
        ..Section::new(
            "Usage",
            vec![Row {
                group: Some("Claude".to_string()),
                ..Row::new("5 hours")
            }],
            None,
        )
    }]);

    assert_eq!(
        texts(&view, 16, 3),
        ["──── Usage ─────", "Claude          ", "  5 hours       "]
    );
}

#[test]
fn long_titles_are_truncated_inside_the_header_line() {
    let view = View::new(vec![Section::new(
        "Review requested",
        vec![Row::new("#9")],
        None,
    )]);

    assert_eq!(texts(&view, 14, 1), ["─ Review re… ─"]);
}

fn agent(name: &str, icon: &str, group: Option<&str>) -> Row {
    Row {
        icon: Some(icon.to_string()),
        icon_tone: Tone::Accent,
        group: group.map(str::to_string),
        ..Row::with_detail(name, "0:05")
    }
}

#[test]
fn icons_are_drawn_before_the_text_with_their_tone() {
    let view = View::new(vec![Section::new(
        "Agents",
        vec![agent("api", "◆", None)],
        None,
    )]);

    let lines = lay(&view, 16, 2);

    assert_eq!(lines[1].text(), "◆ api       0:05");
    let icon = lines[1].spans.iter().find(|span| span.text == "◆").unwrap();
    assert_eq!(icon.tone, Tone::Accent);
}

#[test]
fn the_spinner_icon_cycles_through_frames() {
    let view = View::new(vec![Section::new(
        "Agents",
        vec![agent("api", "spinner", None)],
        None,
    )]);

    let frames: Vec<String> = (0..3)
        .map(|frame| {
            layout(
                &view,
                &LayoutOptions {
                    width: 16,
                    height: 2,
                    spinner_frame: frame,
                    current_window: None,
                },
            )[1]
            .text()
        })
        .collect();

    assert_eq!(
        frames,
        ["⠋ api       0:05", "⠙ api       0:05", "⠹ api       0:05"]
    );
}

#[test]
fn group_lines_are_muted_and_show_an_arrow_only_when_collapsed() {
    let mut view = View::new(vec![Section::new(
        "Agents",
        vec![agent("one", "✓", Some("acme"))],
        None,
    )]);
    view.apply(Key::Down);
    view.apply(Key::Collapse);

    let lines = lay(&view, 18, 2);

    assert_eq!(lines[1].text(), "▸ acme           1");
    assert!(
        lines[1]
            .spans
            .iter()
            .all(|span| span.tone != Tone::Plain || span.text.trim().is_empty())
    );
}

#[test]
fn a_group_line_precedes_each_run_of_rows_sharing_a_group() {
    let view = View::new(vec![Section::new(
        "Agents",
        vec![
            agent("one", "✓", Some("acme")),
            agent("two", "✓", Some("acme")),
            agent("three", "✓", Some("tmux-aside")),
        ],
        None,
    )]);

    let lines = lay(&view, 18, 6);

    let texts: Vec<String> = lines.iter().map(|line| line.text()).collect();
    assert_eq!(
        texts,
        [
            "─── Agents · 3 ───",
            "acme             2",
            "  ✓ one       0:05",
            "  ✓ two       0:05",
            "tmux-aside       1",
            "  ✓ three     0:05",
        ]
    );
    assert_eq!(lines[1].kind, LineKind::Group);
    assert!(
        lines
            .iter()
            .all(|line| line.kind != LineKind::Group || !line.selected)
    );
}

fn pad(left: &str, right: &str, width: usize) -> String {
    let used = left.chars().count() + right.chars().count();
    format!("{left}{}{right}", " ".repeat(width - used))
}

fn tile(text: &str, group: &str, window: &str) -> Row {
    Row {
        icon: Some("✓".to_string()),
        icon_tone: Tone::Success,
        group: Some(group.to_string()),
        lines: vec!["api · main".to_string(), "Fix login".to_string()],
        bar_tone: Tone::Success,
        window: Some(window.to_string()),
        ..Row::with_detail(text, "#7")
    }
}

fn tiles() -> View {
    View::new(vec![Section {
        header: false,
        ..Section::new(
            "Agents",
            vec![tile("review", "api", "@1"), tile("deploy", "api", "@2")],
            None,
        )
    }])
}

#[test]
fn multi_line_rows_draw_a_bar_on_every_line_and_align_extra_lines_with_the_text() {
    let lines = lay(&tiles(), 22, 4);

    let texts: Vec<String> = lines.iter().map(|line| line.text()).collect();
    assert_eq!(
        texts,
        [
            pad("api", "2", 22),
            pad("▌ ✓ review", "#7", 22),
            pad("▌   api · main", "", 22),
            pad("▌   Fix login", "", 22),
        ]
    );
    let bar = lines[2].spans.iter().find(|span| span.text == "▌").unwrap();
    assert_eq!(bar.tone, Tone::Success);
    let extra = lines[2]
        .spans
        .iter()
        .find(|span| span.text == "api · main")
        .unwrap();
    assert_eq!(extra.tone, Tone::Muted);
}

#[test]
fn every_line_of_the_selected_row_is_selected() {
    let mut view = tiles();
    view.apply(Key::Down);

    let selected: Vec<bool> = lay(&view, 22, 7).iter().map(|line| line.selected).collect();

    assert_eq!(selected, [false, true, true, true, false, false, false]);
}

#[test]
fn rows_of_the_current_window_are_marked_current() {
    let lines = layout(
        &tiles(),
        &LayoutOptions {
            width: 22,
            height: 7,
            spinner_frame: 0,
            current_window: Some("@2".to_string()),
        },
    );

    let current: Vec<bool> = lines.iter().map(|line| line.current).collect();
    assert_eq!(current, [false, false, false, false, true, true, true]);
}

#[test]
fn collapsed_groups_show_their_line_with_count_and_hide_their_rows() {
    let mut view = tiles();
    view.apply(Key::Toggle);

    assert_eq!(
        texts(&view, 22, 2),
        [pad("▸ api", "2", 22), pad("", "", 22)]
    );
}

#[test]
fn comfortable_sections_end_with_a_blank_line_before_the_next_header() {
    let view = View::new(vec![
        Section {
            density: Density::Comfortable,
            ..prs()
        },
        Section::new("Reviews", vec![Row::new("#9 Fix")], None),
    ]);

    let lines = lay(&view, 16, 6);

    let texts: Vec<String> = lines.iter().map(|line| line.text()).collect();
    assert_eq!(texts[1], pad("#412 Fix login  ", "", 16));
    assert_eq!(texts[4], " ".repeat(16));
    assert_eq!(lines[4].kind, LineKind::Blank);
    assert_eq!(texts[5], "─ Reviews · 1 ──");
}

#[test]
fn compact_sections_are_followed_right_away_by_the_next_header() {
    let view = View::new(vec![
        prs(),
        Section::new("Reviews", vec![Row::new("#9 Fix")], None),
    ]);

    let texts: Vec<String> = lay(&view, 16, 5).iter().map(|line| line.text()).collect();

    assert_eq!(
        texts,
        [
            "─── PRs · 2 ────".to_string(),
            pad("#412 Fix login  ", "", 16),
            pad("#398 Add pets", "", 16),
            "─ Reviews · 1 ──".to_string(),
            pad("#9 Fix", "", 16),
        ]
    );
}

#[test]
fn sections_without_a_header_are_still_separated_by_a_line() {
    let view = View::new(vec![
        prs(),
        Section {
            header: false,
            ..Section::new("Reviews", vec![Row::new("#9 Fix")], None)
        },
    ]);

    let lines = lay(&view, 16, 6);

    assert_eq!(lines[3].text(), "─".repeat(16));
    assert_eq!(lines[3].kind, LineKind::Separator);
}

fn metered(percent: u8) -> View {
    View::new(vec![Section {
        header: false,
        ..Section::new(
            "Usage",
            vec![Row {
                meter: Some(percent),
                meter_tone: Tone::Danger,
                ..Row::with_detail("5h", "40%")
            }],
            None,
        )
    }])
}

#[test]
fn meters_fill_the_space_between_text_and_detail_in_their_tone() {
    let lines = lay(&metered(40), 20, 1);

    assert_eq!(lines[0].text(), "5h ━━━━━──────── 40%");
    let filled = lines[0]
        .spans
        .iter()
        .find(|span| span.text.starts_with('━'))
        .unwrap();
    assert_eq!(filled.tone, Tone::Danger);
    let empty = lines[0]
        .spans
        .iter()
        .find(|span| span.text.starts_with('─'))
        .unwrap();
    assert_eq!(empty.tone, Tone::Muted);
}

#[test]
fn meters_are_dropped_when_there_is_no_room() {
    assert_eq!(texts(&metered(40), 9, 1), ["5h    40%"]);
}

#[test]
fn empty_sections_show_only_their_title_with_a_zero_count() {
    let mut view = View::new(vec![Section::new("Reviews", vec![], None), prs()]);
    view.apply(Key::Down);

    let lines = lay(&view, 16, 3);

    assert_eq!(lines[0].text(), "─ Reviews · 0 ──");
    assert_eq!(lines[1].text(), "─── PRs · 2 ────");
    assert!(lines[1].selected);
}

#[test]
fn group_gaps_leave_a_blank_line_between_groups() {
    let view = View::new(vec![Section {
        group_gap: true,
        ..Section::new(
            "PRs",
            vec![
                agent("one", "✓", Some("api")),
                agent("two", "✓", Some("web")),
            ],
            None,
        )
    }]);

    assert_eq!(
        texts(&view, 16, 6),
        [
            "─── PRs · 2 ────",
            "api            1",
            "  ✓ one     0:05",
            "                ",
            "web            1",
            "  ✓ two     0:05",
        ]
    );
}

#[test]
fn comfortable_density_leaves_a_blank_line_between_rows_and_bars_only_on_content() {
    let mut view = tiles();
    view.sections[0].density = Density::Comfortable;

    let texts: Vec<String> = lay(&view, 22, 10).iter().map(|line| line.text()).collect();

    assert_eq!(
        texts[..8],
        [
            pad("api", "2", 22),
            pad("▌ ✓ review", "#7", 22),
            pad("▌   api · main", "", 22),
            pad("▌   Fix login", "", 22),
            pad("▌", "", 22),
            pad("▌ ✓ deploy", "#7", 22),
            pad("▌   api · main", "", 22),
            pad("▌   Fix login", "", 22),
        ]
    );
}

#[test]
fn help_lists_the_sidebar_keys_then_the_selected_rows_actions() {
    let mut view = View::new(vec![Section::new(
        "PRs",
        vec![Row {
            actions: vec![
                tmux_aside::protocol::Action::new("enter", &["open"]),
                tmux_aside::protocol::Action {
                    label: Some("copy the URL".to_string()),
                    ..tmux_aside::protocol::Action::new("c", &["copy"])
                },
            ],
            ..Row::new("#1")
        }],
        None,
    )]);
    view.apply(Key::Down);

    let texts: Vec<String> = help(&view, 28).iter().map(|line| line.text()).collect();

    assert_eq!(texts[0], "─────────── Keys ───────────");
    assert_eq!(texts[1], "j k                     move");
    assert_eq!(texts.last().unwrap(), "c               copy the URL");
    assert!(texts.iter().any(|text| text.contains("This row")));
}

#[test]
fn help_without_a_selected_row_lists_only_the_sidebar_keys() {
    let view = View::new(vec![prs()]);

    assert!(
        !help(&view, 28)
            .iter()
            .any(|line| line.text().contains("This row"))
    );
}

#[test]
fn lines_know_the_header_group_or_row_they_belong_to() {
    let view = View::new(vec![Section {
        density: Density::Comfortable,
        ..Section::new("Agents", vec![agent("one", "✓", Some("api"))], None)
    }]);

    let targets: Vec<_> = lay(&view, 18, 5)
        .into_iter()
        .map(|line| line.target.map(|cursor| cursor.target))
        .collect();

    use tmux_aside::view::Target;
    assert_eq!(
        targets,
        [
            Some(Target::Header),
            Some(Target::Group("api".to_string())),
            Some(Target::Row(0)),
            None,
            None,
        ]
    );
}

#[test]
fn wide_characters_take_two_columns_so_details_stay_aligned() {
    let view = View::new(vec![Section {
        header: false,
        ..Section::new(
            "Calendar",
            vec![
                Row::with_detail("😎 Decompress", "12:45"),
                Row::with_detail("Daily", "11:30"),
                Row::with_detail("😎 A title much too long", "12:45"),
            ],
            None,
        )
    }]);

    let lines = texts(&view, 20, 10);

    assert_eq!(lines[0], "😎 Decompress  12:45");
    assert_eq!(lines[1], "Daily          11:30");
    assert_eq!(lines[2], "😎 A title mu… 12:45");
}

#[test]
fn top_sections_scroll_to_keep_the_selection_visible() {
    let mut view = View::new(vec![
        Section::new(
            "A",
            (0..4).map(|row| Row::new(format!("a{row}"))).collect(),
            None,
        ),
        Section::new(
            "B",
            (0..4).map(|row| Row::new(format!("b{row}"))).collect(),
            None,
        ),
    ]);
    for _ in 0..8 {
        view.apply(Key::Down);
    }

    let lines = texts(&view, 12, 4);

    assert_eq!(lines.last().unwrap().trim(), "b2");
}

#[test]
fn long_titles_are_cut_to_eighty_percent_of_the_width() {
    let view = View::new(vec![Section {
        counts: false,
        ..Section::new(
            "Asegurar la identidad y el acceso al checkout",
            vec![],
            None,
        )
    }]);

    assert_eq!(
        texts(&view, 40, 1)[0],
        "─── Asegurar la identidad y el acce… ───"
    );
}

#[test]
fn the_bar_between_comfortable_rows_takes_their_color_when_shared_and_is_muted_otherwise() {
    let tone_between = |second_bar: Tone| {
        let mut view = tiles();
        view.sections[0].density = Density::Comfortable;
        view.sections[0].rows[1].bar_tone = second_bar;
        let lines = lay(&view, 22, 8);
        lines[4]
            .spans
            .iter()
            .find(|span| span.text == "▌")
            .unwrap()
            .tone
    };

    assert_eq!(tone_between(Tone::Success), Tone::Success);
    assert_eq!(tone_between(Tone::Warning), Tone::Muted);
}

#[test]
fn diff_stats_are_drawn_green_and_red_wherever_they_appear() {
    let view = View::new(vec![Section {
        header: false,
        ..Section::new(
            "Agents",
            vec![Row {
                lines: vec!["api · +120 -8 · main".to_string()],
                ..Row::new("review")
            }],
            None,
        )
    }]);

    let lines = lay(&view, 24, 2);

    assert_eq!(lines[1].text(), pad("api · +120 -8 · main", "", 24));
    let tones: Vec<(&str, Tone)> = lines[1]
        .spans
        .iter()
        .filter(|span| span.text.starts_with(['+', '-']))
        .map(|span| (span.text.as_str(), span.tone))
        .collect();
    assert_eq!(tones, [("+120", Tone::Success), ("-8", Tone::Danger)]);
}
