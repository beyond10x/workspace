---
format: aep.planning-md/1
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
---
## Context

Workspace persists the accepted workflow run and dispatches Agent Platform work from a process-local task. A restart or failed follow-up can leave the durable projection accepted or running forever even when admission succeeded, so Devcenter has nothing useful to observe.

## Acceptance

For an accepted project workflow, Workspace durably records the Agent Platform task reference before returning, resumes terminal-state observation after restart, and changes the run exactly once to succeeded with Markdown output or failed with a named safe code instead of leaving it indefinitely accepted or running.

## Scope

The WorkflowRun projection and persistence, Agent Platform dispatch/observation boundary, Workspace client shape if required, and restart/idempotency tests.
