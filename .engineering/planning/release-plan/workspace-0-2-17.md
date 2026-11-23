---
format: aep.planning-md/3
id: release-plan:workspace-0-2-17
kind: release-plan
status: active
title: Release Workspace 0.2.17
summary: Publish native exact-revision Git materialization through independently versioned service seams.
relations:
- delivers: story:bounded-coding-session-materialization
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-09-04T23:26:22Z", actor: "human:timo", revision: 2, imported: true}
---
## Outcome

Publish Workspace 0.2.17 from the exact bot-authored commit with Connector-authorized, Substrate-native Git materialization and recovery hardening.

## Qualification

The complete Workspace gate passes with immutable Connectors, AgentIDE, and Substrate revisions. The released runtime image is promoted by digest into the downstream deployment.
