---
format: aep.planning-md/1
id: story:bounded-coding-session-materialization
kind: story
status: active
title: Bound coding-session materialization
summary: Acknowledge coding-session creation promptly and materialize repositories without serial per-file Connector and Substrate fan-out.
relations:
- informed_by: story:current-agentide-contract
scope:
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: crates/workspace-api
- confidence: cited
  path: crates/workspace-client
- confidence: cited
  path: crates/workspace-service/src/main.rs
revision: 4
---
# Story: Bound coding-session materialization

## Outcome

Workspace accepts a coding-session request promptly, exposes preparation as durable session state, and materializes the exact authorized repository revision with bounded service traffic.

## Acceptance

- `POST /coding-sessions` persists and returns a preparing session before repository transfer begins.
- A caller can observe preparing, ready, and terminal failed/refused outcomes through the existing session read contract without holding the create request open.
- Preparation work is restart-safe or explicitly recoverable; an interrupted service process cannot leave a session spinning forever.
- Repository acquisition avoids one Connector request per directory and file and avoids uploading every file independently into two Substrate workspaces.
- Source authority is revalidated from the current Identity session and Connector grant, and credential bytes never enter persisted Workspace or Substrate state.
- Tests prove bounded acknowledgement, state transitions, exact revision behavior, interruption recovery, and no accidental serial per-file fan-out.

## Scope

Workspace HTTP session lifecycle, durable session state, repository acquisition/materialization adapter, generated client contract, and release evidence.
