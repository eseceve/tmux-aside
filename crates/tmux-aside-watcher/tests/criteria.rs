mod support;

use serde_json::{Value, json};
use support::{Scratch, Tmux, found, launch, node};
use tmux_aside::launch::Launch;
use tmux_aside::protocol::Tone;

fn fixture(name: &str) -> String {
    format!(
        "{}/tests/fixtures/criteria/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn ready_to_merge(scratch: &Scratch, command: &str, args: &[&str], timeout: &str) {
    scratch.configure(&format!(
        "criteria:\n  ready_to_merge:\n    command: {}\n    args: {}\n    timeout: {timeout}\n",
        fixture(command),
        serde_json::to_string(args).unwrap()
    ));
}

#[test]
fn a_configured_criterion_replaces_the_default_and_gets_the_pull_request() {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    scratch.record(&shop, "feat/x", true);
    scratch.respond(found(&[Some(node(json!({})))]));
    let question = scratch.0.join("question.json");
    let answer = json!({"met": false, "summary": "needs the web team", "tone": "warning"});
    ready_to_merge(
        &scratch,
        "answer",
        &[question.to_str().unwrap(), &answer.to_string()],
        "5s",
    );

    let plugin = scratch.start("30s");

    let rows = plugin.rows_where("the criterion's summary", |rows| {
        rows.first()
            .is_some_and(|row| row.lines == ["Implementing · needs the web team"])
    });
    assert_eq!(rows[0].bar_tone, Tone::Warning);
    let asked: Value = serde_json::from_str(&std::fs::read_to_string(question).unwrap()).unwrap();
    assert_eq!(asked["stage"], "ready_to_merge");
    assert_eq!(asked["url"], "https://github.com/org/shop/pull/12");
    assert_eq!(asked["pull_request"]["number"], 12);
    assert_eq!(asked["pull_request"]["head"], "aaa");
}

#[test]
fn a_criterions_nudge_reaches_the_agent() {
    let scratch = Scratch::new();
    let tmux = Tmux::start();
    let pane = tmux.agent_pane(&scratch.0.join("typed"), &scratch.0);
    tmux.report(&scratch, &pane, "done");
    let shop = scratch.repository("shop");
    scratch.save(&Launch {
        pane,
        ..launch(&shop, "feat/x")
    });
    scratch.respond(found(&[Some(node(json!({})))]));
    let answer = json!({"met": false, "nudge": "Ask the web team for a review."});
    ready_to_merge(
        &scratch,
        "answer",
        &[
            scratch.0.join("question.json").to_str().unwrap(),
            &answer.to_string(),
        ],
        "5s",
    );

    let _plugin = scratch.start_in("200ms", Some(&tmux));

    support::wait_until("the nudge", || {
        std::fs::read_to_string(scratch.0.join("typed"))
            .is_ok_and(|typed| typed.contains("Ask the web team for a review."))
    });
}

/// Two launches, `api` with pull request 13 and `shop` with pull request 12, whose Ready to
/// merge Criterion misbehaves in `mode` for pull request 13 only.
fn misbehaving(mode: &str, timeout: &str) -> Scratch {
    let scratch = Scratch::new();
    let shop = scratch.repository("shop");
    let api = scratch.repository("api");
    scratch.record(&api, "feat/bad", true);
    scratch.record(&shop, "feat/fine", true);
    scratch.respond(found(&[
        Some(node(
            json!({"number": 13, "url": "https://github.com/org/api/pull/13"}),
        )),
        Some(node(json!({}))),
    ]));
    ready_to_merge(&scratch, "bad-on-13", &[mode], timeout);
    scratch
}

#[test]
fn a_failing_hanging_or_invalid_criterion_shows_its_error_on_its_row_only() {
    for (mode, error) in [
        ("fail", "criterion failed: exited with status 3"),
        ("invalid", "criterion failed: invalid answer: not json"),
        ("hang", "criterion failed: timed out after 0.5s"),
    ] {
        let scratch = misbehaving(mode, "500ms");

        let plugin = scratch.start("30s");

        let rows = plugin.rows_where(error, |rows| {
            rows.len() == 2 && rows[0].lines[0].contains("criterion failed")
        });
        assert_eq!(rows[0].lines, [format!("Implementing · {error}")], "{mode}");
        assert_eq!(rows[0].bar_tone, Tone::Danger);
        assert_eq!(
            rows[1].lines,
            ["Ready to merge · waiting to be merged"],
            "{mode}"
        );
    }
}

#[test]
fn rows_keep_updating_while_a_criterion_hangs() {
    let scratch = misbehaving("hang", "4s");
    let plugin = scratch.start("30s");
    plugin.rows_where("both launches", |rows| rows.len() == 2);
    let started = std::time::Instant::now();

    scratch.record(&scratch.repository("web"), "feat/new", true);

    plugin.rows_where("the new launch", |rows| rows.len() == 3);
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}

#[test]
fn an_invalid_criteria_config_shows_on_the_rows() {
    let scratch = Scratch::new();
    scratch.record(&scratch.repository("shop"), "feat/x", true);
    scratch.respond(found(&[Some(node(json!({})))]));
    scratch.configure("criteria:\n  merged:\n    command: /bin/true\n");

    let plugin = scratch.start("30s");

    let rows = plugin.rows_where("the config error", |rows| {
        rows.first()
            .is_some_and(|row| row.lines[0].contains("invalid config"))
    });
    assert_eq!(rows[0].bar_tone, Tone::Danger);
}

#[test]
fn a_packs_criterion_replaces_the_default() {
    let scratch = Scratch::new();
    scratch.record(&scratch.repository("shop"), "feat/x", true);
    scratch.respond(found(&[Some(node(json!({})))]));
    let pack = scratch.0.join("team-pack");
    std::fs::create_dir_all(pack.join("bin")).unwrap();
    std::fs::copy(fixture("answer"), pack.join("bin/answer")).unwrap();
    let answer = json!({"met": false, "summary": "needs the team's review"});
    std::fs::write(
        pack.join("pack.yaml"),
        format!(
            "criteria:\n  ready_to_merge:\n    command: bin/answer\n    args: [{}, {}]\n",
            serde_json::to_string(scratch.0.join("question.json").to_str().unwrap()).unwrap(),
            serde_json::to_string(&answer.to_string()).unwrap(),
        ),
    )
    .unwrap();
    scratch.configure(&format!("pack: \"{}\"\n", pack.display()));

    let plugin = scratch.start("30s");

    plugin.rows_where("the pack's summary", |rows| {
        rows.first()
            .is_some_and(|row| row.lines == ["Implementing · needs the team's review"])
    });
}

#[test]
fn a_criterion_is_told_what_the_agent_is_doing() {
    let scratch = Scratch::new();
    let tmux = Tmux::start();
    let pane = tmux.agent_pane(&scratch.0.join("typed"), &scratch.0);
    tmux.report(&scratch, &pane, "working");
    let shop = scratch.repository("shop");
    scratch.save(&Launch {
        pane: pane.clone(),
        ..launch(&shop, "feat/x")
    });
    scratch.respond(found(&[Some(node(json!({})))]));
    let question = scratch.0.join("question.json");
    ready_to_merge(
        &scratch,
        "answer",
        &[question.to_str().unwrap(), r#"{"met": false}"#],
        "5s",
    );

    let _plugin = scratch.start_in("200ms", Some(&tmux));
    support::wait_until("a question", || question.exists());
    let asked =
        || -> Value { serde_json::from_str(&std::fs::read_to_string(&question).unwrap()).unwrap() };
    assert_eq!(asked()["agent"], "busy");

    tmux.report(&scratch, &pane, "done");
    support::wait_until("the agent to be idle", || asked()["agent"] == "idle");
}

#[test]
fn without_a_live_agent_the_question_has_no_agent() {
    let scratch = Scratch::new();
    scratch.record(&scratch.repository("shop"), "feat/x", true);
    scratch.respond(found(&[Some(node(json!({})))]));
    let question = scratch.0.join("question.json");
    ready_to_merge(
        &scratch,
        "answer",
        &[question.to_str().unwrap(), r#"{"met": false}"#],
        "5s",
    );

    let _plugin = scratch.start("30s");
    support::wait_until("a question", || question.exists());

    let asked: Value = serde_json::from_str(&std::fs::read_to_string(&question).unwrap()).unwrap();
    assert!(asked.get("agent").is_none(), "{asked}");
}
