---
format: aep.planning-md/1
id: review-result:recovery-wave-one-workflow-runs-pass-1
kind: review-result
status: active
title: 'Adversarial review: durable Workflow runs pass 1'
summary: Missing-authority and legacy-index migration defects require correction.
owner: wave-adversary
relations:
- reviews: story:complete-project-workflow-runs
revision: 1
---
## Report

unit: story:complete-project-workflow-runs
verdict: red
cases: executed 4→6, red 2
origin: introduced 2, pre-existing 0, undecided 0
wrote-outside-worktree: none
needs-coordinator: yes

- `WORKFLOW-RUN-ADV-001` — blocker. Listing runs without current Agent Platform authority permanently changes every recoverable run to `failed`; it must remain recoverable until fresh authority exists. Added a reproducing test.
- `WORKFLOW-RUN-ADV-002` — high. Creating the new unique task index fails startup when upgrading a legacy database containing duplicate task references. Added a migration regression test.

Commands:

- `RUSTC_WRAPPER=/usr/bin/sccache cargo test -p workspace-service workflow --locked`
- Result: 6 executed, 4 passed, 2 failed.
- `cargo fmt --all`
- `git diff --check`

```findings
- file: crates/workspace-service/src/main.rs
  line: 5473
  category: authority-lifecycle
  severity: blocker
  verdict: CONFIRMED
  origin: introduced
  message: missing current Agent Platform authority irreversibly fails otherwise recoverable accepted or running workflow runs
- file: crates/workspace-service/src/store.rs
  line: 35
  category: migration
  severity: blocker
  verdict: CONFIRMED
  origin: introduced
  message: the unconditional unique-index migration refuses startup when a legacy database contains repeated task identifiers
```
