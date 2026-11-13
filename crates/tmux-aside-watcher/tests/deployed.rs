mod support;

use serde_json::json;
use support::{Scratch, found, launch, node, wait_until};
use tmux_aside::launch::Launch;

/// Launches of `feat/a` and `feat/b` whose pull requests 1 and 2 passed QA, so the default
/// Deployed Criterion deploys them, and a fake `tmux-aside` that logs where and how it runs.
fn deployed_pair(ran_on_deployed: bool) -> (Scratch, String) {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    for (branch, number) in [("feat/a", 1), ("feat/b", 2)] {
        scratch.save(&Launch {
            pull_request: Some(format!("https://github.com/org/shop/pull/{number}")),
            ran_on_deployed,
            ..launch(&shop, branch)
        });
    }
    let passed = |number: u64| {
        node(json!({
            "state": "MERGED",
            "number": number,
            "url": format!("https://github.com/org/shop/pull/{number}"),
            "labels": {"nodes": [{"name": "qa:passed"}]},
        }))
    };
    scratch.respond(found(&[Some(passed(1)), Some(passed(2))]));
    let fake = scratch.0.join("bin/tmux-aside");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nprintf '%s %s\\n' \"$(basename \"$PWD\")\" \"$*\" >> '{}'\n",
            scratch.0.join("deployed.log").display()
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

fn ran(scratch: &Scratch) -> Vec<String> {
    std::fs::read_to_string(scratch.0.join("deployed.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn pull_requests_deployed_in_one_fetch_run_the_deployed_action_once_together() {
    let (scratch, fake) = deployed_pair(false);
    scratch.configure("on_deployed:\n  template: sync\n");

    let plugin = scratch.start_with(&["--interval", "200ms", "--tmux-aside", &fake], None);

    wait_until("the deployed action", || !ran(&scratch).is_empty());
    plugin.rows_where("the rows to leave", |rows| rows.is_empty());
    assert_eq!(
        ran(&scratch),
        [
            "shop deployed --task Task of feat/a --task Task of feat/b https://github.com/org/shop/pull/1 https://github.com/org/shop/pull/2"
        ]
    );
}

#[test]
fn without_a_deployed_action_nothing_runs() {
    let (scratch, fake) = deployed_pair(false);

    let plugin = scratch.start_with(&["--interval", "200ms", "--tmux-aside", &fake], None);

    plugin.rows_where("the rows to leave", |rows| rows.is_empty());
    assert!(ran(&scratch).is_empty());
}

#[test]
fn the_deployed_action_never_runs_twice_for_a_pull_request() {
    let (scratch, fake) = deployed_pair(true);
    scratch.configure("on_deployed:\n  template: sync\n");

    let plugin = scratch.start_with(&["--interval", "200ms", "--tmux-aside", &fake], None);

    plugin.rows_where("the rows to leave", |rows| rows.is_empty());
    assert!(ran(&scratch).is_empty());
}
