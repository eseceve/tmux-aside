# Writing a Criterion

A Criterion decides whether a pull request reached a stage of the Lifecycle, and which Nudge, if any, would move it closer. The [watcher](../README.md#watcher) asks the Criteria of the stages after Implementing in order and stops at the first one that is not met. Each pull request's stage is evaluated again on every fetch, so a pull request falls back when a Criterion it met stops holding.

tmux-aside ships a default Criterion for each stage, which use only what GitHub tells. A [Pack](../README.md#packs) replaces them with its team's, under `criteria:` in its `pack.yaml`, where a `command` is a path relative to the Pack's folder, and the config replaces either:

```yaml
criteria:
  ready_to_merge:
    command: /path/to/ready-to-merge   # an executable; ~ is not expanded
    args: ["--team", "web"]            # passed as command-line arguments
    timeout: 10s                       # how long it may run (the default)
```

The stages are `ready_to_merge`, `qa`, `ready_to_deploy` and `deployed`. Without `command`, the `args` go to the stage's default Criterion. The defaults are `tmux-aside-watcher criterion <stage>` (with dashes: `ready-to-merge`), which go through the same contract:

| Stage | Met when |
|---|---|
| `ready_to_merge` | The pull request is approved, its checks pass and it merges cleanly. A merged pull request is always Ready to merge, whatever its Criterion answers. Asks for a Nudge on conflicts, then on unresolved review comments. |
| `qa` | The pull request is merged. |
| `ready_to_deploy` | It carries the `--passed-label` (`qa:passed`); the `--failed-label` (`qa:failed`) shows that QA failed. |
| `deployed` | Always: without a Criterion that knows the deploy pipeline, a change counts as deployed as soon as it is Ready to deploy. |

A pull request whose Deployed Criterion is met leaves the watcher, and so does a closed one, once its worktree is removed. Before it leaves, a deployed pull request runs the [`on_deployed` action](../README.md#watcher), when the config or the Pack sets one.

## The contract

The watcher runs the command with its arguments, writes one JSON object to its stdin and closes it:

```json
{"stage": "ready_to_merge", "url": "https://github.com/org/app/pull/12", "agent": "idle", "pull_request": {"number": 12, "head": "4f2c...", "draft": false, "review": "APPROVED", "checks": "SUCCESS", "conflicting": false, "unresolved_threads": 0, "labels": ["merge:ready"], "...": "..."}}
```

`pull_request` holds what the watcher already fetched, so most Criteria need no GitHub call of their own. `agent` says what the launch's agent is doing: `idle` once its turn is over, `busy` while it works or waits for the user; it is absent when the launch has no live agent. A Criterion that acts for the agent, such as asking reviewers to look again once a round of fixes is done, waits for `idle`. The Criterion prints one JSON object on stdout and exits with status 0:

| Field | Type | |
|---|---|---|
| `met` | bool | Whether the pull request reached the stage. Required. |
| `summary` | string | What is missing to reach the stage, or where the pull request stands once it is met. Shown on the row as written. |
| `tone` | tone | The row's color: `plain`, `muted`, `accent`, `success`, `warning` or `danger`. `accent` until met and `success` once met when unset. |
| `nudge` | string | A command for the agent, such as asking it to resolve conflicts. The watcher types it into the agent while the agent is idle, at most once per stage and head commit of the pull request. Keep it on one line. |
| `deployed` | bool | For the Deployed Criterion: whether the change is in production. It decides instead of `met` when given. |

Unknown fields are ignored in both directions, and fields added later will be optional, so a Criterion keeps working across versions.

A Criterion that exits with another status, runs longer than its `timeout` or prints something that is not an answer shows the error on its pull request's row, in the stage reached before it, until it answers again; the other rows keep updating. A Criterion that times out is killed with every process it started. Its stderr goes to the watcher's log in `$XDG_STATE_HOME/tmux-aside/`. An invalid `criteria` config shows on every row.

A Criterion may act on its own, such as requesting a review from a team or setting a label, but it runs on every fetch, so whatever it does must be idempotent.

## Example

```sh
#!/bin/sh
# ready-to-merge: also require the merge:ready label
labels=$(jq -r '.pull_request.labels | join(" ")')
case " $labels " in
  *" merge:ready "*) echo '{"met":true,"summary":"waiting to be merged"}' ;;
  *) echo '{"met":false,"summary":"missing merge:ready","tone":"warning"}' ;;
esac
```
