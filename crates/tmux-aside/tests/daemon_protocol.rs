mod common;

use common::daemon::{Daemon, daemon_command, wait_for};
use tmux_aside::view::{Cursor, Target};

#[test]
fn new_clients_receive_the_current_view() {
    let daemon = Daemon::start(10_000);
    let mut client = daemon.connect();

    let view = client.next_view();

    assert!(!view.sections.is_empty());
    assert_eq!(
        view.selected,
        Some(Cursor {
            section: 0,
            target: Target::Header,
        })
    );
}

#[test]
fn a_key_from_one_client_updates_every_client() {
    let daemon = Daemon::start(10_000);
    let mut sender = daemon.connect();
    let mut watcher = daemon.connect();
    sender.next_view();
    watcher.next_view();

    sender.send(r#"{"type":"key","key":"down"}"#);

    let expected = Some(Cursor {
        section: 0,
        target: Target::Row(0),
    });
    assert_eq!(sender.next_view().selected, expected);
    assert_eq!(watcher.next_view().selected, expected);
}

#[test]
fn focus_from_a_window_without_rows_keeps_the_selection() {
    let daemon = Daemon::start(10_000);
    let mut client = daemon.connect();
    client.next_view();

    client.send(r#"{"type":"focus","window":"@999"}"#);

    assert_eq!(
        client.next_view().selected,
        Some(Cursor {
            section: 0,
            target: Target::Header,
        })
    );
}

#[test]
fn a_click_selects_the_line_and_a_click_on_a_hidden_line_is_ignored() {
    let daemon = Daemon::start(10_000);
    let mut client = daemon.connect();
    client.next_view();

    client.send(r#"{"type":"click","cursor":{"section":0,"target":{"kind":"row","value":0}}}"#);
    assert_eq!(
        client.next_view().selected,
        Some(Cursor {
            section: 0,
            target: Target::Row(0),
        })
    );

    client.send(r#"{"type":"click","cursor":{"section":9,"target":{"kind":"header"}}}"#);
    client.send(r#"{"type":"key","key":"up"}"#);
    assert_eq!(
        client.next_view().selected,
        Some(Cursor {
            section: 0,
            target: Target::Header,
        })
    );
}

#[test]
fn malformed_messages_are_ignored() {
    let daemon = Daemon::start(10_000);
    let mut client = daemon.connect();
    client.next_view();

    client.send("not json");
    client.send(r#"{"type":"key","key":"down"}"#);

    assert_eq!(
        client.next_view().selected,
        Some(Cursor {
            section: 0,
            target: Target::Row(0),
        })
    );
}

#[test]
fn daemon_exits_after_its_last_client_leaves() {
    let mut daemon = Daemon::start(300);
    let mut client = daemon.connect();
    client.next_view();

    drop(client);

    wait_for("daemon exit", || daemon.child.try_wait().unwrap().is_some());
    assert!(!daemon.socket.exists());
}

#[test]
fn a_second_daemon_on_the_same_socket_exits_and_leaves_the_first_running() {
    let daemon = Daemon::start(10_000);

    let status = daemon_command(&daemon.home, &daemon.socket)
        .status()
        .unwrap();

    assert!(status.success());
    daemon.connect().next_view();
}

#[test]
fn without_a_config_the_sidebar_says_where_to_create_it() {
    let daemon = Daemon::start(10_000);

    let view = daemon.connect().next_view();

    assert_eq!(view.sections.len(), 1);
    assert!(
        view.sections[0].rows[0]
            .text
            .contains("tmux-aside/config.yaml"),
        "{view:?}"
    );
}

#[test]
fn an_invalid_config_is_shown_instead_of_crashing() {
    let daemon = Daemon::with_config("sections:\n  - plugin: clock\n    intervl: 1s\n");

    let view = daemon.connect().next_view();

    assert!(
        view.sections[0].rows[0].text.contains("intervl"),
        "{view:?}"
    );
}
