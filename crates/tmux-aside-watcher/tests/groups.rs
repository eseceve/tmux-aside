mod support;

use serde_json::json;
use support::{Scratch, found, launch, node, wait_until};
use tmux_aside::launch::Launch;

/// Two launches of the Group `theme`, whose pull requests 1 and 2 are merged, and a fake
/// `tmux-aside` that logs each `qa` it is asked for.
fn ready_group() -> (Scratch, String) {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    for (branch, number) in [("feat/a", 1), ("feat/b", 2)] {
        scratch.save(&Launch {
            group: Some("theme".to_string()),
            pull_request: Some(format!("https://github.com/org/shop/pull/{number}")),
            ..launch(&shop, branch)
        });
    }
    let merged = |number: u64| {
        node(json!({
            "state": "MERGED",
            "number": number,
            "url": format!("https://github.com/org/shop/pull/{number}"),
        }))
    };
    scratch.respond(found(&[Some(merged(1)), Some(merged(2))]));
    let fake = scratch.0.join("bin/tmux-aside");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n",
            scratch.0.join("qa.log").display()
        ),
    )
    .unwrap();
    std::process::Command::new("chmod")
        .arg("+x")
        .arg(&fake)
        .status()
        .unwrap();
    (scratch, fake.to_string_lossy().into_owned())
}

fn launched_qa(scratch: &Scratch) -> Vec<String> {
    std::fs::read_to_string(scratch.0.join("qa.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn wait_for_fetches(scratch: &Scratch, more: usize) {
    let target = scratch.calls().len() + more;
    wait_until("more fetches", || scratch.calls().len() >= target);
}

#[test]
fn with_automatic_qa_one_qa_agent_is_launched_per_ready_group() {
    let (scratch, fake) = ready_group();
    scratch.configure("auto_qa: true\n");

    let _plugin = scratch.start_with(&["--interval", "200ms", "--tmux-aside", &fake], None);

    wait_until("the QA launch", || !launched_qa(&scratch).is_empty());
    wait_for_fetches(&scratch, 3);
    assert_eq!(
        launched_qa(&scratch),
        ["qa https://github.com/org/shop/pull/1 https://github.com/org/shop/pull/2"]
    );
}

#[test]
fn without_automatic_qa_nothing_is_launched_by_itself() {
    let (scratch, fake) = ready_group();

    let plugin = scratch.start_with(&["--interval", "200ms", "--tmux-aside", &fake], None);
    let rows = plugin.rows_where("the group in QA", |rows| {
        rows.len() == 2 && rows.iter().all(|row| row.action("q").is_some())
    });
    wait_for_fetches(&scratch, 3);

    assert!(launched_qa(&scratch).is_empty());
    assert_eq!(rows[0].group.as_deref(), Some("theme"));
}

#[test]
fn qa_reruns_for_a_failed_pull_request_and_its_fix_once_the_fix_reached_qa() {
    let (scratch, fake) = ready_group();
    scratch.configure("auto_qa: true\n");
    let pull_request = |number: u64, changes: serde_json::Value| {
        let mut node = node(json!({
            "number": number,
            "url": format!("https://github.com/org/shop/pull/{number}"),
        }));
        for (key, value) in changes.as_object().unwrap() {
            node[key] = value.clone();
        }
        node
    };
    let failed = pull_request(
        1,
        json!({"state": "MERGED", "labels": {"nodes": [{"name": "qa:failed"}]}}),
    );
    scratch.respond(found(&[
        Some(failed.clone()),
        Some(pull_request(2, json!({}))),
    ]));
    let plugin = scratch.start_with(&["--interval", "200ms", "--tmux-aside", &fake], None);
    plugin.rows_where("the failed QA waiting for the fix", |rows| {
        rows.iter()
            .any(|row| row.lines == ["QA · QA failed · waiting for #2"])
    });
    assert!(launched_qa(&scratch).is_empty());

    scratch.respond(found(&[
        Some(failed),
        Some(pull_request(2, json!({"state": "MERGED"}))),
    ]));

    wait_until("the QA rerun", || !launched_qa(&scratch).is_empty());
    assert_eq!(
        launched_qa(&scratch),
        ["qa https://github.com/org/shop/pull/1 https://github.com/org/shop/pull/2"]
    );
}
