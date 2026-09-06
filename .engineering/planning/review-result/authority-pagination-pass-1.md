---
format: aep.planning-md/1
id: review-result:authority-pagination-pass-1
kind: review-result
status: active
title: Independent terminal authority pagination review
relations:
- reviews: story:consume-authority-query-pages
revision: 1
---
approve

No actionable findings in the reviewed Workspace source, HTTP regressions, or release metadata.

Scope: the six runtime/test/release files against 35b204b246a2ee73fb6fe39387abc3afdf457ffb, excluding .engineering. The new crates/workspace-service/src/authority_query_tests.rs file was untracked during review and was inspected in full. The complete six-file full-index patch SHA-256 is ec1213c8dc40854a60b5000bbe3b15d62acb8ecd6401da7428eec304c50a52ab, computed by combining the tracked patch with that new file's full-index addition and sorting file sections. The tracked-only patch SHA-256 is 2197b112c017d5290853f67681f1e2771897c55577cb9a472a0e22cb6aeb6c54.

The shared collector in crates/workspace-service/src/main.rs:534 now consumes raw projection continuations, including empty filtered pages. Each request preserves the authenticated authority, operation, and session selector; returned opaque cursors are replayed unchanged. The collector requires completion, rejects cycles, admits at most ten 1,000-row pages, and bounds accumulated visible rows at 10,000. An early matching row cannot turn a malformed, refused, or unfinished later page into success.

The parser in crates/workspace-service/src/main.rs:606 requires correctly typed items, partial, cursor, and revision metadata. Partial must agree with the presence of a nonempty string cursor. Nonempty QueryPage responses require an unsigned authorized revision, and differing supplied revisions fail with a conflict. Empty pages retain previously observed revision evidence. Legacy bare-array output remains accepted only as an initial, complete response and is now subject to the total row bound; a bare array during continuation is refused. The old permissive rows-object extraction is removed from terminal admission; the released generated-service seam uses QueryPage, not that unversioned wrapper.

Both get_session and list_grants in crates/workspace-service/src/main.rs:3212 now use this collector. All terminal session bindings remain unchanged: exact AgentIDE session, materialization, Workspace session, project, source revision, manifest, authenticated owner, and Active state. Grant checks still require the exact grant and session, authenticated grantee, Active state, Medium risk, interactive_terminal intent, root path admission, and unexpired authority. Verification still precedes terminal reservation. No credential handling, provider authority, confinement profile, session storage, or Substrate behavior changes.

The new tests exercise the production authority verifier and shared collector through actual Identity and Connector HTTP clients. The fixture checks authenticated owner context, Describe/Invoke operation identities, request envelopes, and the exact session selector, cursor, page limit, description reference, and absence of approval evidence. It covers later-page session and grant discovery; wrong owner, revision, grantee, grant state, intent and expiry; malformed or missing continuation fields; cyclic and endless pages; bounded legacy output; revision changes or malformed revisions; and a later Connector refusal after an early matching session. This targets the reported admission failure without requiring terminal allocation.

The retained predecessor regression fails with 403 and exit 101. The corrected focused suite passes all six cases. The initial full gate failed on a test-fixture by-value Clippy warning; the final fixture consumes that value, preserving the emitted QueryPage bytes. The corrected task check exits zero, including formatting, Clippy, 67 workspace Rust tests and 20 publication-policy tests. These retained results were inspected; no builds or tests were rerun for this review.

Cargo.toml, Cargo.lock, and CHANGELOG.md prepare 0.2.23. Only the three local workspace package versions change in Cargo.lock; all dependency identities and other lock fields remain unchanged.

Limitations: the ten-page raw scan is intentionally finite and refuses a continuing inventory at its boundary. Consistency checking compares supplied per-query aggregate revisions; it does not introduce a snapshot transaction across separate session and grant queries. Legacy complete arrays retain their existing lack of revision metadata. The HTTP fixture does not run the generated-service database or allocate a PTY; the retained live pagination probe separately established the released raw-window behavior. Published/deployed terminal acceptance and model-credential availability remain separate work.

Reviewed file SHA-256 values:
- CHANGELOG.md: ac46840ea8941d7e283c70282aff94f56fc5754b958979021cbf8efc1ffdb55c
- Cargo.lock: 02adfce15411f5a1fde7f6865c79387603bc999205bc0db624dc84e53593a3f3
- Cargo.toml: b3e6235812664f21b19f55005df3f4821287773f52fb127ca47d82ce37163849
- crates/workspace-service/src/authority_query_tests.rs: 36967b03d73851cdac0ff36090333c332fe65243bdc6d598b52ad74206209cb3
- crates/workspace-service/src/main.rs: b14f40c89f91ea5d515ef52456fbaa541dbd7bdbf7cae1f6db3269d7ff97d378
- crates/workspace-service/src/repository_search_tests.rs: 9a1c2d8933c51d21e208bda44a084dfaf365f4e956dc4f2a0fb43f36cf9eb52d

```findings
[]
```

