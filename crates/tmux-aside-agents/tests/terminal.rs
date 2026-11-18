use tmux_aside_agents::processes::{from_codex, pane_process};
use tmux_aside_agents::terminal::{OscParser, TerminalStatus as State};

#[test]
fn split_sequences_and_multiple_records_preserve_permission_question_and_completion() {
    let wire = b"text\x1b]7501;state=working:app=codex\x1b\\\x1b]7501;state=blocked:app=codex:kind=permission\x07\x1b]7501;state=blocked:app=codex:kind=question\x1b\\\x1b]7501;state=done:app=codex\x1b\\";
    for split in 0..=wire.len() {
        let mut parser = OscParser::default();
        let mut states = parser.feed(&wire[..split]);
        states.extend(parser.feed(&wire[split..]));
        assert_eq!(
            states,
            [State::Working, State::Waiting, State::Waiting, State::Done]
        );
    }
}

#[test]
fn malformed_foreign_and_oversized_records_cannot_become_idle_or_done() {
    let mut parser = OscParser::default();
    assert!(parser.feed(b"\x1b]7501;state=done:app=other\x07\x1b]7501;state=done\x07\x1b]7501;state=done:state=idle:app=codex\x07").is_empty());
    let mut long = b"\x1b]7501;".to_vec();
    long.extend(vec![b'x'; 10_000]);
    long.extend(b"\x07\x1b]7501;state=clear\x1b\\");
    assert_eq!(parser.feed(&long), [State::Unknown]);
    assert_eq!(
        parser.feed(b"\x1b]7501;state=error:app=codex\x07"),
        [State::Error]
    );
}

#[test]
fn process_identity_distinguishes_restarts_and_ignores_sidebar_plugins() {
    let table = " 10 1 /bin/zsh\n 20 10 /opt/bin/codex\n 30 1 /bin/tmux-aside\n 40 30 /bin/codex\n 50 1 /bin/tmux-aside\n 60 50 /bin/tmux-aside-agents\n";
    assert_eq!(pane_process(10, "codex", table), ("codex".into(), Some(20)));
    assert_eq!(
        pane_process(30, "tmux-aside", table),
        ("codex".into(), Some(40))
    );
    assert_eq!(
        pane_process(50, "tmux-aside", table),
        ("tmux-aside".into(), None)
    );
    assert_eq!(pane_process(20, "codex", table), ("codex".into(), Some(20)));
    assert_eq!(
        pane_process(10, "codex", "21 10 codex"),
        ("codex".into(), Some(21))
    );
}

#[test]
fn an_agent_launched_by_any_wrapper_is_found_behind_it() {
    let table = " 70 1 /opt/bin/terminal-orchestrator\n 71 70 codex\n 80 1 /bin/zsh\n 81 80 /bin/tmux-aside-agents\n";
    assert_eq!(
        pane_process(70, "terminal-orches", table),
        ("codex".into(), Some(71))
    );
    assert_eq!(pane_process(80, "zsh", table), ("zsh".into(), None));
}

#[test]
fn a_codex_hook_is_identified_by_ancestry_even_when_its_pane_now_runs_claude() {
    let table = "100 1 codex\n101 100 /bin/sh\n102 101 tmux-aside-agents\n200 1 claude\n201 200 sh\n202 201 tmux-aside-agents";
    assert!(from_codex(102, table));
    assert!(!from_codex(202, table));
    assert!(!from_codex(42, table));
}
