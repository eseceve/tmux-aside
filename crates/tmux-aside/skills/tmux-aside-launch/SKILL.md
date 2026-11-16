---
name: tmux-aside-launch
description: Launches AI agents on tasks with tmux-aside, each in its own branch, worktree and tmux window, and reports where launched work stands. Use when the user asks to launch, start, kick off or parallelize work on one or more tasks, issues, tickets or pull requests ("lanzá estas tareas", "start an agent on issue 12 in the api repo", "take over this PR"), to put launches in a group, asks what launched agents and pull requests are doing, wants their agents back after tmux or the computer restarted ("resucitá los agentes", "reabrí las ventanas de las tareas"), or wants launches and their worktrees cleaned up ("borrá el worktree de este PR", "limpiá los worktrees mezclados").
---

# tmux-aside-launch

Turn the request into `tmux-aside` commands. Hold no logic of your own: tmux-aside creates the branches, worktrees and windows, and the watcher follows the pull requests.

## Commands

- `tmux-aside status --json`: every launch with its repository, branch, worktree, window, AI (`ai`), template, task reference, group and pull request. Read it first when the user asks how things stand, or to avoid launching twice.
- `tmux-aside launch <repo> --template <name> --task <reference> --branch <branch> [--group <name>]`: a new branch from the remote's default branch.
- `tmux-aside launch <repo> --template adopt --pr <url> [--group <name>]`: take over an existing pull request.
- `tmux-aside remove <id | branch | pull request URL | worktree path> [--force] [--keep-branch]`: remove a launch's worktree, local branch, record and window, when the user is done with it, such as after its pull request merged or its QA finished. It refuses to lose uncommitted or unpushed work; never add `--force` unless the user asks for it after seeing what would be lost.
- `tmux-aside prune [<path>...] --dry-run`, then without `--dry-run`: remove the worktrees no launch follows whose pull request was merged or closed. Show the dry-run list to the user before running it.
- `tmux-aside resurrect [--dry-run]`: reopen the agent of every launch whose window is gone, resuming its conversation. Run it with `--dry-run` first and show the list.

## Agent settings

When the user names an agent, model or reasoning effort, pass it explicitly as `--agent <claude|codex|agy>`, `--model <name>` or `--effort <value>` on `launch`, `qa` or `fix`. Omit settings the user did not choose: flags override the template, which overrides the config. Model names belong to the selected agent; do not invent or translate them. `resurrect` keeps the settings saved with each launch. `status --json` includes `model` and `effort` when specified.

## Choosing

1. **Repositories**: `<repo>` is a path to a local clone. Use the ones the user names; when a task spans several repositories, launch it once per repository with the same `--group`. If you cannot tell where a repository is cloned, ask.
2. **Template**: the user's own templates are in `$XDG_CONFIG_HOME/tmux-aside/templates/` (`~/.config/tmux-aside/templates/` by default). A Pack's are in the `templates` folder of the Pack named by the `pack` key of the config file next to it (`config.yaml`), or in the folder its `pack.yaml` names under `templates:`. The bundled ones are `implement` (a pull request for a task), `adopt` (with `--pr`), `qa` and `fix`. A template with the same name in an earlier place wins. Use `implement` unless the request or a template's name fits better; list the folders when unsure.
3. **Task reference**: pass what the user gave, as written: a URL, an issue, a sentence. The template knows what to do with it.
4. **Branch**: short and descriptive, following the repository's conventions (`feat/dark-theme`, `fix/login-redirect`); look at `git branch -r` when unsure.
5. **Group**: use one when launches belong together, such as one task across repositories or pull requests that must be tested together, so that their QA runs once for all of them.

Launch each one, then tell the user what was launched and in which window, from the command's output. If a command fails, report its message and do not retry with guessed values.
