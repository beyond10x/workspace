---
format: aep.planning-md/1
id: story:preserve-ready-materialization
kind: story
status: implemented
title: Preserve ready materialization during delayed recovery
relations:
- informed_by: story:bounded-coding-session-materialization
- informed_by: task:remediate-git-materialization-recovery
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: Cargo.lock
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: crates/workspace-service/src/main.rs
revision: 9
---
## Outcome

Keep a successfully published coding workspace and its files available when a delayed preparing-session read schedules recovery after the original worker finishes. This corrects the existing materialization lifecycle; no entity or authority is introduced.

## Evidence

Authenticated creation reaches ready, then tree, file and terminal metadata reads return not found. A read-only durable operation inspection proves the same deterministic Workspace create completed successfully and Workspace's deterministic cleanup operation destroyed that exact resource immediately afterward. The service can read preparing, await provider access, and only then check its worker registry. The original worker can publish ready and release its guard during that await. A recovery worker then replays create, fails the preparing-only reference update and cleans up the published workspace.

## Implementation and acceptance

After acquiring the existing single-worker guard, reload durable session state before provisioning effects. A delayed recovery request must do nothing for a session that is already ready or has entered a terminal/cleanup state. Preserve normal preparing recovery and exact cleanup when Close wins while a create is in flight. Store-read uncertainty must not authorize destruction. Add a deterministic regression that completes the first materialization, replays the old preparing snapshot, and proves the ready record/reference remain unchanged without further provider/create/destroy operations. Retain the existing lost-response recovery and close-race tests.

Publish the corrected Workspace runtime through its existing versioned release workflow. The downstream Devcenter Projects repair coordinates consumption and deployment. Authenticated browser read/edit/save/restore/close remains the product acceptance gate.

## Scope

- cited: crates/workspace-service/src/main.rs; recovery scheduling, provisioning, and the real transport fixture.
- inferred: crates/workspace-service/src/store.rs if a durable compare-and-set change is required by the regression.
- cited: Cargo.toml, Cargo.lock, CHANGELOG.md for the next immutable runtime release.

This is one bounded repair, so no multi-item decomposition panel is applicable.

## Verification

The extended real transport regression fails the predecessor when replaying an old preparing snapshot after ready publication. The correction passes that same fixture, preserving the exact ready record and leaving subsequent closing/closed records unchanged while both fixture servers are stopped. The existing lost-response create recovery remains covered. No store change was needed.

The full task check passes: toolchain and release-policy checks, five policy cases, fifteen adversarial publication cases, formatting, workspace Clippy with warnings denied, and all fifty-nine Rust cases. Prepare runtime0.2.21 with only the three local package versions advanced; all external dependency pins remain unchanged. Independent source review and normal publication precede downstream acceptance.

## Published and deployed verification

Runtime0.2.21 was published from c5fde67ecb119d8f56a75985d97c176838c2b27e in successful release run34023626908, with signed immutable image digest sha256:c2fe42ad1f7ffec7c89f465216bd95c5b6ca68f92457e6e521e945b74834977f. The normal downstream validation, deployment and verification jobs succeeded and the ready runtime reports that exact image digest with zero restarts.

Authenticated coding sessions remain ready with their materializations present; independent direct file and terminal metadata reads return success. Normal close/retry closes both Workspace and AgentIDE coordination after the companion consumer fix. The root explorer call exposes a separate empty-path mismatch owned by story:serve-workspace-root-tree; this lifecycle correction is delivered, while the final product read/edit/save/restore acceptance remains with that follow-up and the Devcenter coordination story. No Substrate source change was necessary.
