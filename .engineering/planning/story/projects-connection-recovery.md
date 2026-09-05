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
  path: Cargo.lock
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: crates/workspace-service/src/main.rs
- confidence: cited
  path: crates/workspace-service/src/repository_search_tests.rs
revision: 7
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

## Hosted branch discovery finding

Authenticated repository discovery recovered after normal OAuth reconnect. Project details loaded, but branches took roughly 18 seconds and a subsequent branch selection returned 503 after timing out. Source inspection shows discover_branches resolves gitlab.branches bindings by scanning the provider membership project catalogue; each datasource page resolves the same binding through another scan. This measured UI path is in the original Projects loading scope.

The repair additionally uses the existing admitted gitlab-branch-list operation, bound to the already revalidated project and connection, with 100 records per provider page. It preserves the current Branch response and failure semantics, fetches additional pages until a short page, and never uses missing authorization as permission. Regression fixtures must prove exact numeric project identity and fresh description use, current connection admission, page progression, and errors instead of hidden partial lists. No cross-principal cache or new endpoint is introduced.

## Integrated validation

Integration commit 1f225640586ecb61ffc9c7cb4ed905398b0242a4 passed the complete `task check` on the final source and adversarial tests: 59 Cargo tests plus 5 release-policy and 15 release-adversary cases, all passing. Formatting and full-workspace Clippy with warnings denied passed. Runtime was 10.707 seconds with per-tree build output. The test-runner-only short TMPDIR resolves inside the assigned tree and preserves the existing Unix-socket fixture.

The adversary added two reachable authority/reconnection cases and found no defect; its report is retained verbatim. The installed planning CLI validates the store but warns that the report has no findings despite its explicit empty findings fence; no report bytes were rewritten to suppress that tooling warning.

Runtime 0.2.20 is ready for publication. Hosted deployment verification remains required before this story closes.
