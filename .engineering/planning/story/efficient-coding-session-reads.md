---
format: aep.planning-md/1
id: story:efficient-coding-session-reads
kind: story
status: implemented
title: Reuse transport resources for coding workspace reads
tags:
- coding-workspace-speed
relations:
- derived_from: story:bounded-coding-session-materialization
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: Cargo.lock
- confidence: inferred
  path: Cargo.toml
- confidence: inferred
  path: crates/workspace-service
revision: 7
---
## Acceptance

Coding-session file and tree reads reuse transport configuration and connections while preserving current Identity authority, Connector grants, readiness checks and bounded reads. The endpoint factory must never cache caller credentials or grants.

## Implementation and evidence

Source candidate 0.2.19 retains one credential-free Substrate RemoteEndpoint per deployment configuration. Each caller binds its own current Identity token provider and performs the existing machine/contract handshake. Server-Timing reports handler duration without project or actor identifiers. Exact source pins compose Substrate 0.7.3 at 329a128606a7c18c4477d6fff58ffc57b296fb70 and Connectors 0.6.4 at dbdd285c629d8b93bb685cc5a89a270316978ce5.

The complete `task check` gate passed against those published Git dependencies with exit 0, including toolchain, release-policy/adversarial tests, formatting, Clippy with warnings denied, workspace unit tests and doc tests. Temporary local SDK overrides are not part of this change or its final lockfile. Substrate's public endpoint test separately proves that reused TLS rejects an invalid caller after a valid caller, and its pool tests prove cancellation, concurrency and idle bounds.

Provider and consumer review branches precede release approval and deployment. Production workspace timing remains unobserved until the operator identifies its deployment target.
