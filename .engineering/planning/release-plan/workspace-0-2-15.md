---
format: aep.planning-md/3
id: release-plan:workspace-0-2-15
kind: release-plan
status: implemented
title: Release Workspace 0.2.15
summary: Publish durable project Workflow-run recovery.
relations:
- delivers: story:complete-project-workflow-runs
revision: 3
transitions:
- {from: "draft", to: "active", at: "2026-09-04T12:50:33Z", actor: "human:timo", revision: 2, imported: true}
- {from: "active", to: "implemented", at: "2026-09-04T13:09:07Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":2,"artifact":1}}, imported: true}
---
## Outcome

Workspace 0.2.15 is published from the exact bot-authored main commit and exposes recoverable, owner-bound project Workflow execution to Devcenter.

## Scope

Version and changelog alignment, the complete repository gate, bot-authored main publication, annotated tag, and promotion into the independently built Workspace runtime image.

## Qualification

The Workspace gate passes at the release commit. The tag peels to that commit, and Devcenter promotion publishes an immutable multi-architecture image manifest.
