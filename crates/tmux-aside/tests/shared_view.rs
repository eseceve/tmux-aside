mod common;

use common::TmuxServer;

fn shows_fixture(server: &TmuxServer, pane_id: &str) -> bool {
    server.capture(pane_id).contains("Fixture")
}

#[test]
fn every_sidebar_shows_the_same_sections() {
    let server = TmuxServer::start(2);

    server.aside(&["on"]);

    for sidebar in server.sidebars() {
        server.wait_until("sidebar content", |server| {
            shows_fixture(server, &sidebar.pane_id)
        });
    }
}

#[test]
fn unfocused_sidebars_do_not_highlight_the_selection() {
    let server = TmuxServer::start(1);
    server.aside(&["on"]);
    let sidebar = server.sidebars().remove(0);
    server.wait_until("sidebar content", |server| {
        shows_fixture(server, &sidebar.pane_id)
    });

    assert_eq!(server.selected_line(&sidebar.pane_id), None);

    server.focus(&sidebar.pane_id);
    server.wait_until("highlight on focus", |server| {
        server.selected_line(&sidebar.pane_id).is_some()
    });

    server.tmux(&["send-keys", "-H", "-t", &sidebar.pane_id, "1b", "5b", "4f"]);
    server.wait_until("highlight gone on blur", |server| {
        server.selected_line(&sidebar.pane_id).is_none()
    });
}

#[test]
fn a_key_in_one_sidebar_moves_the_selection_in_every_sidebar() {
    let server = TmuxServer::start(2);
    server.aside(&["on"]);
    let sidebars = server.sidebars();
    let (typed_in, other) = (&sidebars[0], &sidebars[1]);
    server.wait_until("sidebar content", |server| {
        shows_fixture(server, &other.pane_id)
    });
    server.focus(&other.pane_id);

    server.tmux(&["send-keys", "-t", &typed_in.pane_id, "j"]);

    server.wait_until("selection to move in the other window", |server| {
        server
            .selected_line(&other.pane_id)
            .is_some_and(|line| line.contains("row 1"))
    });
}

#[test]
fn typed_keys_are_not_echoed_into_the_sidebar() {
    let server = TmuxServer::start(1);
    server.aside(&["on"]);
    let sidebar = server.sidebars().remove(0);
    server.wait_until("sidebar content", |server| {
        shows_fixture(server, &sidebar.pane_id)
    });

    server.tmux(&["send-keys", "-t", &sidebar.pane_id, "pwd", "Enter"]);
    std::thread::sleep(std::time::Duration::from_millis(300));

    assert!(!server.capture(&sidebar.pane_id).contains("pwd"));
}

#[test]
fn turning_off_and_on_again_shows_content_right_away() {
    let server = TmuxServer::start(1);
    let shows_content = |server: &TmuxServer| {
        server
            .sidebars()
            .first()
            .is_some_and(|sidebar| shows_fixture(server, &sidebar.pane_id))
    };
    server.aside(&["on"]);
    server.wait_until("content after first on", shows_content);

    server.aside(&["off"]);
    server.aside(&["on"]);

    server.wait_until("content after off and on", shows_content);
}

#[test]
fn enter_runs_the_selected_rows_action_from_the_sidebar() {
    let server = TmuxServer::start_with(1, |home| {
        format!(
            "sections:\n  - plugin: \"{}\"\n    args: [\"{}\"]\n    interval: 60s\n",
            common::daemon::fixture("action"),
            home.path("touched").display()
        )
    });
    let marker = server.home.path("touched");
    server.aside(&["on"]);
    let sidebar = server.sidebars().remove(0);
    server.wait_until("action row", |server| {
        server.capture(&sidebar.pane_id).contains("touch it")
    });

    server.tmux(&["send-keys", "-t", &sidebar.pane_id, "j", "Enter"]);

    server.wait_until("action to run", |_| marker.exists());
}

fn action_server(extra: &str) -> TmuxServer {
    let extra = extra.to_string();
    TmuxServer::start_with(1, move |home| {
        format!(
            "{extra}sections:\n  - plugin: \"{}\"\n    args: [\"{}\"]\n    interval: 60s\n",
            common::daemon::fixture("action"),
            home.path("touched").display()
        )
    })
}

#[test]
fn other_keys_run_the_selected_rows_action_bound_to_them() {
    let server = action_server("");
    let marker = server.home.path("touched.x");
    server.aside(&["on"]);
    let sidebar = server.sidebars().remove(0);
    server.wait_until("action row", |server| {
        server.capture(&sidebar.pane_id).contains("touch it")
    });

    server.tmux(&["send-keys", "-t", &sidebar.pane_id, "j", "x"]);

    server.wait_until("x action to run", |_| marker.exists());
}

#[test]
fn in_select_mode_enter_falls_back_to_the_action_when_the_window_has_no_sidebar() {
    let server = action_server("enter: select\n");
    let marker = server.home.path("touched");
    server.aside(&["on"]);
    let sidebar = server.sidebars().remove(0);
    server.wait_until("action row", |server| {
        server.capture(&sidebar.pane_id).contains("touch it")
    });

    server.tmux(&["send-keys", "-t", &sidebar.pane_id, "j", "Enter"]);

    server.wait_until("fallback action to run", |_| marker.exists());
}

#[test]
fn o_runs_the_selected_rows_action_in_select_mode() {
    let server = action_server("enter: select\n");
    let marker = server.home.path("touched");
    server.aside(&["on"]);
    let sidebar = server.sidebars().remove(0);
    server.wait_until("action row", |server| {
        server.capture(&sidebar.pane_id).contains("touch it")
    });

    server.tmux(&["send-keys", "-t", &sidebar.pane_id, "j", "o"]);

    server.wait_until("action to run", |_| marker.exists());
}
