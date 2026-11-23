---
format: aep.planning-md/3
id: story:complete-project-workflow-runs
kind: story
status: implemented
title: Complete project Workflow runs
summary: Drive every accepted project workflow to one observable terminal task result.
scope:
- confidence: cited
  path: crates/workspace-client/src/lib.rs
- confidence: cited
  path: crates/workspace-core/src/lib.rs
- confidence: cited
  path: crates/workspace-service/src/main.rs
- confidence: cited
  path: crates/workspace-service/src/store.rs
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-09-04T11:04:58Z", actor: "human:timo", revision: 3, imported: true}
- {from: "proposed", to: "active", at: "2026-09-04T11:04:58Z", actor: "human:timo", revision: 4, imported: true}
- {from: "active", to: "implemented", at: "2026-09-04T11:42:24Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1,"review_outcome":4}}, imported: true}
---
## Context

Workspace persists the accepted workflow run and dispatches Agent Platform work from a process-local task. A restart or failed follow-up can leave the durable projection accepted or running forever even when admission succeeded, so Devcenter has nothing useful to observe.

## Acceptance

For an accepted project workflow, Workspace durably records the Agent Platform task reference before returning, resumes terminal-state observation after restart, and changes the run exactly once to succeeded with Markdown output or failed with a named safe code instead of leaving it indefinitely accepted or running.

## Scope

The WorkflowRun projection and persistence, Agent Platform dispatch/observation boundary, Workspace client shape if required, and restart/idempotency tests.
