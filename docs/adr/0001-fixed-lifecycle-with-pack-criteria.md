# A fixed Lifecycle, with Criteria as Pack commands and one central Watcher

tmux-aside carries every launched pull request through a fixed Lifecycle (Implementing, Ready to merge, QA, Ready to deploy, Deployed), but never knows what a stage means: each Criterion is an executable that answers whether the stage is met and which Nudge would help, tmux-aside ships a GitHub-only default for each, and an organization replaces them with a private Pack. One Watcher, a `stream` plugin, follows every launched pull request and draws the section, so the open-source project holds no company knowledge while a team still gets one row per pull request that reacts to its own rules.

## Considered Options

- **Stages defined by each Pack**: rejected, it turns tmux-aside into a workflow engine for nobody in particular, and the sidebar could no longer show or react to a stage.
- **Declarative criteria** (required checks, labels, review team): rejected, things like "the merge reached staging according to Jenkins" fit no generic field.
- **A Watcher per pull request, in a pane of its window** (the old `pr-watch.sh`): rejected, it dies with the window at Ready to merge, before QA and Ready to deploy, and polls GitHub once per pull request.
- **Each Pack shipping its own Watcher**: rejected, it duplicates the hard part (polling, Nudges, the section).

## Consequences

- A Pack is executable code that tmux-aside runs, so `setup --pack <url>` clones it once and updates it only when run again, never on its own.
- Without a Pack, a pull request enters QA at its merge and counts as Deployed as soon as it is Ready to deploy.
