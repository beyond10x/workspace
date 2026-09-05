---
format: aep.planning-md/1
id: story:projects-connection-recovery
kind: story
status: active
title: Recover repository discovery without usable GitLab authority
relations:
- informed_by: story:bounded-coding-session-materialization
scope:
- confidence: cited
  path: crates/workspace-service/src/main.rs
revision: 4
---
## Outcome

Repository search handles absent GitLab operation authority as a recoverable connection state, without collapsing it into a gateway failure or admitting hidden repositories. This serves O1 and O4.

## Evidence

The current search_projects function directly describes gitlab-project-list. The deployed GitLab backend returns operation NotFound when it has no usable current-grant connection. connector_operation maps that refusal to connector_read_refused (502). A separate live state inspection is in progress; this is a verified reachable code path, not yet a claim about the particular user record.

## Acceptance

A test uses the real Workspace HTTP/client boundary with a Connector fixture that hides the operation when no usable connection exists. Repository discovery uses admitted operations before description/invocation, returns no unauthorized repositories, and does not mask actual transport, policy, provider, or protocol failures as empty success. A connected-user regression preserves repository discovery. The Devcenter companion change provides the connection recovery route and accurate failure text. Runtime publication and hosted authenticated verification are required for delivery.

## Scope

- cited: crates/workspace-service/src/main.rs, repository search and Connector operation error mapping.
- inferred: Cargo.toml and Cargo.lock for the separately authorized runtime release.
