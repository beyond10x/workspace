---
format: aep.planning-md/1
id: story:current-agentide-contract
kind: story
status: implemented
title: Align Workspace with current AgentIDE contracts
summary: Release one Workspace client graph that DevCenter can consume without duplicate AgentIDE contract types.
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: Cargo.lock
- confidence: cited
  path: Cargo.toml
revision: 6
---
## Outcome

Workspace consumes AgentIDE 0.3.1 and Agent Platform 0.6.7 so DevCenter can use the latest released Workspace-facing contracts as one Rust type graph.

## Acceptance

- Workspace pins AgentIDE 0.3.1 and Agent Platform 0.6.7.
- Existing project, coding-session, context, grant, diff, file, tree, terminal, agent-turn, and workflow behavior remains unchanged.
- Workspace's public client and service use the same current AgentIDE contract and pass the complete locked gate.
- Workspace releases 0.2.13 for exact downstream consumption.

## Out of Scope

Changes to Platform, Cloud, Atlas, Website, Workspace semantics, or compatibility adapters.

## Scope

- cited: Cargo.toml, Cargo.lock, CHANGELOG.md, and package version metadata.
