use tmux_aside::protocol::Row;
use tmux_aside::view::{Cursor, Key, Section, Target, View};

fn section(title: &str, rows: usize, max_height: Option<usize>) -> Section {
    Section::new(
        title,
        (0..rows)
            .map(|row| Row::new(format!("{title} {row}")))
            .collect(),
        max_height,
    )
}

fn view() -> View {
    View::new(vec![section("A", 2, None), section("B", 1, None)])
}

fn header(section: usize) -> Option<Cursor> {
    Some(Cursor {
        section,
        target: Target::Header,
    })
}

fn row(section: usize, row: usize) -> Option<Cursor> {
    Some(Cursor {
        section,
        target: Target::Row(row),
    })
}

fn group(section: usize, name: &str) -> Option<Cursor> {
    Some(Cursor {
        section,
        target: Target::Group(name.to_string()),
    })
}

#[test]
fn starts_on_the_first_section_header() {
    assert_eq!(view().selected, header(0));
}

#[test]
fn down_walks_headers_and_rows_across_sections() {
    let mut view = view();
    let mut visited = vec![view.selected.clone()];
    for _ in 0..4 {
        view.apply(Key::Down);
        visited.push(view.selected.clone());
    }
    assert_eq!(
        visited,
        vec![header(0), row(0, 0), row(0, 1), header(1), row(1, 0)]
    );
}

#[test]
fn down_stops_at_the_last_line_and_up_at_the_first() {
    let mut view = view();
    view.apply(Key::Up);
    assert_eq!(view.selected, header(0));

    for _ in 0..10 {
        view.apply(Key::Down);
    }
    assert_eq!(view.selected, row(1, 0));
}

#[test]
fn up_walks_back_across_sections() {
    let mut view = view();
    for _ in 0..3 {
        view.apply(Key::Down);
    }
    view.apply(Key::Up);
    assert_eq!(view.selected, row(0, 1));
}

#[test]
fn toggle_on_a_header_collapses_and_skips_its_rows() {
    let mut view = view();

    view.apply(Key::Toggle);
    view.apply(Key::Down);

    assert!(view.sections[0].collapsed);
    assert_eq!(view.selected, header(1));
}

#[test]
fn toggle_twice_expands_again() {
    let mut view = view();

    view.apply(Key::Toggle);
    view.apply(Key::Toggle);
    view.apply(Key::Down);

    assert!(!view.sections[0].collapsed);
    assert_eq!(view.selected, row(0, 0));
}

#[test]
fn toggle_on_a_row_does_nothing() {
    let mut view = view();
    view.apply(Key::Down);

    view.apply(Key::Toggle);

    assert!(!view.sections[0].collapsed);
    assert_eq!(view.selected, row(0, 0));
}

#[test]
fn scroll_keeps_the_selected_row_inside_max_height() {
    let mut view = View::new(vec![section("A", 5, Some(3))]);

    for _ in 0..4 {
        view.apply(Key::Down);
    }
    assert_eq!(view.selected, row(0, 3));
    assert_eq!(view.sections[0].visible_rows(), 1..4);

    for _ in 0..3 {
        view.apply(Key::Up);
    }
    assert_eq!(view.selected, row(0, 0));
    assert_eq!(view.sections[0].visible_rows(), 0..3);
}

#[test]
fn sections_without_max_height_show_every_row() {
    let view = View::new(vec![section("A", 5, None)]);

    assert_eq!(view.sections[0].visible_rows(), 0..5);
}

fn rows(count: usize) -> Vec<Row> {
    (0..count)
        .map(|row| Row::new(format!("new {row}")))
        .collect()
}

#[test]
fn set_section_replaces_title_and_rows_and_keeps_collapse() {
    let mut view = view();
    view.apply(Key::Toggle);

    view.set_section(0, "Renamed", rows(3));

    assert_eq!(view.sections[0].title, "Renamed");
    assert_eq!(view.sections[0].rows, rows(3));
    assert!(view.sections[0].collapsed);
}

#[test]
fn set_section_moves_the_selection_to_the_last_row_when_rows_shrink() {
    let mut view = view();
    view.apply(Key::Down);
    view.apply(Key::Down);
    assert_eq!(view.selected, row(0, 1));

    view.set_section(0, "A", rows(1));

    assert_eq!(view.selected, row(0, 0));
}

#[test]
fn set_section_moves_the_selection_to_the_header_when_rows_disappear() {
    let mut view = view();
    view.apply(Key::Down);

    view.set_section(0, "A", vec![]);

    assert_eq!(view.selected, header(0));
}

#[test]
fn set_section_clamps_the_scroll_to_the_new_rows() {
    let mut view = View::new(vec![section("A", 20, Some(5))]);
    for _ in 0..20 {
        view.apply(Key::Down);
    }
    assert_eq!(view.sections[0].visible_rows(), 15..20);

    view.set_section(0, "A", rows(7));

    assert_eq!(view.selected, row(0, 6));
    assert_eq!(view.sections[0].visible_rows(), 2..7);
}

#[test]
fn set_section_leaves_other_sections_selection_alone() {
    let mut view = view();
    for _ in 0..4 {
        view.apply(Key::Down);
    }
    assert_eq!(view.selected, row(1, 0));

    view.set_section(0, "A", vec![]);

    assert_eq!(view.selected, row(1, 0));
}

fn headerless(title: &str, rows: usize) -> Section {
    Section {
        header: false,
        ..section(title, rows, None)
    }
}

#[test]
fn starts_on_the_first_row_when_the_first_section_has_no_header() {
    let view = View::new(vec![headerless("A", 2), section("B", 1, None)]);

    assert_eq!(view.selected, row(0, 0));
}

#[test]
fn navigation_skips_the_header_of_headerless_sections() {
    let mut view = View::new(vec![section("A", 1, None), headerless("B", 2)]);
    let mut visited = vec![view.selected.clone()];
    for _ in 0..3 {
        view.apply(Key::Down);
        visited.push(view.selected.clone());
    }

    assert_eq!(visited, vec![header(0), row(0, 0), row(1, 0), row(1, 1)]);
}

#[test]
fn emptying_a_headerless_section_moves_the_selection_to_a_remaining_line() {
    let mut view = View::new(vec![section("A", 1, None), headerless("B", 2)]);
    for _ in 0..2 {
        view.apply(Key::Down);
    }

    view.set_section(1, "B", vec![]);

    assert_eq!(view.selected, row(0, 0));
}

fn with_action(text: &str, command: &[&str]) -> Row {
    Row {
        actions: vec![tmux_aside::protocol::Action::new("enter", command)],
        ..Row::new(text)
    }
}

#[test]
fn activating_a_row_returns_its_enter_action() {
    let mut view = View::new(vec![Section::new(
        "A",
        vec![with_action("jump", &["tmux", "switch-client", "-t", "%3"])],
        None,
    )]);
    view.apply(Key::Down);

    assert_eq!(
        view.activate().map(|action| action.command),
        Some(vec![
            "tmux".to_string(),
            "switch-client".to_string(),
            "-t".to_string(),
            "%3".to_string()
        ])
    );
}

#[test]
fn activating_a_header_toggles_it_and_returns_nothing() {
    let mut view = view();

    assert_eq!(view.activate(), None);
    assert!(view.sections[0].collapsed);
}

#[test]
fn activating_a_row_without_actions_does_nothing() {
    let mut view = view();
    view.apply(Key::Down);

    assert_eq!(view.activate(), None);
    assert!(!view.sections[0].collapsed);
}

fn grouped() -> View {
    let row = |text: &str, group: &str| Row {
        group: Some(group.to_string()),
        ..Row::new(text)
    };
    View::new(vec![Section {
        header: false,
        ..Section::new(
            "Agents",
            vec![row("a1", "api"), row("a2", "api"), row("w1", "web")],
            None,
        )
    }])
}

#[test]
fn navigation_walks_group_lines_and_their_rows() {
    let mut view = grouped();
    let mut visited = vec![view.selected.clone()];
    for _ in 0..4 {
        view.apply(Key::Down);
        visited.push(view.selected.clone());
    }

    assert_eq!(
        visited,
        vec![
            group(0, "api"),
            row(0, 0),
            row(0, 1),
            group(0, "web"),
            row(0, 2)
        ]
    );
}

#[test]
fn toggling_a_group_collapses_it_and_skips_its_rows() {
    let mut view = grouped();

    view.apply(Key::Toggle);
    view.apply(Key::Down);

    assert_eq!(view.sections[0].collapsed_groups, ["api"]);
    assert_eq!(view.selected, group(0, "web"));
}

#[test]
fn activating_a_group_toggles_it() {
    let mut view = grouped();

    assert_eq!(view.activate(), None);
    assert_eq!(view.sections[0].collapsed_groups, ["api"]);
    view.activate();
    assert!(view.sections[0].collapsed_groups.is_empty());
}

#[test]
fn collapsed_groups_survive_snapshots_and_vanished_groups_are_forgotten() {
    let mut view = grouped();
    view.apply(Key::Toggle);
    let rows = view.sections[0].rows.clone();

    view.set_section(0, "Agents", rows.clone());
    assert_eq!(view.sections[0].collapsed_groups, ["api"]);

    view.set_section(
        0,
        "Agents",
        rows.into_iter()
            .filter(|row| row.group.as_deref() == Some("web"))
            .collect(),
    );
    assert!(view.sections[0].collapsed_groups.is_empty());
}

#[test]
fn a_selected_row_that_disappears_moves_the_selection_to_a_visible_line() {
    let mut view = grouped();
    for _ in 0..4 {
        view.apply(Key::Down);
    }
    assert_eq!(view.selected, row(0, 2));
    let rows: Vec<Row> = view.sections[0].rows[..2].to_vec();

    view.set_section(0, "Agents", rows);

    assert_eq!(view.selected, row(0, 1));
}

#[test]
fn collapse_folds_the_selected_group_and_expand_unfolds_it() {
    let mut view = grouped();

    view.apply(Key::Collapse);
    assert_eq!(view.sections[0].collapsed_groups, ["api"]);
    view.apply(Key::Collapse);
    assert_eq!(view.sections[0].collapsed_groups, ["api"]);

    view.apply(Key::Expand);
    assert!(view.sections[0].collapsed_groups.is_empty());
    view.apply(Key::Expand);
    assert!(view.sections[0].collapsed_groups.is_empty());
}

#[test]
fn collapse_on_a_row_moves_to_its_group_line() {
    let mut view = grouped();
    view.apply(Key::Down);
    assert_eq!(view.selected, row(0, 0));

    view.apply(Key::Collapse);

    assert_eq!(view.selected, group(0, "api"));
    assert!(view.sections[0].collapsed_groups.is_empty());
}

#[test]
fn collapse_on_an_ungrouped_row_moves_to_the_section_header() {
    let mut view = view();
    view.apply(Key::Down);

    view.apply(Key::Collapse);

    assert_eq!(view.selected, header(0));
    assert!(!view.sections[0].collapsed);
}

#[test]
fn collapse_and_expand_fold_section_headers() {
    let mut view = view();

    view.apply(Key::Collapse);
    assert!(view.sections[0].collapsed);
    view.apply(Key::Expand);
    assert!(!view.sections[0].collapsed);
}

fn in_window(text: &str, window: &str) -> Row {
    Row {
        window: Some(window.to_string()),
        ..Row::new(text)
    }
}

#[test]
fn focus_moves_the_selection_to_the_first_row_of_the_window() {
    let mut view = View::new(vec![
        section("A", 1, None),
        Section::new(
            "Agents",
            vec![in_window("api", "@1"), in_window("web", "@2")],
            Some(1),
        ),
    ]);

    view.follow_window("@2");

    assert_eq!(view.selected, row(1, 1));
    assert_eq!(view.sections[1].scroll, 1);
}

#[test]
fn focus_ignores_rows_folded_out_of_sight() {
    let mut view = View::new(vec![Section::new(
        "Agents",
        vec![in_window("api", "@1")],
        None,
    )]);
    view.apply(Key::Toggle);

    view.follow_window("@1");

    assert_eq!(view.selected, header(0));
}

#[test]
fn selected_rows_resolve_actions_bound_to_other_keys() {
    let mut view = View::new(vec![Section::new(
        "A",
        vec![Row {
            actions: vec![tmux_aside::protocol::Action {
                cwd: Some("/repo".to_string()),
                ..tmux_aside::protocol::Action::new("p", &["gh", "pr", "view"])
            }],
            ..Row::new("agent")
        }],
        None,
    )]);

    assert_eq!(view.selected_action("p"), None);
    view.apply(Key::Down);

    let action = view.selected_action("p").unwrap();
    assert_eq!(action.cwd.as_deref(), Some("/repo"));
    assert_eq!(view.selected_action("x"), None);
}
