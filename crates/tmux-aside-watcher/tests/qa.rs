mod support;

use serde_json::{Value, json};
use support::{Scratch, found, launch, node};
use tmux_aside::launch::Launch;
use tmux_aside::protocol::Tone;

fn merged(labels: &[&str]) -> Value {
    let nodes: Vec<Value> = labels.iter().map(|name| json!({"name": name})).collect();
    node(json!({"state": "MERGED", "labels": {"nodes": nodes}}))
}

/// A launch of `feat/x` that followed its pull request, now merged with `labels`; its worktree
/// is gone.
fn setup(labels: &[&str]) -> Scratch {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    scratch.save(&Launch {
        pull_request: Some("https://github.com/org/shop/pull/12".to_string()),
        ..launch(&shop, "feat/x")
    });
    scratch.respond(found(&[Some(merged(labels))]));
    scratch
}

fn line(rows: &[tmux_aside::protocol::Row]) -> &str {
    rows.first()
        .and_then(|row| row.lines.first())
        .map_or("", String::as_str)
}

#[test]
fn a_merged_pull_request_is_in_qa() {
    let scratch = setup(&[]);

    let plugin = scratch.start("30s");

    plugin.rows_where("QA", |rows| line(rows) == "QA · waiting for QA");
}

#[test]
fn the_passed_label_deploys_it_by_default_and_its_row_leaves() {
    let scratch = setup(&["qa:passed"]);
    let plugin = scratch.start("30s");
    plugin.rows_where("the launch", |rows| !rows.is_empty());

    plugin.rows_where("the row to leave", |rows| rows.is_empty());

    assert!(
        !scratch
            .0
            .join("state/tmux-aside/launches/shop__feat-x.json")
            .exists()
    );
}

#[test]
fn the_failed_label_keeps_it_in_qa_with_a_failure() {
    let scratch = setup(&["qa:failed"]);

    let plugin = scratch.start("30s");

    let rows = plugin.rows_where("the failure", |rows| line(rows) == "QA · QA failed");
    assert_eq!(rows[0].bar_tone, Tone::Danger);
}

#[test]
fn the_label_names_are_criterion_arguments_in_the_config() {
    let scratch = setup(&["qa:ok"]);
    scratch.configure(
        "criteria:\n  ready_to_deploy:\n    args: [\"--passed-label\", \"qa:ok\", \"--failed-label\", \"qa:ko\"]\n",
    );
    let plugin = scratch.start("30s");
    plugin.rows_where("the launch", |rows| !rows.is_empty());

    plugin.rows_where("the row to leave", |rows| rows.is_empty());
}

#[test]
fn a_replaced_deployed_criterion_keeps_it_ready_to_deploy_until_in_production() {
    let scratch = setup(&["qa:passed"]);
    let answer = json!({"met": false, "summary": "waiting for the release"});
    scratch.configure(&format!(
        "criteria:\n  deployed:\n    command: {}/tests/fixtures/criteria/answer\n    args: [{}, {}]\n",
        env!("CARGO_MANIFEST_DIR"),
        serde_json::to_string(scratch.0.join("question.json").to_str().unwrap()).unwrap(),
        serde_json::to_string(&answer.to_string()).unwrap(),
    ));

    let plugin = scratch.start("30s");

    plugin.rows_where("ready to deploy", |rows| {
        line(rows) == "Ready to deploy · waiting for the release"
    });
}
