use tmux_aside::protocol::{Action, Align, PluginMessage, Row, Tone};

#[test]
fn rows_accept_alignment_and_tones() {
    let message: PluginMessage = serde_json::from_str(
        r#"{"type":"snapshot","rows":[{"text":"date","align":"center","tone":"muted","detail":"9 AM","detail_tone":"accent"}]}"#,
    )
    .unwrap();

    let PluginMessage::Snapshot { rows, .. } = message else {
        panic!("expected a snapshot");
    };
    assert_eq!(rows[0].align, Align::Center);
    assert_eq!(rows[0].tone, Tone::Muted);
    assert_eq!(rows[0].detail_tone, Tone::Accent);
}

#[test]
fn unknown_tones_and_alignments_fall_back_instead_of_rejecting_the_snapshot() {
    let message: PluginMessage = serde_json::from_str(
        r#"{"type":"snapshot","rows":[{"text":"x","align":"justify","tone":"sparkly"}]}"#,
    )
    .unwrap();

    let PluginMessage::Snapshot { rows, .. } = message else {
        panic!("expected a snapshot");
    };
    assert_eq!(rows[0].align, Align::Left);
    assert_eq!(rows[0].tone, Tone::Plain);
}

#[test]
fn plain_rows_serialize_without_optional_fields() {
    assert_eq!(
        serde_json::to_string(&Row::new("x")).unwrap(),
        r#"{"text":"x"}"#
    );
}

#[test]
fn rows_accept_icons_groups_and_actions() {
    let message: PluginMessage = serde_json::from_str(
        r#"{"type":"snapshot","rows":[{"text":"api","icon":"spinner","icon_tone":"accent","group":"acme","actions":[{"key":"enter","command":["tmux","switch-client","-t","%3"]}]}]}"#,
    )
    .unwrap();

    let PluginMessage::Snapshot { rows, .. } = message else {
        panic!("expected a snapshot");
    };
    assert_eq!(rows[0].icon.as_deref(), Some("spinner"));
    assert_eq!(rows[0].icon_tone, Tone::Accent);
    assert_eq!(rows[0].group.as_deref(), Some("acme"));
    assert_eq!(
        rows[0].actions,
        [Action::new("enter", &["tmux", "switch-client", "-t", "%3"])]
    );
}

#[test]
fn snapshots_carry_an_optional_summary() {
    let with: PluginMessage =
        serde_json::from_str(r#"{"type":"snapshot","summary":"14:20","rows":[]}"#).unwrap();
    let without: PluginMessage = serde_json::from_str(r#"{"type":"snapshot","rows":[]}"#).unwrap();

    let PluginMessage::Snapshot { summary, .. } = with else {
        panic!("expected a snapshot");
    };
    assert_eq!(summary.as_deref(), Some("14:20"));
    let PluginMessage::Snapshot { summary, .. } = without else {
        panic!("expected a snapshot");
    };
    assert_eq!(summary, None);
}
