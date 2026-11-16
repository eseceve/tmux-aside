mod support;

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::{Plugin, Scratch, Tmux, found, launch, node};

/// An agent pane on a private tmux server, followed by a Watcher that answers with `response`.
struct Setup {
    scratch: Scratch,
    tmux: Tmux,
    pane: String,
}

impl Setup {
    fn new(status: &str, response: Value) -> Self {
        let scratch = Scratch::new();
        let tmux = Tmux::start();
        let pane = tmux.agent_pane(&scratch.0.join("typed"), &scratch.0);
        tmux.report(&scratch, &pane, status);
        let shop = scratch.repository("shop");
        scratch.save(&tmux_aside::launch::Launch {
            pane: pane.clone(),
            ..launch(&shop, "feat/x")
        });
        scratch.respond(found(&[Some(response)]));
        Setup {
            scratch,
            tmux,
            pane,
        }
    }

    fn start(&self) -> Plugin {
        self.scratch.start_in("200ms", Some(&self.tmux))
    }

    fn typed(&self) -> Vec<String> {
        std::fs::read_to_string(self.scratch.0.join("typed"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn wait_for_typed(&self, count: usize) -> Vec<String> {
        wait_until(&format!("{count} typed lines"), || {
            self.typed().len() >= count
        });
        self.typed()
    }

    /// Waits until the Watcher fetched `more` times from now.
    fn wait_for_fetches(&self, more: usize) {
        let target = self.scratch.calls().len() + more;
        wait_until(&format!("{more} more fetches"), || {
            self.scratch.calls().len() >= target
        });
    }
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn conflicting(head: &str) -> Value {
    node(json!({"mergeable": "CONFLICTING", "headRefOid": head}))
}

fn recorded_nudges(scratch: &Scratch) -> Vec<String> {
    scratch.recorded("shop__feat-x").nudges
}

#[test]
fn a_conflict_is_typed_into_an_idle_agent() {
    let setup = Setup::new("done", conflicting("aaa"));

    let _plugin = setup.start();

    let typed = setup.wait_for_typed(1);
    assert_eq!(typed.len(), 1);
    assert!(typed[0].contains("conflicts"), "{typed:?}");
    assert!(
        typed[0].contains("https://github.com/org/shop/pull/12"),
        "{typed:?}"
    );
}

#[test]
fn unresolved_review_comments_are_typed_into_an_idle_agent() {
    let setup = Setup::new(
        "done",
        node(json!({"reviewThreads": {"nodes": [{"isResolved": false}]}})),
    );

    let _plugin = setup.start();

    let typed = setup.wait_for_typed(1);
    assert!(
        typed[0].contains("1 unresolved review comments"),
        "{typed:?}"
    );
}

#[test]
fn nothing_is_typed_while_the_agent_works_or_waits_for_the_user() {
    let setup = Setup::new("working", conflicting("aaa"));
    let _plugin = setup.start();
    setup.wait_for_fetches(3);
    assert!(setup.typed().is_empty(), "{:?}", setup.typed());

    setup.tmux.report(&setup.scratch, &setup.pane, "waiting");
    setup.wait_for_fetches(3);
    assert!(setup.typed().is_empty(), "{:?}", setup.typed());

    setup.tmux.report(&setup.scratch, &setup.pane, "done");
    setup.wait_for_typed(1);
}

#[test]
fn the_same_nudge_is_typed_once_per_head_commit() {
    let setup = Setup::new("done", conflicting("aaa"));
    let _plugin = setup.start();
    setup.wait_for_typed(1);
    setup.wait_for_fetches(3);
    assert_eq!(setup.typed().len(), 1);

    setup.scratch.respond(found(&[Some(conflicting("bbb"))]));

    assert_eq!(setup.wait_for_typed(2).len(), 2);
    let heads: Vec<String> = recorded_nudges(&setup.scratch)
        .iter()
        .filter_map(|key| key.rsplit_once('@').map(|(_, head)| head.to_string()))
        .collect();
    assert_eq!(heads, ["aaa", "bbb"]);
}

#[test]
fn nudges_already_sent_survive_a_watcher_restart() {
    let setup = Setup::new("done", conflicting("aaa"));
    let plugin = setup.start();
    setup.wait_for_typed(1);
    drop(plugin);

    let _plugin = setup.start();
    setup.wait_for_fetches(3);

    assert_eq!(setup.typed().len(), 1);
}

#[test]
fn a_different_problem_on_the_same_head_is_delivered() {
    let setup = Setup::new("done", conflicting("aaa"));
    let _plugin = setup.start();
    setup.wait_for_typed(1);
    setup.scratch.respond(found(&[Some(node(json!({
        "headRefOid": "aaa", "mergeable": "MERGEABLE",
        "reviewThreads": {"nodes": [{"isResolved": false}]}
    })))]));
    let typed = setup.wait_for_typed(2);
    assert!(typed[1].contains("unresolved review comments"));
}

#[test]
fn a_review_pane_never_receives_implementation_nudges() {
    let setup = Setup::new("done", conflicting("aaa"));
    let mut review = setup.scratch.recorded("shop__feat-x");
    review.id = "shop__review".into();
    review.opens_pull_request = false;
    review.template = Some("review".into());
    setup.scratch.save(&review);
    let _plugin = setup.start();
    setup.wait_for_fetches(3);
    assert!(setup.typed().is_empty());
    assert!(recorded_nudges(&setup.scratch).is_empty());
}
