# Writing a plugin

A plugin is any executable that prints JSON lines on stdout. tmux-aside runs it, reads what it prints and draws it as a section. Write it in the language you like; the bundled plugins are Rust crates in [`crates/`](../crates), and their `src/lib.rs` files are good references.

## Where tmux-aside finds it

In the config, `plugin: weather` runs `tmux-aside-weather`, looked up next to the `tmux-aside` binary and then in `PATH`. A value with a `/` is used as a path:

```yaml
sections:
  - plugin: /usr/local/bin/my-section    # a path; ~ is not expanded
    args: ["--city", "Lisbon"]
    interval: 5m
```

`args` are passed as command-line arguments. stderr goes to `$XDG_STATE_HOME/tmux-aside/<index>-<name>.log`.

## The protocol

Messages are JSON objects, one per line, with a `type`. The current protocol version is `1`.

1. tmux-aside writes a `hello` line to the plugin's stdin: `{"type":"hello","protocol":1,"features":[]}`.
2. The plugin answers with its own `hello`, then one or more `snapshot` lines:

```json
{"type":"hello","protocol":1,"features":[]}
{"type":"snapshot","title":"Weather","rows":[{"text":"Lisbon","detail":"21°C"}]}
```

Each snapshot replaces the whole section. Unknown message types and fields are ignored in both directions, so later versions can add fields without breaking older plugins.

### Modes

- `mode: interval` (the default): the plugin is started every `interval`, prints its hello and one snapshot, and exits. It must finish within `timeout`.
- `mode: stream`: the plugin is started once and kept running. It prints a new snapshot whenever its content changes, and must exit when its stdin closes. If it exits, it is restarted with a backoff.

A plugin that fails, times out or prints invalid lines shows its error in the section, and is retried with a backoff.

## Rows

Only `text` is required.

| Field | Type | |
|---|---|---|
| `text` | string | Left side of the row. |
| `detail` | string | Right side. `"spinner"` draws an animated spinner. |
| `tone`, `detail_tone` | tone | Color of the text and the detail. |
| `icon`, `icon_tone` | string, tone | One-column glyph before the text. `"spinner"` is animated. |
| `bar_tone` | tone | Color of a bar along every line of the row. `plain` (the default) draws none. |
| `group` | string | Consecutive rows with the same group are shown under one foldable group line. |
| `lines` | strings | Extra muted lines below the text. |
| `meter`, `meter_tone` | 0-100, tone | A progress bar drawn between the text and the detail. |
| `window` | string | tmux window id (`@3`) the row belongs to. The sidebar in that window highlights it, and moving into that window selects it. |
| `align` | `left` or `center` | Horizontal placement of the text. |
| `actions` | actions | Commands run from the sidebar, see below. |

Tones are `plain`, `muted`, `accent`, `success`, `warning` and `danger`; the user's theme maps them to colors. Use them for the meaning described in the [README](../README.md#tmux-aside): accent for work in progress, warning for what needs the user, danger for what is broken, success for what is done.

### Actions

```json
{"key":"enter","command":["open","https://example.com"],"label":"open the forecast","cwd":"/tmp"}
```

- `key` is `enter` or a single character. The sidebar keeps `j`, `k`, `h`, `l`, `o`, `?` and space for itself, so actions bound to them never run.
- `command` is run without a shell, from the sidebar's pane, in `cwd` when set. Use `["sh", "-c", "...", "name", "arg"]` when you need one.
- `label` is shown in the `?` help.

## A complete example

```sh
#!/bin/sh
# tmux-aside-load: the load average, as a section
read -r _hello
load=$(uptime | sed 's/.*load averages*: //' | cut -d' ' -f1 | tr -d ',')
printf '%s\n' '{"type":"hello","protocol":1,"features":[]}'
printf '{"type":"snapshot","title":"Load","rows":[{"text":"1 min","detail":"%s"}]}\n' "$load"
```

Make it executable, put it in `PATH` and add `- plugin: load` to the config.
