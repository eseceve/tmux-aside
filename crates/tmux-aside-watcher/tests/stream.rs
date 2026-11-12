mod support;

use serde_json::json;
use support::{Scratch, found, node, stage};

#[test]
fn a_launched_pull_request_shows_its_stage() {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    scratch.record(&shop, "feat/ready", true);
    scratch.respond(found(&[Some(node(
        json!({"number": 12, "title": "PR 12"}),
    ))]));

    let plugin = scratch.start("30s");

    let rows = plugin.rows_where("a ready row", |rows| {
        rows.len() == 1 && stage(&rows[0]) == "Ready to merge"
    });
    assert_eq!(rows[0].text, "PR 12");
    assert_eq!(rows[0].detail.as_deref(), Some("#12"));
}

#[test]
fn a_launch_without_a_pull_request_is_implementing_until_its_branch_has_one() {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    scratch.record(&shop, "feat/new", true);
    scratch.respond(found(&[None]));
    let plugin = scratch.start("200ms");
    let rows = plugin.rows_where("an implementing row", |rows| rows.len() == 1);
    assert_eq!(rows[0].lines, ["Implementing · no pull request yet"]);
    assert_eq!(rows[0].text, "Task of feat/new");

    scratch.respond(found(&[Some(node(json!({"number": 13, "title": "PR 13", "url": "https://github.com/org/shop/pull/13", "reviewDecision": "REVIEW_REQUIRED"})))]));

    let rows = plugin.rows_where("the pull request", |rows| {
        rows[0].detail.as_deref() == Some("#13")
    });
    assert_eq!(rows[0].lines, ["Implementing · waiting for approval"]);
    assert_eq!(
        scratch.recorded("shop__feat-new").pull_request.as_deref(),
        Some("https://github.com/org/shop/pull/13"),
        "the registry learns the pull request"
    );
}

#[test]
fn launches_that_open_no_pull_request_are_not_shown() {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    scratch.record(&shop, "research/cache", false);
    scratch.record(&shop, "feat/shown", true);
    scratch.respond(found(&[None]));

    let plugin = scratch.start("30s");

    let rows = plugin.rows_where("one row", |rows| !rows.is_empty());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text, "Task of feat/shown");
}

#[test]
fn every_pull_request_is_fetched_in_one_query() {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    let api = scratch.repository("api");
    scratch.record(&shop, "feat/one", true);
    scratch.record(&api, "feat/two", true);
    scratch.respond(found(&[None, None]));

    let plugin = scratch.start("30s");
    plugin.rows_where("two rows", |rows| rows.len() == 2);

    let calls = scratch.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    for expected in ["n0=api", "b0=feat/two", "n1=shop", "b1=feat/one"] {
        assert!(
            calls[0].contains(expected),
            "{expected} missing: {}",
            calls[0]
        );
    }
}

#[test]
fn a_pull_request_that_stops_being_ready_falls_back_to_implementing() {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    scratch.record(&shop, "feat/flaky", true);
    scratch.respond(found(&[Some(node(json!({})))]));
    let plugin = scratch.start("200ms");
    plugin.rows_where("ready", |rows| {
        rows.len() == 1 && stage(&rows[0]) == "Ready to merge"
    });

    scratch.respond(found(&[Some(node(
        json!({"reviewDecision": "REVIEW_REQUIRED"}),
    ))]));

    plugin.rows_where("implementing again", |rows| {
        stage(&rows[0]) == "Implementing"
    });
}
