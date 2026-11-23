---
format: aep.planning-md/3
id: task:remediate-git-materialization-recovery
kind: task
status: implemented
title: Remediate Git materialization recovery
summary: Close the race, restart, credential, refusal, test, and ESS gaps found after the coding-session story was implemented.
relations:
- derived_from: story:bounded-coding-session-materialization
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-09-04T22:47:17Z", actor: "human:timo", revision: 2, imported: true}
- {from: "proposed", to: "active", at: "2026-09-04T22:47:17Z", actor: "human:timo", revision: 3, imported: true}
- {from: "active", to: "implemented", at: "2026-09-04T23:26:21Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}, imported: true}
---
# Task: Remediate Git materialization recovery

## Outcome

Coding-session materialization remains actor-bound, exact-revision, recoverable, and leak-free across close races, process interruption, dependency refusal, and rolling service upgrades.

## Acceptance

- Closing a preparing session cannot lose a materialization created concurrently; every known reference is destroyed or retained in durable cleanup-unknown state for retry.
- A persisted source intent resumes without consulting mutable project branch state; only a legacy row with no source intent may be backfilled from an unchanged exact project snapshot.
- Permanent Substrate configuration or authority refusal reaches a durable terminal state, while explicitly transient failures remain bounded and retryable.
- Identity-session, Connector, Agent Platform, and source capabilities use zeroizing secret ownership across background task and token-provider boundaries.
- Tests exercise prompt HTTP acknowledgement, reconstructed-store restart after the project advances, the close/create race, and permanent setup refusal.
- The durable coding-session source intent and its ownership relation are declared in the authoritative ESS domain and validate deterministically.

## Scope

Workspace coding-session lifecycle, durable source intent, materialization worker, secret ownership, failure tests, and ESS domain.
