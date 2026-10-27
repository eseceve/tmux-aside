mod common;

use common::TmuxServer;

#[test]
fn on_adds_one_unfocused_sidebar_per_window() {
    let server = TmuxServer::start(2);

    server.aside(&["on", "--width", "30"]);

    let sidebars = server.sidebars();
    let mut sidebar_windows: Vec<String> =
        sidebars.iter().map(|pane| pane.window_id.clone()).collect();
    sidebar_windows.sort();
    let mut windows = server.window_ids();
    windows.sort();
    assert_eq!(sidebar_windows, windows);
    assert!(
        sidebars.iter().all(|pane| pane.width == 30 && !pane.active),
        "{sidebars:?}"
    );
}

#[test]
fn repeating_on_applies_the_new_width_without_duplicates() {
    let server = TmuxServer::start(2);

    server.aside(&["on", "--width", "30"]);
    server.aside(&["on", "--width", "40"]);

    let sidebars = server.sidebars();
    assert_eq!(sidebars.len(), 2, "{sidebars:?}");
    assert!(sidebars.iter().all(|pane| pane.width == 40), "{sidebars:?}");
}

#[test]
fn windows_created_after_on_get_a_sidebar() {
    let server = TmuxServer::start(1);
    server.aside(&["on", "--width", "30"]);

    server.tmux(&["new-window", "-t", "main"]);

    server.wait_until("sidebar in new window", |server| {
        server.sidebars().len() == 2
    });
    assert!(
        server
            .sidebars()
            .iter()
            .all(|pane| pane.width == 30 && !pane.active)
    );
}

#[test]
fn off_removes_sidebars_and_its_hooks_but_keeps_user_hooks() {
    let server = TmuxServer::start(2);
    server.tmux(&[
        "set-hook",
        "-g",
        "after-new-window[0]",
        "display-message user-hook",
    ]);
    server.aside(&["on"]);

    server.aside(&["off"]);

    assert!(server.sidebars().is_empty());
    let hooks = server.global_hooks();
    assert!(hooks.contains("user-hook"), "{hooks}");
    assert!(!hooks.contains("_sync"), "{hooks}");
}

#[test]
fn toggle_turns_the_sidebar_on_and_off() {
    let server = TmuxServer::start(2);

    server.aside(&["toggle"]);
    assert_eq!(server.sidebars().len(), 2);

    server.aside(&["toggle"]);
    assert!(server.sidebars().is_empty());
}

#[test]
fn default_width_follows_window_resizes() {
    let server = TmuxServer::start(1);
    server.aside(&["on"]);
    assert_eq!(server.sidebars()[0].width, 25);

    server.tmux(&["resize-window", "-t", "main", "-x", "400"]);

    server.wait_until("sidebar reflow", |server| server.sidebars()[0].width == 40);
}

#[test]
fn sessions_created_after_on_get_a_sidebar() {
    let server = TmuxServer::start(1);
    server.aside(&["on", "--width", "30"]);

    server.tmux(&["new-session", "-d", "-s", "other"]);

    server.wait_until("sidebar in new session", |server| {
        server.sidebars().len() == 2
    });
}

#[test]
fn window_closes_when_only_the_sidebar_is_left() {
    let server = TmuxServer::start(2);
    server.aside(&["on"]);
    let content_pane = server
        .panes()
        .into_iter()
        .find(|pane| !pane.is_sidebar)
        .unwrap();

    server.tmux(&["kill-pane", "-t", &content_pane.pane_id]);

    server.wait_until("sidebar-only window to close", |server| {
        server.window_ids().len() == 1
    });
    assert_eq!(server.sidebars().len(), 1);
}

#[test]
fn window_closes_when_the_last_content_process_exits() {
    let server = TmuxServer::start(2);
    server.aside(&["on"]);
    let content_pane = server
        .panes()
        .into_iter()
        .find(|pane| !pane.is_sidebar)
        .unwrap();

    server.tmux(&["send-keys", "-t", &content_pane.pane_id, "exit", "Enter"]);

    server.wait_until("sidebar-only window to close", |server| {
        server.window_ids().len() == 1
    });
    assert_eq!(server.sidebars().len(), 1);
}

fn sidebar_in(server: &TmuxServer, window: usize) -> common::Pane {
    let window_id = &server.window_ids()[window];
    server
        .sidebars()
        .into_iter()
        .find(|pane| &pane.window_id == window_id)
        .unwrap()
}

#[test]
fn dragging_a_sidebar_without_an_explicit_width_resizes_every_sidebar() {
    let server = TmuxServer::start(2);
    server.aside(&["on"]);
    server.wait_until("clients running", |server| {
        server
            .sidebars()
            .iter()
            .all(|pane| server.capture(&pane.pane_id).contains("Fixture"))
    });
    let dragged = sidebar_in(&server, 0);

    server.tmux(&["resize-pane", "-t", &dragged.pane_id, "-x", "40"]);

    server.wait_until("other window to follow", |server| {
        sidebar_in(server, 1).width == 40
    });
    assert_eq!(sidebar_in(&server, 0).width, 40);
}

#[test]
fn dragging_a_sidebar_with_an_explicit_width_snaps_it_back() {
    let server = TmuxServer::start(2);
    server.aside(&["on", "--width", "30"]);
    server.wait_until("clients running", |server| {
        server
            .sidebars()
            .iter()
            .all(|pane| server.capture(&pane.pane_id).contains("Fixture"))
    });
    let dragged = sidebar_in(&server, 0);

    server.tmux(&["resize-pane", "-t", &dragged.pane_id, "-x", "45"]);

    server.wait_until("snap back", |server| sidebar_in(server, 0).width == 30);
    assert_eq!(sidebar_in(&server, 1).width, 30);
}

#[test]
fn a_dragged_width_survives_turning_the_sidebar_off_and_on() {
    let server = TmuxServer::start(1);
    server.aside(&["on"]);
    server.wait_until("client running", |server| {
        server
            .sidebars()
            .iter()
            .all(|pane| server.capture(&pane.pane_id).contains("Fixture"))
    });
    server.tmux(&[
        "resize-pane",
        "-t",
        &server.sidebars()[0].pane_id,
        "-x",
        "44",
    ]);
    server.wait_until("width stored", |server| {
        server.home.path("state/tmux-aside/settings.json").exists()
    });

    server.aside(&["off"]);
    server.aside(&["on"]);

    assert_eq!(server.sidebars()[0].width, 44);
}

#[test]
fn reload_fails_when_the_sidebar_is_not_running() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_tmux-aside"))
        .arg("reload")
        .env("TMUX", "/nonexistent/tmux-aside-test,1,0")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("tmux-aside is not running"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
