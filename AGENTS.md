# AGENTS.md

tmux-aside is a tmux sidebar built from plugin sections, written in Rust as a Cargo workspace. These instructions apply to people and coding agents alike.

## Commands

You need Rust 1.88 or newer and tmux 3.0 or newer.

```sh
cargo build
cargo test --workspace
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

CI runs `cargo fmt --all --check`, the same clippy command and the tests, with `--locked`, on macOS and Linux. Run all four before you commit.

- Integration tests start their own tmux server on a private socket (`tmux -L <name>`), so they never touch your tmux sessions. Some of them wait up to 5 seconds for a plugin snapshot and can time out on a busy machine. Rerun a failing test on its own before you debug it.
- Run one crate's tests with `cargo test -p tmux-aside-agents`, or one test file with `--test setup`.
- To see the sidebar with fake data in every state, without touching your tmux, run `examples/demo/run.sh target/debug/tmux-aside` and attach with the command it prints.

## Layout

| Crate | |
|---|---|
| `crates/tmux-aside` | The command, the daemon (shared view, plugin supervision) and the sidebar client (layout and rendering). |
| `crates/tmux-aside-*` | One crate per bundled plugin. Logic that can be tested lives in `src/lib.rs`; `src/main.rs` talks to the outside world. |

The plugin protocol is described in [docs/plugins.md](docs/plugins.md). How the sidebar is used and configured is described in the [README](README.md).

## Rules that are easy to break

- **A plugin's stdout is the protocol.** Plugins print one JSON message per line. Debug output on stdout corrupts the section, so print it to stderr; tmux-aside logs plugin stderr to `~/.local/state/tmux-aside/`.
- **Hook commands are silent and never fail.** `tmux-aside-agents set-status` and `register` run from AI agents' hooks. They must exit 0, write nothing to stdout and do nothing when `$TMUX_PANE` is unset, because some agents read hook stdout as a decision.
- **Tests never touch the real agent configs.** `setup` tests point the agent's config at a temporary directory with `CLAUDE_CONFIG_DIR`, `CODEX_HOME` or `HOME`.
- **The protocol is versioned.** New fields must be optional so older plugins keep working.

## Conventions

- Tests describe behavior in their names (`a_flat_list_puts_waiting_agents_first_across_projects`) and live in each crate's `tests/` directory.
- Public items have doc comments; otherwise prefer clear names over comments.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org) (`feat(agents): ...`, `fix: ...`).
