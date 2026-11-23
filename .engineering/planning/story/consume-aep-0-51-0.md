---
format: aep.planning-md/3
id: story:consume-aep-0-51-0
kind: story
status: implemented
title: Workspace consumes AEP 0.51.0
summary: Re-pin aep-client and aep-contract from 0.45.0 to 0.51.0 and follow any API change between them.
scope:
- confidence: cited
  path: Cargo.lock
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: crates
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-09-04T00:19:40Z", actor: "human:timo", revision: 3, imported: true}
- {from: "proposed", to: "active", at: "2026-09-04T00:19:40Z", actor: "human:timo", revision: 4, imported: true}
- {from: "active", to: "implemented", at: "2026-09-04T00:29:06Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}, imported: true}
---
# Story: Workspace consumes AEP 0.51.0

## Context

`Cargo.toml` pins aep-client and aep-contract at tag 0.45.0. AEP 0.51.0 (2026-09-04) moved crates under area directories and
renamed adp-domain, aop-domain and protocol-cli; none of the two pinned crates changed name, and
git dependencies resolve by package name, so the re-pin is a tag change plus whatever API moved
between 0.45.0 and 0.51.0 (`aep/CHANGELOG.md` sections 0.46.0–0.51.0). 

## Acceptance

The two pins (aep-client, aep-contract) read `tag = "0.51.0"`, the lockfile resolves them from the area-qualified paths, and
`task check` exits 0.

## Notes

Cross-repository: aep 0.51.0 is released (e6a3118). Devcenter composes Workspace; its re-pin is a later story in devcenter.

