---
format: aep.planning-md/1
id: story:independent-runtime-publication
kind: story
status: implemented
title: Publish the Workspace runtime from its owner repository
scope:
- confidence: cited
  path: .dockerignore
- confidence: inferred
  path: .github/workflows/release.yml
- confidence: cited
  path: Dockerfile
- confidence: cited
  path: Taskfile.yml
revision: 6
---
## Outcome

Workspace owns publication of its runnable service image, serving O4 from AGENTS.md and the independent-component-delivery epic.

## Context

Workspace 0.2.17 already has a Dockerfile, but no owner release workflow. Devcenter .github/workflows/promote-workspace.yml builds and signs it from a sibling checkout. Ownership is the missing boundary, rather than the existence of any image build.

## Acceptance

A Workspace release triggered from its own repository validates exact source and version, builds and smoke-tests the runtime for its declared architectures, signs the immutable image digest and publishes durable version/source/digest metadata that a downstream deployment can consume without running Devcenter publication.

Publication must verify source against the provider-reported default branch, scope credentials to needed dependencies, retain private image visibility, support safe retries without overwriting successful version identities, and perform no cluster mutation. PR validation exercises release behavior without publishing. Reuse the existing owner Dockerfile; no product/server/chart rebuild participates.

## Scope

Workspace Dockerfile and Cargo.toml are cited. Workspace .github/workflows/release.yml and focused Rust release checks are inferred. Devcenter .github/workflows/promote-workspace.yml is cited and coordinator-owned for retirement or redirection once the owner path exists.

Confidence high for the existing build and old promotion path, inferred for new workflow files. Separate repository paths cannot collide with Devcenter publication implementation.

## Out of scope

Workspace currently lacks a complete ESS component build/runtime manifest; creating that model is a separate pre-existing adoption gap. This story establishes owner publication using the existing Dockerfile and durable digest metadata without introducing a second ESS model or rebuilding a chart.
