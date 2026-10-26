use tmux_aside::config::parse;
use tmux_aside::folds::{Fold, Folds, apply, key, of, read, write};
use tmux_aside::protocol::Row;
use tmux_aside::view::{Section, View};

fn view() -> View {
    View::new(vec![
        Section::new("Agentes", vec![Row::new("a")], None),
        Section::new("Usage", vec![Row::new("b")], None),
    ])
}

#[test]
fn sections_are_keyed_by_their_configured_title_or_plugin() {
    let config =
        parse("sections:\n  - plugin: clock\n    title: ${today}\n  - plugin: usage\n").unwrap();

    assert_eq!(key(&config.sections[0]), "${today}");
    assert_eq!(key(&config.sections[1]), "usage");
}

fn configs(yaml: &str) -> Vec<tmux_aside::config::SectionConfig> {
    parse(yaml).unwrap().sections
}

#[test]
fn only_folds_that_differ_from_the_config_are_remembered_and_they_fold_again() {
    let configs = configs("sections:\n  - plugin: agents\n  - plugin: usage\n");
    let mut folded = view();
    folded.sections[1].collapsed = true;
    folded.sections[1].collapsed_groups = vec!["Claude".to_string()];

    let folds = of(&folded, &configs);
    assert_eq!(
        folds,
        Folds::from([(
            "usage".to_string(),
            Fold {
                collapsed: true,
                groups: vec!["Claude".to_string()],
            }
        )])
    );

    let mut fresh = view();
    apply(&mut fresh, &configs, &folds);
    assert!(fresh.sections[1].collapsed);
    assert_eq!(fresh.sections[1].collapsed_groups, ["Claude"]);
    assert!(!fresh.sections[0].collapsed);
}

#[test]
fn unfolding_a_section_configured_collapsed_is_remembered() {
    let configs = configs("sections:\n  - plugin: agents\n    collapsed: true\n");
    let unfolded = View::new(vec![Section::new("Agentes", vec![], None)]);

    let folds = of(&unfolded, &configs);
    assert_eq!(folds["agents"], Fold::default());

    let mut fresh = View::new(vec![Section {
        collapsed: true,
        ..Section::new("Agentes", vec![], None)
    }]);
    apply(&mut fresh, &configs, &folds);
    assert!(!fresh.sections[0].collapsed);
    assert!(of(&fresh, &configs).contains_key("agents"));
}

#[test]
fn folds_round_trip_through_their_file_and_a_missing_file_is_empty() {
    let dir = std::env::temp_dir().join(format!("tmux-aside-folds-{}", std::process::id()));
    let path = dir.join("state/folds.json");
    assert!(read(&path).is_empty());

    let folds = Folds::from([(
        "usage".to_string(),
        Fold {
            collapsed: true,
            groups: vec![],
        },
    )]);
    write(&path, &folds).unwrap();

    assert_eq!(read(&path), folds);
    let _ = std::fs::remove_dir_all(dir);
}
