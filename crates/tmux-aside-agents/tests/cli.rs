mod support;

use std::time::Duration;

use support::{Plugin, Tmux, wait_for};
use tmux_aside::protocol::{PluginMessage, Tone};
use tmux_aside_agents::Status;

#[test]
fn set_status_records_the_agent_running_in_the_pane() {
    let tmux = Tmux::start();

    let output = tmux.run(Some("%0"), &["set-status", "working"]);

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    let state = tmux.state("%0");
    assert_eq!(state.status, Some(Status::Working));
    assert_eq!(state.command, "sleep");
    assert_eq!(state.project, "repo");
    assert_eq!(state.window, "work");
    assert_eq!(
        state.pane_pid.to_string(),
        tmux.tmux(&["display-message", "-p", "-t", "%0", "#{pane_pid}"])
    );
}

#[test]
fn repeating_a_status_keeps_its_start_time_and_a_new_status_resets_it() {
    let tmux = Tmux::start();
    tmux.run(Some("%0"), &["set-status", "working"]);
    let mut state = tmux.state("%0");
    state.status_ts = 5;
    tmux.write_state("%0", &state);

    tmux.run(Some("%0"), &["set-status", "working"]);
    assert_eq!(tmux.state("%0").status_ts, 5);

    tmux.run(Some("%0"), &["set-status", "done"]);
    assert_ne!(tmux.state("%0").status_ts, 5);
}

#[test]
fn register_records_the_agent_without_a_status_and_clear_forgets_it() {
    let tmux = Tmux::start();

    tmux.run(Some("%0"), &["register"]);
    assert_eq!(tmux.state("%0").status, None);

    tmux.run(Some("%0"), &["set-status", "clear"]);
    assert!(!tmux.state_file("%0").exists());
}

#[test]
fn outside_tmux_set_status_is_a_silent_no_op() {
    let tmux = Tmux::start();

    let output = tmux.run(None, &["set-status", "working"]);

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!tmux.dir.join("state/tmux-aside/agents").exists());
}

#[test]
fn the_plugin_greets_and_shows_a_placeholder_without_agents() {
    let tmux = Tmux::start();
    let mut plugin = Plugin::start(&tmux, &[]);

    assert!(matches!(
        plugin.next(),
        PluginMessage::Hello { protocol: 1, .. }
    ));
    plugin.snapshot_where("placeholder", |rows| {
        rows.len() == 1 && rows[0].text == "no agents"
    });
}

#[test]
fn the_plugin_lists_agents_and_forgets_them_when_their_pane_dies() {
    let tmux = Tmux::start();
    let pane = tmux.split();
    let window_id = tmux.tmux(&["display-message", "-p", "-t", &pane, "#{window_id}"]);
    let mut plugin = Plugin::start(&tmux, &["--sort", "priority", "--group", "none"]);
    plugin.next();

    tmux.run(Some(&pane), &["set-status", "waiting"]);
    plugin.snapshot_where("agent row", |rows| {
        rows.iter().any(|row| {
            row.group.is_none()
                && row.bar_tone == Tone::Warning
                && row.text == "work"
                && row.window.as_deref() == Some(window_id.as_str())
                && row.lines.first().map(String::as_str) == Some("repo · feature/sidebar")
        })
    });

    tmux.tmux(&["kill-pane", "-t", &pane]);
    plugin.snapshot_where("agent gone", |rows| {
        rows.iter().all(|row| row.window.is_none())
    });
    wait_for("state file removed", || !tmux.state_file(&pane).exists());
}

#[test]
fn the_plugin_exits_when_its_stdin_closes() {
    let tmux = Tmux::start();
    let mut plugin = Plugin::start(&tmux, &[]);
    plugin.next();

    plugin.close_stdin();

    assert!(plugin.exited_within(Duration::from_secs(3)));
}

#[test]
fn the_plugin_hides_elapsed_time_unless_asked() {
    let tmux = Tmux::start();
    let pane = tmux.split();
    tmux.run(Some(&pane), &["set-status", "done"]);

    let mut hidden = Plugin::start(&tmux, &[]);
    hidden.next();
    hidden.snapshot_where("row without time", |rows| {
        rows.iter()
            .any(|row| row.window.is_some() && row.detail.is_none())
    });

    let mut shown = Plugin::start(&tmux, &["--elapsed"]);
    shown.next();
    shown.snapshot_where("row with time", |rows| {
        rows.iter()
            .any(|row| row.window.is_some() && row.detail.is_some())
    });
}

#[test]
fn the_plugin_shows_the_pull_request_of_the_agents_branch() {
    let tmux = Tmux::start();
    tmux.fake_gh(
        r#"[{"number": 348, "headRefName": "feature/sidebar", "statusCheckRollup": [{"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS"}]}]"#,
    );
    let pane = tmux.split();
    tmux.run(Some(&pane), &["set-status", "done"]);

    let mut plugin = Plugin::start(&tmux, &[]);
    plugin.next();

    plugin.snapshot_where("pull request detail", |rows| {
        rows.iter().any(|row| {
            row.detail.as_deref() == Some("#348 ✓")
                && row.detail_tone == tmux_aside::protocol::Tone::Success
        })
    });
}

#[test]
fn list_prints_each_agent_status_as_text_or_json() {
    let tmux = Tmux::start();
    let pane = tmux.split();
    tmux.run(Some("%0"), &["set-status", "working"]);
    tmux.run(Some(&pane), &["set-status", "waiting"]);

    let output = tmux.run(None, &["list", "--json"]);

    assert!(output.status.success());
    let agents: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    let status = |id: &str| {
        agents
            .iter()
            .find(|agent| agent["pane_id"] == id)
            .map(|agent| agent["status"].clone())
    };
    assert_eq!(agents.len(), 2, "{agents:?}");
    assert_eq!(status("%0"), Some("working".into()));
    assert_eq!(status(&pane), Some("waiting".into()));
    assert_eq!(agents[0]["project"], "repo");

    let text = String::from_utf8(tmux.run(None, &["list"]).stdout).unwrap();
    assert!(
        text.lines()
            .any(|line| line.starts_with(&format!("{pane}\twaiting\trepo\t"))),
        "{text}"
    );
}
