---
format: aep.planning-md/1
id: release-plan:workspace-0-2-16
kind: release-plan
status: active
title: Release Workspace 0.2.16
summary: Publish bounded and recoverable coding-session startup.
relations:
- delivers: story:bounded-coding-session-materialization
revision: 2
---
## Outcome

Workspace 0.2.16 is published from the exact bot-authored main commit and makes coding-session preparation asynchronous, recoverable, bounded, and observable to released clients.

## Scope

Version and changelog alignment, the complete repository gate, bot-authored main publication, annotated tag, and promotion into the independently built Workspace runtime image.

## Qualification

The Workspace gate passes at the release commit. The tag peels to that commit, and Devcenter promotion publishes an immutable multi-architecture image manifest.
