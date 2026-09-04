---
format: aep.planning-md/1
id: story:one-agentide-contract-graph
kind: story
status: draft
title: One AgentIDE contract graph in the lockfile
summary: agentide-contracts 0.2.1 and 0.3.1 both resolve (via agent-platform-core 0.6.7); schemars 0.8 and 1.0 likewise.
revision: 1
---
# Story: One AgentIDE contract graph in the lockfile

## Context

`Cargo.lock` resolves two `agentide-contracts` generations: 0.2.1 (pulled transitively by
`agent-platform-core 0.6.7`) beside the pinned 0.3.1. The same defect class was closed for Identity
in 0.2.12. `schemars` 0.8 and 1.0 are likewise duplicated. Found while re-pinning AEP 0.51.0
(2026-09-04); pre-existing on `origin/main` 7e0ca31.

## Acceptance

`cargo metadata` shows one `agentide-contracts` and one `schemars` generation, and a test asserts no
package from a beyond10x git source resolves at two versions.
