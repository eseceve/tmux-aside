use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use tmux_aside::protocol::{PluginMessage, Tone};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

struct FakeGh(PathBuf);

impl FakeGh {
    fn new(script_body: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tmux-aside-github-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("gh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/args\"\n{script_body}\n"
            ),
        )
        .unwrap();
        Command::new("chmod")
            .arg("+x")
            .arg(&script)
            .status()
            .unwrap();
        FakeGh(dir)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tmux-aside-github"))
            .args(args)
            .env("TMUX_ASIDE_LANG", "en")
            .env(
                "PATH",
                format!("{}:{}", self.0.display(), std::env::var("PATH").unwrap()),
            )
            .output()
            .unwrap()
    }

    fn args(&self) -> String {
        std::fs::read_to_string(self.0.join("args")).unwrap()
    }
}

impl Drop for FakeGh {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn messages(output: &Output) -> Vec<PluginMessage> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

const ONE_PR: &str = r#"{"data":{"search":{"nodes":[{"number":345,"title":"Count writes","url":"https://github.com/org/shop/pull/345","isDraft":false,"reviewDecision":"APPROVED","createdAt":"2026-10-18T10:00:00Z","author":{"login":"octocat"},"updatedAt":"2026-10-20T10:00:00Z","repository":{"name":"shop","nameWithOwner":"org/shop"},"commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"SUCCESS"}}}]}}]}}}"#;

#[test]
fn prints_a_hello_and_a_snapshot_of_the_search() {
    let gh = FakeGh::new(&format!("cat <<'JSON'\n{ONE_PR}\nJSON"));

    let output = gh.run(&["--title", "Mine"]);

    assert!(output.status.success());
    let messages = messages(&output);
    assert!(matches!(
        messages[0],
        PluginMessage::Hello { protocol: 1, .. }
    ));
    let PluginMessage::Snapshot { title, rows, .. } = &messages[1] else {
        panic!("expected a snapshot");
    };
    assert_eq!(title.as_deref(), Some("Mine"));
    assert_eq!(rows[0].text, "Count writes");
    assert_eq!(rows[0].detail.as_deref(), Some("#345"));
}

#[test]
fn sends_the_configured_search_to_github() {
    let gh = FakeGh::new(&format!("cat <<'JSON'\n{ONE_PR}\nJSON"));

    gh.run(&["--query", "is:pr is:open review-requested:@me"]);

    let args = gh.args();
    assert!(args.starts_with("api\ngraphql\n"), "{args}");
    assert!(
        args.contains("q=is:pr is:open review-requested:@me"),
        "{args}"
    );
}

#[test]
fn an_empty_search_sends_no_rows() {
    let gh = FakeGh::new(r#"echo '{"data":{"search":{"nodes":[]}}}'"#);

    let messages = messages(&gh.run(&[]));

    let PluginMessage::Snapshot { rows, .. } = &messages[1] else {
        panic!("expected a snapshot");
    };
    assert!(rows.is_empty());
}

#[test]
fn byline_adds_the_author_on_a_second_line() {
    let gh = FakeGh::new(&format!("cat <<'JSON'\n{ONE_PR}\nJSON"));

    let messages = messages(&gh.run(&["--byline"]));

    let PluginMessage::Snapshot { rows, .. } = &messages[1] else {
        panic!("expected a snapshot");
    };
    assert!(
        rows[0].lines[0].starts_with("@octocat · "),
        "{:?}",
        rows[0].lines
    );
}

#[test]
fn a_failing_gh_exits_with_its_error_on_stderr() {
    let gh = FakeGh::new("echo 'HTTP 401: Bad credentials' >&2\nexit 1");

    let output = gh.run(&[]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Bad credentials"));
}

#[test]
fn state_color_picks_the_parts_colored_by_the_state() {
    let gh = FakeGh::new(&format!("cat <<'JSON'\n{ONE_PR}\nJSON"));

    let messages = messages(&gh.run(&["--state-color", "number"]));

    let PluginMessage::Snapshot { rows, .. } = &messages[1] else {
        panic!("expected a snapshot");
    };
    assert_eq!(rows[0].icon, None);
    assert_eq!(rows[0].detail_tone, Tone::Success);
}
