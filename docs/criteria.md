# Writing a Criterion

A Criterion decides whether a pull request reached a stage of the Lifecycle, and which Nudge, if any, would move it closer. The [watcher](../README.md#watcher) asks the Criteria of the stages after Implementing in order and stops at the first one that is not met. Each pull request's stage is evaluated again on every fetch, so a pull request falls back when a Criterion it met stops holding.

tmux-aside ships a default Criterion, which uses only what GitHub tells, and you can replace it in the config:

```yaml
criteria:
  ready_to_merge:
    command: /path/to/ready-to-merge   # an executable; ~ is not expanded
    args: ["--team", "web"]            # passed as command-line arguments
    timeout: 10s                       # how long it may run (the default)
```

The watcher asks the `ready_to_merge` Criterion; the config also accepts `qa`, `ready_to_deploy` and `deployed`, the stages that follow. The default Ready to merge Criterion is `tmux-aside-watcher criterion ready-to-merge`, which goes through the same contract: met when the pull request is approved, its checks pass and it merges cleanly, with a Nudge on conflicts and then on unresolved review comments.

## The contract

The watcher runs the command with its arguments, writes one JSON object to its stdin and closes it:

```json
{"stage": "ready_to_merge", "url": "https://github.com/org/app/pull/12", "pull_request": {"number": 12, "head": "4f2c...", "draft": false, "review": "APPROVED", "checks": "SUCCESS", "conflicting": false, "unresolved_threads": 0, "labels": ["merge:ready"], "...": "..."}}
```

`pull_request` holds what the watcher already fetched, so most Criteria need no GitHub call of their own. The Criterion prints one JSON object on stdout and exits with status 0:

| Field | Type | |
|---|---|---|
| `met` | bool | Whether the pull request reached the stage. Required. |
| `summary` | string | What is missing to reach the stage, or where the pull request stands once it is met. Shown on the row as written. |
| `tone` | tone | The row's color: `plain`, `muted`, `accent`, `success`, `warning` or `danger`. `accent` until met and `success` once met when unset. |
| `nudge` | string | A command for the agent, such as asking it to resolve conflicts. The watcher types it into the agent while the agent is idle, at most once per stage and head commit of the pull request. Keep it on one line. |
| `deployed` | bool | For the Deployed Criterion: whether the change is in production. |

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
