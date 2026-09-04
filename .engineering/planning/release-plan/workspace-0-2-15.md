---
format: aep.planning-md/1
id: release-plan:workspace-0-2-15
kind: release-plan
status: active
title: Release Workspace 0.2.15
summary: Publish durable project Workflow-run recovery.
relations:
- delivers: story:complete-project-workflow-runs
revision: 2
---
## Outcome

Workspace 0.2.15 is published from the exact bot-authored main commit and exposes recoverable, owner-bound project Workflow execution to Devcenter.

## Scope

Version and changelog alignment, the complete repository gate, bot-authored main publication, annotated tag, and promotion into the independently built Workspace runtime image.

## Qualification

The Workspace gate passes at the release commit. The tag peels to that commit, and Devcenter promotion publishes an immutable multi-architecture image manifest.
