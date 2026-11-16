---
name: tmux-aside-qa
description: Launches the QA of launched pull requests with tmux-aside, one fresh QA agent for pull requests that are tested together, and launches fixes when QA failed. Use when the user asks to QA, test in staging or verify merged pull requests ("hacé el QA de estos PRs", "run QA for the theme group", "repeat QA of #12"), or to fix what a failed QA found.
---

# tmux-aside-qa

Turn the request into `tmux-aside` commands. Hold no logic of your own: the QA agent tests and records its verdict, and the watcher follows the pull requests.

## Commands

- `tmux-aside status --json`: every launch with its pull request and group. Use it to turn "the theme group" or "#12" into pull request URLs.
- `tmux-aside qa <url>...`: one QA agent, with a fresh context and the `qa` template, for the given pull requests. Each must be followed by a launch.
- `tmux-aside fix <url>`: an agent that fixes what the QA of that pull request found, on a new branch in the same group. QA reruns for both once the fix reaches QA.
- `tmux-aside remove <QA launch id>`: once QA finished, close the QA agent's window and forget its launch; the clone it worked in is kept. Its id starts with the repository and `qa-` in `tmux-aside status --json`.

## Agent settings

When the user names an agent, model or reasoning effort, pass it explicitly as `--agent <claude|codex|agy>`, `--model <name>` or `--effort <value>` on `launch`, `qa` or `fix`. Omit settings the user did not choose: flags override the template, which overrides the config. Model names belong to the selected agent; do not invent or translate them. `resurrect` keeps the settings saved with each launch. `status --json` includes `model` and `effort` when specified.

## Choosing

1. **Pull requests**: the ones the user names. Pull requests of one group are tested together, so pass all of a group's pull requests to one `tmux-aside qa`, unless the user asks otherwise.
2. **Repeating QA**: run `tmux-aside qa` again; nothing prevents it.
3. **A failed QA**: run `tmux-aside fix <url>` for the pull request whose QA failed, when the user asks for a fix.

Tell the user what was launched and in which window, from the command's output. If a command fails, report its message and do not retry with guessed values.
