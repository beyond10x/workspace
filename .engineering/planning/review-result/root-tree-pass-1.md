---
format: aep.planning-md/1
id: review-result:root-tree-pass-1
kind: review-result
status: active
title: Independent review of bounded workspace root pages
relations:
- reviews: story:serve-workspace-root-tree
revision: 1
---
approve

No actionable findings in the Workspace implementation reviewed against `c5fde67ecb119d8f56a75985d97c176838c2b27e`. Recommend accepting the bounded root-tree correction for release and downstream acceptance.

The released SDK 0.7.3 at source `329a128606a7c18c4477d6fff58ffc57b296fb70` deliberately validates a nonempty relative path in `crates/b10x-substrate-sdk/src/lib.rs:754`; its existing tree method at `crates/b10x-substrate-sdk/src/lib.rs:926` uses the v2 recursive observation. The correction selects that existing method only for the empty root (`crates/workspace-service/src/main.rs:2083`). It passes the stored materialization limit with hidden entries enabled. Nonempty paths still use native directory reads and forward their cursor and limit unchanged (`crates/workspace-service/src/main.rs:2092`). No Substrate source, dependency, route, authority, or wire contract changes are present.

The root projection first refuses a truncated observation with a named 503, and refuses a foreign materialization or oversized response with 502 (`crates/workspace-service/src/main.rs:2135`). It selects immediate children, preserves entry kinds and sizes, and sorts by path bytes before paging. Inspection of the released Substrate 0.7.5 source `64ae2ed5a888663b036cbe06515cbfd277369d58`, `crates/substrate-host/src/fs.rs:308` and `crates/substrate-host/src/fs.rs:787`, confirms the complete-tree/truncation behavior, omission of Git control directories, and refusal to follow symlinks during recursive directory descent. This does not infer deployed daemon behavior from the older SDK package version.

The root cursor hashes the owned session identifier, observed materialization identifier, and sorted root projection (`crates/workspace-service/src/main.rs:2167`). Canonical positive offsets and digest syntax are checked; offsets outside the current listing and foreign cursor formats are refused, while a changed digest is stale. The cursor is pagination state, not an authority token: each page still passes the existing Identity authentication, owner-scoped store lookup, current project access, Ready-state requirement, and Substrate authority path (`crates/workspace-service/src/main.rs:2019`, `crates/workspace-service/src/main.rs:4408`). Existing route checks bound the page limit and cursor size before the helper is called. File reads, writes, source binding, and materialization limits are unchanged.

The transport fixture uses the real pinned SDK over its Unix HTTP transport. The retained predecessor run fails the root request with 502; the corrected run verifies the exact tree route and query for both root pages and the exact native child-directory route and cursor. Assertions cover hidden root visibility, descendant exclusion, page completion, and preservation of nested pagination (`crates/workspace-service/src/main.rs:6399`, `crates/workspace-service/src/main.rs:6563`). The two additional projection tests cover deterministic ordering, empty roots, malformed and out-of-range cursors, changed and foreign snapshots, materialization mismatch, oversized observations, and truncation (`crates/workspace-service/src/main.rs:6094`). The existing create-recovery and stale-Preparing lifecycle checks remain in the passing fixture.

The final full `task check` evidence passes toolchain checks, formatting, Clippy with warnings denied, all 61 Rust cases, and 20 publication policy/adversary cases. The earlier intermediate projection-test compilation failure is superseded by that successful full gate, including both new projection cases. The reviewer inspected these logs without repeating compilation or tests. Whitespace validation passes. Parsed lock comparison confirms that only the three local package versions advance from 0.2.21 to 0.2.22. The reviewed diff for `crates/workspace-service/src/main.rs`, `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` has SHA256 `3f864d706548b6a795f9fbba7e2c78a90c2b602c5391345633ca8bf7e7f21ec0`.

Limits: every root page repeats the bounded recursive observation; this is not native root-directory pagination for larger materializations. Only complete observations within the existing bound produce root pages. The cursor binds root names, kinds, and sizes, not file contents, descendant changes, or a filesystem transaction; changing observation time alone correctly leaves it usable. The retained authenticated live proof confirms the released observation succeeds without truncation, but does not prove the corrected browser flow is deployed. Release CI, publication, and downstream explorer/read/edit/save/restore/close acceptance remain separate evidence. Planning mutations and publication were outside this read-only review.

```findings
[]
```
