use std::collections::HashMap;

use tmux_aside_agents::{
    AgentState, Status, parse_window_statuses, window_status_updates, window_statuses,
};

fn agent(pane: &str, window_id: &str, status: Option<Status>) -> AgentState {
    AgentState {
        pane_id: pane.to_string(),
        pane_pid: 100,
        command: "codex".to_string(),
        project: "app".to_string(),
        window: "app".to_string(),
        status,
        status_ts: 0,
        activity_ts: 0,
        title: String::new(),
        workdir: String::new(),
        window_id: window_id.to_string(),
        window_index: 0,
        diff: None,
        stalled: false,
        inferred: false,
        background: false,
        ready: false,
        terminal: None,
    }
}

#[test]
fn each_window_publishes_the_status_of_its_own_agent() {
    let agents = [
        agent("%1", "@1", Some(Status::Working)),
        agent("%2", "@2", Some(Status::Waiting)),
        agent("%3", "@3", None),
        agent("%4", "", Some(Status::Done)),
    ];
    let statuses = window_statuses(&agents);
    assert_eq!(
        statuses,
        HashMap::from([
            ("@1".to_string(), Status::Working),
            ("@2".to_string(), Status::Waiting),
        ])
    );
}

#[test]
fn a_window_with_several_agents_publishes_the_most_urgent_status() {
    let agents = [
        agent("%1", "@1", Some(Status::Working)),
        agent("%2", "@1", Some(Status::Done)),
        agent("%3", "@1", Some(Status::Waiting)),
    ];
    assert_eq!(window_statuses(&agents)["@1"], Status::Waiting);
}

#[test]
fn only_changed_windows_are_written_and_vanished_ones_are_cleared() {
    let published = HashMap::from([
        ("@1".to_string(), Status::Working),
        ("@2".to_string(), Status::Waiting),
        ("@3".to_string(), Status::Done),
    ]);
    let current = HashMap::from([
        ("@1".to_string(), Status::Working),
        ("@2".to_string(), Status::Done),
        ("@4".to_string(), Status::Working),
    ]);
    assert_eq!(
        window_status_updates(&published, &current),
        vec![
            ("@2".to_string(), Some(Status::Done)),
            ("@3".to_string(), None),
            ("@4".to_string(), Some(Status::Working)),
        ]
    );
}

#[test]
fn statuses_left_in_tmux_by_an_earlier_run_are_read_back() {
    let output = "@0\twaiting\n@7\t\n@11\tworking\n@12\tbogus\n";
    assert_eq!(
        parse_window_statuses(output),
        HashMap::from([
            ("@0".to_string(), Status::Waiting),
            ("@11".to_string(), Status::Working),
        ])
    );
}
