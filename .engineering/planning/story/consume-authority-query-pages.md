---
format: aep.planning-md/1
id: story:consume-authority-query-pages
kind: story
status: active
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
revision: 6
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
