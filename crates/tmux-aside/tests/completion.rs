use std::process::Command;

fn completion(shell: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_tmux-aside"))
        .args(["completion", shell])
        .env("TMUX_ASIDE_LANG", "en")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn every_supported_shell_gets_commands_and_flags() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let script = completion(shell);
        for word in [
            "tmux-aside",
            "launch",
            "resurrect",
            "template",
            "agent",
            "model",
            "effort",
        ] {
            assert!(script.contains(word), "{shell} completion lacks {word}");
        }
    }
}

#[test]
fn bash_and_zsh_completion_scripts_have_valid_shell_syntax() {
    for shell in ["bash", "zsh"] {
        if Command::new(shell).arg("--version").output().is_err() {
            continue;
        }
        let output = Command::new(shell)
            .args(["-n", "-c", &completion(shell)])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{shell}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn unsupported_shells_fail_without_printing_a_script() {
    let output = Command::new(env!("CARGO_BIN_EXE_tmux-aside"))
        .args(["completion", "not-a-shell"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn bash_zsh_and_fish_complete_agent_and_effort_values() {
    for shell in ["bash", "zsh", "fish"] {
        let script = completion(shell);
        for value in ["codex", "claude", "agy", "minimal", "high", "xhigh", "max"] {
            assert!(script.contains(value), "{shell} completion lacks {value}");
        }
    }
}
