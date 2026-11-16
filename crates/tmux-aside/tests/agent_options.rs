use tmux_aside::{
    config,
    i18n::Language,
    launch::{AgentOptions, Launch, Template, resolve_agent},
    setup::Ai,
};

#[test]
fn each_explicit_option_wins_over_template_and_configuration() {
    let config = config::parse("ai: agy\nmodel: config-model\neffort: low\n").unwrap();
    let template =
        Template::parse("---\nai: claude\nmodel: template-model\neffort: medium\n---\nDo it")
            .unwrap();
    let settings = resolve_agent(
        &template,
        AgentOptions {
            agent: Some("codex".into()),
            model: Some("chosen-model".into()),
            effort: Some("high".into()),
        },
        Some(&config),
        Language::En,
    )
    .unwrap();
    assert_eq!(settings.ai, Ai::Codex);
    assert_eq!(settings.model.as_deref(), Some("chosen-model"));
    assert_eq!(settings.effort.as_deref(), Some("high"));
}

#[test]
fn partial_settings_fall_back_independently() {
    let config = config::parse("ai: codex\nmodel: config-model\neffort: low\n").unwrap();
    let template = Template::parse("---\nmodel: template-model\n---\nDo it").unwrap();
    let settings = resolve_agent(
        &template,
        AgentOptions::default(),
        Some(&config),
        Language::En,
    )
    .unwrap();
    assert_eq!(settings.ai, Ai::Codex);
    assert_eq!(settings.model.as_deref(), Some("template-model"));
    assert_eq!(settings.effort.as_deref(), Some("low"));
}

#[test]
fn absent_settings_preserve_existing_defaults() {
    let template = Template::parse("Do it").unwrap();
    let settings = resolve_agent(&template, AgentOptions::default(), None, Language::En).unwrap();
    assert_eq!(settings.ai, Ai::Claude);
    assert!(settings.arguments().is_empty());
}

#[test]
fn invalid_settings_are_rejected_before_launching() {
    let template = Template::parse("Do it").unwrap();
    for options in [
        AgentOptions {
            agent: Some("unknown".into()),
            ..Default::default()
        },
        AgentOptions {
            model: Some(" ".into()),
            ..Default::default()
        },
        AgentOptions {
            agent: Some("claude".into()),
            effort: Some("xhigh".into()),
            ..Default::default()
        },
    ] {
        assert!(resolve_agent(&template, options, None, Language::En).is_err());
    }
}

#[test]
fn older_launch_records_load_without_model_or_effort_and_new_records_round_trip() {
    let json = serde_json::json!({"id":"example", "repository":"/repo", "branch":"feature", "worktree":"/worktree", "session":"s", "window":"w", "pane":"p", "ai":"codex", "task":"task", "opens_pull_request":true, "pull_request":null});
    let mut launch: Launch = serde_json::from_value(json).unwrap();
    assert_eq!(launch.model, None);
    assert_eq!(launch.effort, None);
    launch.model = Some("selected-model".into());
    launch.effort = Some("high".into());
    let restored: Launch = serde_json::from_str(&serde_json::to_string(&launch).unwrap()).unwrap();
    assert_eq!(restored, launch);
}
