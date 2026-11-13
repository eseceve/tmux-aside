# tmux-aside

A tmux sidebar built from plugin sections, which also launches AI agents to carry pull requests through a fixed lifecycle while showing where each one stands.

## Language

### Lifecycle

**Lifecycle**:
The fixed sequence each launched pull request goes through on its own: Implementing, Ready to merge, QA, Ready to deploy. A pull request can fall back to an earlier stage when the Criterion it met stops holding, such as a new conflict before the merge.
_Avoid_: Workflow, pipeline, process

**Group**:
An optional label shared by pull requests that belong together, such as those of one task or those that must be tested together; their QA runs once, when all of them reached QA. A pull request without one is a Group of its own.
_Avoid_: Task (each tracker means something else by it), batch, set

**Ready to merge**:
The stage where the pull request meets its merge criteria and only waits for someone to merge it.
_Avoid_: Done, approved

**QA**:
The stage where the merged change is verified in a pre-production environment.
_Avoid_: Testing, validation

**Ready to deploy**:
The stage where QA passed and the change only waits to be promoted to production. The pull request leaves the Lifecycle once it is Deployed.
_Avoid_: Done, shippable

**Deployed**:
The end of the Lifecycle: the change reached production. Without a Pack a pull request counts as Deployed as soon as it is Ready to deploy.
_Avoid_: Released, shipped, live

**Criterion**:
The check that decides whether a pull request has reached a stage of the Lifecycle, and which Nudge, if any, would move it closer; tmux-aside ships a default for each and a Pack can replace it.
_Avoid_: Rule, condition, definition of done

**Watcher**:
The non-AI process that follows a pull request, evaluates the Criterion of its stage and Nudges its agent when there is work to do.
_Avoid_: Monitor (taken by sidebar sections and Datadog), bot, poller

**Nudge**:
A command the Watcher types into an idle agent, such as asking it to resolve new review comments; the agent decides how.
_Avoid_: Trigger, prompt, ping

### Extension

**Pack**:
A private bundle of Criteria and Templates for one organization, installed on top of tmux-aside.

**Launch**:
An agent started by tmux-aside in its own worktree and window from a Template, or an agent already working on a pull request that registered itself with `follow`; it enters the Lifecycle only if it carries a pull request, opened by its Template, adopted with `launch --pr`, or discovered by the Watcher from the sources the config lists.
_Avoid_: Job, run, session

**Template**:
The prompt an agent is launched with, written by the user or a Pack, into which the Task reference is filled.
_Avoid_: Prompt file, recipe

**Task reference**:
Free text naming the work to launch, such as a tracker URL or a sentence; only the Template knows what to do with it.
_Avoid_: Task id, ticket
_Avoid_: Profile, preset, plugin (a Plugin draws a section)
