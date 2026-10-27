mod common;

use std::process::Command;

use common::daemon::{Daemon, Home, fixture, wait_for};
use tmux_aside::view::View;

fn config(sections: &[String]) -> String {
    format!("sections:\n{}", sections.concat())
}

fn section(plugin: &str, args: &[&str], extra: &str) -> String {
    let args = args
        .iter()
        .map(|arg| format!("\"{arg}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!("  - plugin: \"{plugin}\"\n    args: [{args}]\n{extra}")
}

fn first_row(view: &View) -> &str {
    view.sections[0]
        .rows
        .first()
        .map(|row| row.text.as_str())
        .unwrap_or("")
}

fn row_texts(view: &View) -> Vec<&str> {
    view.sections[0]
        .rows
        .iter()
        .map(|row| row.text.as_str())
        .collect()
}

fn lines_in(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path)
        .map(|content| content.lines().count())
        .unwrap_or(0)
}

#[test]
fn sections_show_the_plugin_name_and_loading_until_the_first_snapshot() {
    let daemon = Daemon::with_config(&config(&[section(
        &fixture("hang"),
        &["5"],
        "    interval: 1s\n",
    )]));

    let view = daemon.connect().next_view();

    assert_eq!(view.sections[0].title, "hang");
    assert_eq!(row_texts(&view), ["loading..."]);
}

#[test]
fn a_configured_title_replaces_the_plugin_title() {
    let daemon = Daemon::with_config(&config(&[section(
        &fixture("rows"),
        &["Fixture", "1"],
        "    interval: 1s\n    title: Renamed\n    counts: false\n",
    )]));

    let view = daemon
        .connect()
        .view_where("snapshot", |view| first_row(view) == "row 1");

    assert_eq!(view.sections[0].title, "Renamed");
    assert!(!view.sections[0].counts);
}

#[test]
fn an_interval_plugin_snapshot_fills_its_section() {
    let daemon = Daemon::with_config(&config(&[section(
        &fixture("rows"),
        &["Fixture", "3"],
        "    interval: 1s\n    max_height: 2\n",
    )]));

    let view = daemon
        .connect()
        .view_where("snapshot", |view| view.sections[0].title == "Fixture");

    assert_eq!(row_texts(&view), ["row 1", "row 2", "row 3"]);
    assert_eq!(view.sections[0].max_height, Some(2));
}

#[test]
fn interval_plugins_run_again_every_interval() {
    let home = Home::new("plugins");
    let counter = home.path("runs");
    home.write_config(&config(&[section(
        &fixture("counter"),
        &[counter.to_str().unwrap()],
        "    interval: 100ms\n",
    )]));
    let daemon = Daemon::start_in(home, 10_000);

    let view = daemon
        .connect()
        .view_where("third run", |view| first_row(view) == "runs 3");

    assert_eq!(view.sections[0].title, "Counter");
}

#[test]
fn a_failing_plugin_shows_its_exit_status_and_stderr_and_is_retried() {
    let home = Home::new("plugins");
    let runs = home.path("runs");
    home.write_config(&config(&[section(
        &fixture("fail"),
        &[runs.to_str().unwrap()],
        "    interval: 100ms\n",
    )]));
    let daemon = Daemon::start_in(home, 10_000);

    let view = daemon
        .connect()
        .view_where("error row", |view| first_row(view).starts_with('!'));

    let row = first_row(&view);
    assert!(row.contains("exited with status 3"), "{row}");
    assert!(row.contains("boom"), "{row}");
    wait_for("a retry", || lines_in(&runs) >= 2);
    let log = std::fs::read_to_string(daemon.home.path("state/tmux-aside/0-fail.log")).unwrap();
    assert!(log.contains("boom"), "{log}");
}

#[test]
fn a_hanging_plugin_times_out_and_its_whole_process_group_is_killed() {
    let marker = format!("7{}", std::process::id() % 1000);
    let daemon = Daemon::with_config(&config(&[section(
        &fixture("hang"),
        &[&marker],
        "    interval: 1s\n    timeout: 200ms\n",
    )]));

    let view = daemon
        .connect()
        .view_where("timeout row", |view| first_row(view).starts_with('!'));

    assert!(
        first_row(&view).contains("timed out after 200ms"),
        "{view:?}"
    );
    let sleeping = Command::new("pgrep")
        .args(["-f", &format!("sleep {marker}")])
        .output()
        .unwrap();
    assert!(
        sleeping.stdout.is_empty(),
        "orphaned sleep: {}",
        String::from_utf8_lossy(&sleeping.stdout)
    );
}

#[test]
fn a_plugin_that_skips_the_hello_is_rejected() {
    let daemon = Daemon::with_config(&config(&[section(
        &fixture("no-hello"),
        &[],
        "    interval: 1s\n",
    )]));

    let view = daemon
        .connect()
        .view_where("error row", |view| first_row(view).starts_with('!'));

    assert!(first_row(&view).contains("hello"), "{view:?}");
}

#[test]
fn a_plugin_speaking_another_protocol_version_is_rejected() {
    let daemon = Daemon::with_config(&config(&[section(
        &fixture("future-protocol"),
        &[],
        "    interval: 1s\n",
    )]));

    let view = daemon
        .connect()
        .view_where("error row", |view| first_row(view).starts_with('!'));

    assert!(first_row(&view).contains("protocol 99"), "{view:?}");
}

#[test]
fn an_unknown_plugin_name_says_which_executable_is_missing() {
    let daemon = Daemon::with_config(&config(&[section(
        "does-not-exist",
        &[],
        "    interval: 1s\n",
    )]));

    let view = daemon
        .connect()
        .view_where("error row", |view| first_row(view).starts_with('!'));

    assert!(
        first_row(&view).contains("tmux-aside-does-not-exist"),
        "{view:?}"
    );
}

fn running(pattern: &str) -> String {
    let output = Command::new("pgrep")
        .args(["-fl", pattern])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn a_stream_plugin_updates_its_section_with_every_line() {
    let daemon = Daemon::with_config(&config(&[section(
        &fixture("ticker"),
        &["0.1"],
        "    mode: stream\n",
    )]));

    let view = daemon
        .connect()
        .view_where("third tick", |view| first_row(view) == "tick 3");

    assert_eq!(view.sections[0].title, "Ticker");
}

#[test]
fn a_crashing_stream_plugin_shows_the_exit_and_is_restarted() {
    let home = Home::new("plugins");
    let starts = home.path("starts");
    home.write_config(&config(&[section(
        &fixture("crash-stream"),
        &[starts.to_str().unwrap()],
        "    mode: stream\n",
    )]));
    let daemon = Daemon::start_in(home, 10_000);

    let view = daemon
        .connect()
        .view_where("exit row", |view| first_row(view).starts_with('!'));

    assert!(
        first_row(&view).contains("exited with status 1"),
        "{view:?}"
    );
    wait_for("a restart", || lines_in(&starts) >= 2);
}

#[test]
fn stream_plugins_die_with_the_daemon() {
    let marker = format!("0.1{}", std::process::id());
    let daemon = Daemon::with_config(&config(&[section(
        &fixture("ticker"),
        &[&marker],
        "    mode: stream\n",
    )]));
    daemon
        .connect()
        .view_where("first tick", |view| first_row(view).starts_with("tick"));
    let pattern = format!("ticker {marker}");
    assert!(!running(&pattern).is_empty());

    drop(daemon);

    wait_for("ticker to exit", || running(&pattern).is_empty());
}

#[test]
fn plugin_names_resolve_to_tmux_aside_executables_on_path() {
    let home = Home::new("plugins");
    let bin = home.path("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(fixture("rows"), bin.join("tmux-aside-rows")).unwrap();
    home.write_config(&config(&[section(
        "rows",
        &["From PATH", "1"],
        "    interval: 1s\n",
    )]));
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let socket = home.path("aside.sock");
    let child = common::daemon::daemon_command(&home, &socket)
        .env("PATH", path)
        .spawn()
        .unwrap();
    wait_for("socket", || socket.exists());
    let daemon = Daemon {
        child,
        socket,
        home,
    };

    let view = daemon
        .connect()
        .view_where("snapshot", |view| view.sections[0].title == "From PATH");

    assert_eq!(row_texts(&view), ["row 1"]);
}

#[test]
fn reload_rereads_the_config_and_keeps_unchanged_sections() {
    let kept = section(&fixture("rows"), &["Kept", "1"], "    interval: 1s\n");
    let daemon = Daemon::with_config(&config(std::slice::from_ref(&kept)));
    let mut client = daemon.connect();
    client.view_where("snapshot", |view| view.sections[0].title == "Kept");

    daemon.home.write_config(&config(&[
        kept,
        section(&fixture("rows"), &["Added", "2"], "    interval: 1s\n"),
    ]));
    client.send(r#"{"type":"reload"}"#);

    let view = client.view_where("reloaded", |view| {
        view.sections.len() == 2 && view.sections[1].title == "Added"
    });
    assert_eq!(view.sections[0].title, "Kept");
}

#[test]
fn auto_reload_picks_up_config_changes_without_a_refresh() {
    let daemon = Daemon::with_config(&format!(
        "auto_reload: true\n{}",
        config(&[section(
            &fixture("rows"),
            &["Before", "1"],
            "    interval: 1s\n"
        )])
    ));
    let mut client = daemon.connect();
    client.view_where("snapshot", |view| view.sections[0].title == "Before");

    std::thread::sleep(std::time::Duration::from_millis(1100));
    daemon.home.write_config(&format!(
        "auto_reload: true\n{}",
        config(&[section(
            &fixture("rows"),
            &["After", "1"],
            "    interval: 1s\n"
        )])
    ));

    client.view_where("auto reloaded", |view| view.sections[0].title == "After");
}

#[test]
fn plugins_receive_the_configured_language() {
    let daemon = Daemon::with_config(&format!(
        "language: es\n{}",
        config(&[section(&fixture("language"), &[], "    interval: 1s\n")])
    ));

    let view = daemon
        .connect()
        .view_where("snapshot", |view| first_row(view) != "cargando...");

    assert_eq!(first_row(&view), "es");
}

#[test]
fn sections_wait_in_the_configured_language() {
    let daemon = Daemon::with_config(&format!(
        "language: es\n{}",
        config(&[section(&fixture("hang"), &["5"], "    interval: 1s\n")])
    ));

    let view = daemon.connect().next_view();

    assert_eq!(row_texts(&view), ["cargando..."]);
}
