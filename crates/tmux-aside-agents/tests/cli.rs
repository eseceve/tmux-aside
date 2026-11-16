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

#[test]
fn discovery_resolves_codex_inside_a_launcher_without_trusting_legacy_hooks() {
    use std::os::unix::fs::PermissionsExt;
    let tmux = Tmux::start();
    // Emulate tmux reporting the launcher as the foreground command.
    let real_tmux = std::process::Command::new("which")
        .arg("tmux")
        .output()
        .unwrap();
    let real_tmux = String::from_utf8(real_tmux.stdout).unwrap();
    let wrapper = tmux.dir.join("bin/tmux");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n'{}' \"$@\" | sed 's/sleep/tmux-aside/g'\n",
            real_tmux.trim()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let pid = tmux.tmux(&["display-message", "-p", "-t", "%0", "#{pane_pid}"]);
    let ps = tmux.dir.join("bin/ps");
    std::fs::write(
        &ps,
        format!("#!/bin/sh\nprintf '%s\\n' '99998 {pid} /opt/homebrew/bin/codex'\n"),
    )
    .unwrap();
    std::fs::set_permissions(&ps, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut plugin = Plugin::start(&tmux, &[]);
    plugin.next();
    plugin.snapshot_where("launched Codex", |rows| {
        rows.iter().any(|row| row.window.is_some())
    });
    tmux.run(Some("%0"), &["set-status", "waiting"]);
    let state = tmux.state("%0");
    assert_eq!(state.command, "codex");
    assert_eq!(state.status, None);
    assert!(state.terminal.is_some());
    let listed = tmux.run(None, &["list", "--json"]);
    let states: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(states[0]["command"], "codex");
    assert!(states[0]["status"].is_null());

    // Once the child exits, a sidebar's plugins and unrelated Codex processes
    // must not keep the launcher registered as an agent.
    std::fs::write(
        &ps,
        format!("#!/bin/sh\nprintf '%s\\n' '99998 {pid} /bin/tmux-aside-agents' '99997 99999 /bin/codex'\n"),
    )
    .unwrap();
    plugin.snapshot_where("agent child gone", |rows| {
        rows.iter().all(|row| row.window.is_none())
    });
    wait_for("launcher state removed", || !tmux.state_file("%0").exists());
    let output = tmux.run(Some("%0"), &["set-status", "waiting"]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!tmux.state_file("%0").exists());
}

fn emulate_codex(tmux: &Tmux, pane: &str) {
    use std::os::unix::fs::PermissionsExt;
    tmux.tmux(&["respawn-pane", "-k", "-t", pane, "stty raw -echo; exec cat"]);
    wait_for("terminal fixture", || {
        tmux.tmux(&[
            "display-message",
            "-p",
            "-t",
            pane,
            "#{pane_current_command}",
        ]) == "cat"
    });
    let actual = std::process::Command::new("which")
        .arg("tmux")
        .output()
        .unwrap();
    let actual = String::from_utf8(actual.stdout).unwrap();
    for (name, script) in [
        (
            "tmux",
            format!(
                "#!/bin/sh\n'{}' \"$@\" | sed 's/cat/codex/g'\n",
                actual.trim()
            ),
        ),
        (
            "ps",
            format!(
                "#!/bin/sh\n'{}' list-panes -a -F '#{{pane_pid}} 1 codex'\n",
                actual.trim()
            ),
        ),
    ] {
        let path = tmux.dir.join("bin").join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn emit_status(tmux: &Tmux, pane: &str, state: &str) {
    let sequence = format!("\x1b]7501;state={state}:app=codex\x1b\\");
    let mut args = vec![
        "send-keys".to_string(),
        "-t".into(),
        pane.into(),
        "-H".into(),
    ];
    args.extend(sequence.bytes().map(|byte| format!("{byte:02x}")));
    tmux.tmux(&args.iter().map(String::as_str).collect::<Vec<_>>());
}

#[test]
fn terminal_states_stay_with_their_panes_and_legacy_hooks_cannot_overwrite_them() {
    let tmux = Tmux::start();
    let other = tmux.split();
    emulate_codex(&tmux, "%0");
    emulate_codex(&tmux, &other);
    let mut plugin = Plugin::start(&tmux, &[]);
    plugin.next();
    plugin.snapshot_where("unknown before first event", |rows| {
        rows.len() == 2
            && rows
                .iter()
                .all(|r| r.detail.as_deref() == Some("status unknown"))
    });
    wait_for("both readers", || {
        ["%0", &other]
            .iter()
            .all(|pane| tmux.tmux(&["display-message", "-p", "-t", pane, "#{pane_pipe}"]) == "1")
    });
    emit_status(&tmux, "%0", "blocked");
    emit_status(&tmux, &other, "done");
    wait_for("independent states", || {
        tmux.state("%0").status == Some(Status::Waiting)
            && tmux.state(&other).status == Some(Status::Done)
    });
    tmux.run(Some("%0"), &["set-status", "working"]);
    tmux.run(Some("%0"), &["set-status", "clear"]);
    assert_eq!(tmux.state("%0").status, Some(Status::Waiting));
    emit_status(&tmux, "%0", "idle");
    wait_for("idle observation", || {
        tmux.state("%0").terminal.unwrap().state
            == tmux_aside_agents::terminal::TerminalStatus::Idle
    });
    assert!(!tmux.state("%0").ready);
    plugin.close_stdin();
    assert!(plugin.exited_within(Duration::from_secs(3)));
    // tmux notices its pipe reader's EOF on the next write.
    for pane in ["%0", &other] {
        tmux.tmux(&["send-keys", "-t", pane, "x"]);
    }
    wait_for("readers removed on plugin exit", || {
        ["%0", &other]
            .iter()
            .all(|pane| tmux.tmux(&["display-message", "-p", "-t", pane, "#{pane_pipe}"]) == "0")
    });
}

#[test]
fn an_existing_pipe_is_preserved_and_replacing_our_pipe_invalidates_the_state() {
    let tmux = Tmux::start();
    emulate_codex(&tmux, "%0");
    let log = tmux.dir.join("other-consumer");
    let consumer = format!("cat > '{}'", log.display());
    tmux.tmux(&["pipe-pane", "-O", "-t", "%0", &consumer]);
    let mut plugin = Plugin::start(&tmux, &[]);
    plugin.next();
    plugin.snapshot_where("busy pipe stays unknown", |rows| {
        rows[0].detail.as_deref() == Some("status unknown")
    });
    emit_status(&tmux, "%0", "blocked");
    wait_for("existing consumer receives output", || {
        std::fs::read(&log).is_ok_and(|b| b.windows(7).any(|w| w == b"blocked"))
    });
    assert_eq!(tmux.state("%0").status, None);
    tmux.tmux(&["pipe-pane", "-t", "%0"]);
    wait_for("observer attaches", || {
        tmux.tmux(&["display-message", "-p", "-t", "%0", "#{pane_pipe}"]) == "1"
    });
    emit_status(&tmux, "%0", "blocked");
    wait_for("observed waiting", || {
        tmux.state("%0").status == Some(Status::Waiting)
    });
    tmux.tmux(&["pipe-pane", "-O", "-t", "%0", &consumer]);
    wait_for("lost observation becomes unknown", || {
        tmux.state("%0").terminal.unwrap().state
            == tmux_aside_agents::terminal::TerminalStatus::Unknown
    });
    plugin.close_stdin();
    assert!(plugin.exited_within(Duration::from_secs(3)));
    assert_eq!(
        tmux.tmux(&["display-message", "-p", "-t", "%0", "#{pane_pipe}"]),
        "1"
    );
}

#[test]
fn restarting_the_agent_in_the_same_pane_does_not_reuse_its_old_status() {
    let tmux = Tmux::start();
    emulate_codex(&tmux, "%0");
    let mut plugin = Plugin::start(&tmux, &[]);
    plugin.next();
    plugin.snapshot_where("first unknown", |rows| {
        rows[0].detail.as_deref() == Some("status unknown")
    });
    emit_status(&tmux, "%0", "blocked");
    wait_for("waiting", || {
        tmux.state("%0").status == Some(Status::Waiting)
    });
    let old = tmux.state("%0").terminal.unwrap().process_id;
    emulate_codex(&tmux, "%0");
    wait_for("new process unknown", || {
        let Ok(bytes) = std::fs::read(tmux.state_file("%0")) else {
            return false;
        };
        let Ok(s) = serde_json::from_slice::<tmux_aside_agents::AgentState>(&bytes) else {
            return false;
        };
        s.terminal.is_some_and(|t| {
            t.process_id != old && t.state == tmux_aside_agents::terminal::TerminalStatus::Unknown
        })
    });
}
