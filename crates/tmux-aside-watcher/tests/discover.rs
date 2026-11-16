mod support;

use std::path::Path;

use serde_json::{Value, json};
use support::{Scratch, Tmux, found, node, wait_until};
use tmux_aside::launch::Launch;

const URL: &str = "https://github.com/org/shop/pull/12";

fn recorded(scratch: &Scratch) -> Vec<Launch> {
    let dir = scratch.0.join("state/tmux-aside/launches");
    let mut launches: Vec<Launch> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                .map(|entry| {
                    serde_json::from_str(&std::fs::read_to_string(entry.path()).unwrap()).unwrap()
                })
                .collect()
        })
        .unwrap_or_default();
    launches.sort_by(|a, b| a.id.cmp(&b.id));
    launches
}

fn open_pull_request(changes: Value) -> Value {
    let mut node = node(json!({"headRefName": "feat/x"}));
    for (key, value) in changes.as_object().unwrap() {
        node[key] = value.clone();
    }
    node
}

#[test]
fn a_searched_pull_request_is_followed_once_without_an_agent() {
    let scratch = Scratch::new();
    scratch.configure("discover:\n  - search: \"is:pr is:open author:@me\"\n");
    scratch.respond_search(&[open_pull_request(json!({}))]);
    scratch.respond(found(&[Some(open_pull_request(json!({})))]));

    let plugin = scratch.start("200ms");

    let rows = plugin.rows_where("the discovered pull request", |rows| {
        rows.iter().any(|row| row.detail.as_deref() == Some("#12"))
    });
    assert!(rows[0].action("enter").is_none(), "no agent to jump to");
    wait_until("a few fetches", || scratch.calls().len() >= 6);
    let launches = recorded(&scratch);
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].id, "shop__pr-12");
    assert_eq!(launches[0].pull_request.as_deref(), Some(URL));
    assert_eq!(launches[0].branch, "feat/x");
    assert!(launches[0].keeps_worktree);
}

#[test]
fn a_pull_request_listed_by_a_command_is_followed() {
    let scratch = Scratch::new();
    let lister = scratch.0.join("bin/pending");
    std::fs::write(&lister, format!("#!/bin/sh\necho {URL}\n")).unwrap();
    std::process::Command::new("chmod")
        .arg("+x")
        .arg(&lister)
        .status()
        .unwrap();
    scratch.configure(&format!("discover:\n  - command: {}\n", lister.display()));
    scratch.respond(found(&[Some(open_pull_request(
        json!({"state": "MERGED"}),
    ))]));

    let plugin = scratch.start("200ms");

    plugin.rows_where("the merged pull request in QA", |rows| {
        rows.first()
            .is_some_and(|row| row.lines == ["QA · waiting for QA"])
    });
}

#[test]
fn a_pull_request_a_launch_already_works_on_is_not_followed_twice() {
    let scratch = Scratch::new();
    scratch.configure("discover:\n  - search: \"is:pr is:open author:@me\"\n");
    scratch.record(&scratch.repository("shop"), "feat/x", true);
    scratch.respond_search(&[open_pull_request(json!({}))]);
    scratch.respond(found(&[Some(open_pull_request(json!({})))]));

    let _plugin = scratch.start("200ms");
    wait_until("a few fetches", || scratch.calls().len() >= 6);

    let launches = recorded(&scratch);
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].id, "shop__feat-x");
}

#[test]
fn the_agent_working_on_the_branch_is_linked_and_nudged_and_its_worktree_is_kept() {
    let scratch = Scratch::new();
    let tmux = Tmux::start();
    let clone = scratch.repository("shop");
    support::git(&clone, &["checkout", "-q", "-b", "feat/x"]);
    support::git(
        &clone,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ],
    );
    let pane = tmux.agent_pane(&scratch.0.join("typed"), &clone);
    tmux.report(&scratch, &pane, "done");
    scratch.configure("discover:\n  - search: \"is:pr is:open author:@me\"\n");
    let conflicting = open_pull_request(json!({"mergeable": "CONFLICTING"}));
    scratch.respond_search(std::slice::from_ref(&conflicting));
    scratch.respond(found(&[Some(conflicting)]));

    let _plugin = scratch.start_in("200ms", Some(&tmux));

    wait_until("the nudge", || {
        std::fs::read_to_string(scratch.0.join("typed"))
            .is_ok_and(|typed| typed.contains("conflicts"))
    });
    let launch = &recorded(&scratch)[0];
    assert_eq!(launch.pane, pane);
    assert_eq!(
        std::fs::canonicalize(&launch.worktree).unwrap(),
        std::fs::canonicalize(Path::new(&clone)).unwrap()
    );
    assert!(launch.keeps_worktree);
}

#[test]
fn discovery_does_not_take_a_review_agents_pane() {
    let scratch = Scratch::new();
    let tmux = Tmux::start();
    let clone = scratch.repository("shop");
    support::git(&clone, &["checkout", "-q", "-b", "feat/x"]);
    let pane = tmux.agent_pane(&scratch.0.join("typed"), &clone);
    tmux.report(&scratch, &pane, "done");
    let mut review = support::launch(&clone, "feat/x");
    review.id = "shop__review".into();
    review.pane = pane;
    review.opens_pull_request = false;
    review.pull_request = None;
    review.template = Some("review".into());
    scratch.save(&review);
    scratch.configure("discover:\n  - search: \"is:pr is:open author:@me\"\n");
    let conflicting = open_pull_request(json!({"mergeable": "CONFLICTING"}));
    scratch.respond_search(std::slice::from_ref(&conflicting));
    scratch.respond(found(&[Some(conflicting)]));
    let _plugin = scratch.start_in("200ms", Some(&tmux));
    wait_until("discovery cycles", || scratch.calls().len() >= 6);
    let launches = recorded(&scratch);
    let discovered = launches
        .iter()
        .find(|launch| launch.opens_pull_request)
        .unwrap();
    assert!(discovered.pane.is_empty());
    assert!(
        std::fs::read_to_string(scratch.0.join("typed"))
            .unwrap_or_default()
            .is_empty()
    );
}
