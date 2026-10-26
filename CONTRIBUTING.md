# Contributing

Thanks for helping! Bug reports, ideas and pull requests are welcome. For a larger change, open an issue first so we can agree on the approach.

## Setup

You need Rust 1.88 or newer and tmux 3.0 or newer; some integration tests start their own tmux server.

```sh
cargo build
cargo test --workspace
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

CI runs the same checks on macOS and Linux.

## Trying a change

Run the binaries from `target/debug`, which also finds the plugins built next to it:

```sh
cargo build && target/debug/tmux-aside on
```

- After changing a plugin or the config: `target/debug/tmux-aside reload`.
- After changing the daemon or how the sidebar draws: `target/debug/tmux-aside off && target/debug/tmux-aside on`.
- Plugin errors are logged to `~/.local/state/tmux-aside/`.

## Layout

| Crate | |
|---|---|
| `tmux-aside` | The command, the daemon (shared view, plugin supervision) and the sidebar client (layout and rendering). |
| `tmux-aside-*` | One crate per bundled plugin. Logic that can be tested lives in `src/lib.rs`; `src/main.rs` talks to the outside world. |

## Conventions

- Tests describe behavior in their names (`a_flat_list_puts_waiting_agents_first_across_projects`) and live in each crate's `tests/` directory.
- Public items have doc comments; otherwise prefer clear names over comments.
- The plugin protocol is versioned. New fields must be optional so older plugins keep working; see [docs/plugins.md](docs/plugins.md).
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org) (`feat(agents): ...`, `fix: ...`).

## License

By contributing, you agree that your contributions are dual licensed under MIT OR Apache-2.0, as described in the [README](README.md#license).
