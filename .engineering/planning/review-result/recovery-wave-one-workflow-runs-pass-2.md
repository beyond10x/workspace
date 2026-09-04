---
format: aep.planning-md/1
id: review-result:recovery-wave-one-workflow-runs-pass-2
kind: review-result
status: active
title: 'Adversarial review: durable Workflow runs pass 2'
summary: Transient observation, fixed timeout, and ambiguous submission paths need final correction.
owner: wave-adversary
relations:
- reviews: story:complete-project-workflow-runs
revision: 1
---
## Report

unit: story:complete-project-workflow-runs
verdict: red
cases: executed 6→9, red 1
origin: introduced 3, pre-existing 0, undecided 0
wrote-outside-worktree: none
needs-coordinator: yes

Both pass-one corrections are verified. Task reassignment is refused, shared legacy task IDs remain admissible across distinct runs, missing authority preserves recovery, and schema initialization accepts legacy duplicates.

New blocker: one temporary Agent Platform read failure immediately and irreversibly marks the run failed. The added regression test receives HTTP 503 once, then would return success; Workspace performs only one request and projects `Failed`.

Two adjacent code-path findings remain: the hard-coded 150-second observation window falsely terminates legitimate long-running workflows, and ambiguous/transient task-submission failures are persisted as terminal failures instead of permitting idempotent resubmission.

Commands:

- `RUSTC_WRAPPER=/usr/bin/sccache cargo test -p workspace-service workflow --locked`
- Result: 7 tests, 6 passed, 1 failed.
- `cargo fmt --all --check`
- `git diff --check`

```findings
- file: crates/workspace-service/src/main.rs
  line: 5557
  category: recovery
  severity: blocker
  verdict: CONFIRMED
  origin: introduced
  message: A single transient task-read failure permanently fails the durable run, preventing this observer or a later authenticated observer from recovering it.
- file: crates/workspace-service/src/main.rs
  line: 5593
  category: timeout
  severity: blocker
  verdict: CONFIRMED
  origin: introduced
  message: The fixed 150-second local polling deadline permanently fails tasks that remain legitimately accepted, running, or awaiting approval beyond that window.
- file: crates/workspace-service/src/main.rs
  line: 3677
  category: dispatch
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: Any submit transport failure terminalizes the durable run even though the idempotent Agent Platform request may have succeeded and can safely be retried.
```
