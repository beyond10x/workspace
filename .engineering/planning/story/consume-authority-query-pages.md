---
format: aep.planning-md/1
id: story:consume-authority-query-pages
kind: story
status: implemented
title: Consume all bounded authority query pages before terminal admission
relations:
- informed_by: story:current-agentide-contract
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: Cargo.lock
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: crates/workspace-service/src/authority_query_tests.rs
- confidence: cited
  path: crates/workspace-service/src/main.rs
- confidence: cited
  path: crates/workspace-service/src/repository_search_tests.rs
revision: 11
---
## Outcome

Admit an owned terminal whose session or grant appears after the first raw projection page. Preserve all existing owner, session, workspace-root, project, revision, manifest, state, intent, risk and expiry checks. Never turn an incomplete query into a claim that authority is absent.

## Evidence

Workspace0.2.22 verify_terminal_grant requests agentide.get_session with a raw page limit of2, then service_rows discards partial and next_cursor. The released Service SDK defines page limits over raw projection rows and filters selectors afterward. An authenticated read-only deployed comparison observes zero matching rows with partial=true and a cursor at limit2, one matching row at limit100, and the same row after following five limit2 pages. The complete browser journey therefore receives terminal_session_binding_refused even though its exact workspace and current grant bindings match. A narrow terminal API test happens to succeed when its session sorts into the initial raw window.

The existing agentide_service_rows helper also rejects partial=true before consuming its cursor. Correct the shared Workspace consumer and use it for terminal session and grant lookups. Bounded pagination must include empty filtered pages, reject inconsistent metadata and cursor cycles, and stop explicitly when its page or row budget is exhausted. Devcenter owns a matching consumer correction for its current100-row boundary under its existing Projects recovery story. This change introduces no entity or authority source; existing Workspace and AgentIDE contracts remain authoritative.

## Acceptance

A regression supplies an empty partial page followed by a matching session/grant and proves terminal verification succeeds only when every existing binding is valid. Negative cases preserve wrong-owner/session/grant and expiry refusals; malformed or non-terminating continuation must return an explicit incomplete/invalid response within the configured bounds. Run the full repository gate, independent review, immutable release, reviewed downstream composition and real headless Files-plus-terminal output/cleanup acceptance. Agent model credentials remain separate.

## Scope

crates/workspace-service/src/main.rs owns authority queries, terminal admission and existing tests. Cargo.toml, Cargo.lock and CHANGELOG.md carry the next immutable runtime publication. Scope is cited from the released implementation and current repository version metadata.

## Implementation and regression

The shared authority consumer now follows up to ten1000-row raw windows, including empty filtered pages. It rejects inconsistent partial/cursor metadata, repeated cursors, excess page/row budgets and conflicting supplied aggregate revisions. Nonempty envelopes require an unsigned authorized revision; a bounded legacy array remains a complete initial response only. Terminal session and grant queries both use this consumer, and their matching predicates remain unchanged.

The deciding new regression runs through the real Connector HTTP client and reproducesHTTP403 with the previous terminal verifier. The corrected implementation passes six new HTTP cases covering later session/grant matches, unchanged authority refusals, malformed/cyclic/exhausted continuation, revision consistency and a refusal after an earlier matching row. Existing Connector fixtures now carry the actual generated-service revision metadata. Full gate, independent final-source review, immutable publication and deployed browser output remain required before completion.

## Reviewed publication

PR25 merged as 050d8adf82a4565692002fcd7e2b7f3d9be62c71. The release tag 0.2.23 names exact reviewed source 74bf4aac051ccb1c7db5f151c5d8a59e4cf44fcf, whose tree matches the merge and which is an ancestor of the remote default branch. All six reviewed file identities were checked before integration. Full local task check passed 67 Rust cases and 20 release-policy/adversary cases with formatting and Clippy. The repository's PR workflow is a release-policy check; it is not a remote substitute for that full local gate.

Release 34057012414 completed successfully, including both architecture builds and keyless signing plus exact workflow-certificate verification. The immutable manifest selects workspace_service sha256:9747a2073949b0a75151561496c713f21fea8f17d09968eb41be21454d5aa8d2, AMD64 child sha256:a4ef99b7ea56711a2d784ec1172c22cbf4d32d62e0730fd4c138094fbc8465cb and ARM64 child sha256:40d1793d600fb59c1d3027b227acda2272b446ec7dd16dd095cd74a8cfdacc03. The manifest source agrees with the reviewed tagged commit. Downstream composition and real headless Files-plus-terminal output and cleanup remain required before the story is implemented. No Agent-credential recovery is claimed by this publication.

## Deployed acceptance

The reviewed immutable server 0.8.33 and Workspace 0.2.23 composition passed normal downstream artifact validation, atomic deployment and deployed verification. The live inventory changes only the two intended runtime images among thirteen workloads, preserving all other images, replicas, durable claim identities and installed execution profiles. Every workload is Ready.

Authenticated headless acceptance proves fresh Git creation, Project Files entry and persisted pane navigation, editor layout, real keyboard save, exact content/hash restoration and reload. After initial PTY output and canvas focus, the actual browser sends fifty binary input bytes and receives ten binary output frames including the complete split random printf sentinel. The terminal is explicitly terminated; the workspace remains Ready with a readable file tree afterward. The disposable Workspace and coordination close after one normal reconciliation retry. The separately preserved operator workspace remains untouched and readable.

An earlier browser attempt created the terminal and displayed a Bash prompt but did not receive its command sentinel. That full failed result is retained. The later test changes both readiness and focus conditions, so it does not isolate which condition caused the earlier missing input. Independent review accepts the bounded Files and terminal proof and its cleanup, with no findings. Both Agent checks still fail and overall Devcenter acceptance remains incomplete; the newly exposed terminal-context directory repair is owned by story:normalize-terminal-context-directory, and model credentials remain a separate unresolved boundary.

The retained deciding browser result has SHA-256 a4e043cf44172cc36c4e5af73b3c5e2ab9d5d46fa929b6158dab92bc9a099a59. The independent deployed acceptance review has SHA-256 3272582eb5f35b9bafe70704b647ece3d59771f391de2cda10fb60e811caa050. No Agent reply is claimed by completion of this authority-pagination story.
