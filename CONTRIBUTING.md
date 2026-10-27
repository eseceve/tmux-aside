# Contributing

Thanks for helping! Bug reports, ideas and pull requests are welcome. For a larger change, open an issue first so we can agree on the approach.

## Setup

The commands, the crate layout and the conventions are in [AGENTS.md](AGENTS.md), which coding agents also read. You need Rust 1.88 or newer and tmux 3.0 or newer.

## Trying a change

Run the binaries from `target/debug`, which also finds the plugins built next to it:

```sh
cargo build && target/debug/tmux-aside on
```

- After changing a plugin or the config: `target/debug/tmux-aside reload`.
- After changing the daemon or how the sidebar draws: `target/debug/tmux-aside off && target/debug/tmux-aside on`.
- Plugin errors are logged to `~/.local/state/tmux-aside/`.

## License

By contributing, you agree that your contributions are dual licensed under MIT OR Apache-2.0, as described in the [README](README.md#license).
