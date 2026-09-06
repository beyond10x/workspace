---
format: aep.planning-md/1
id: review-result:preserve-ready-materialization-pass-1
kind: review-result
status: active
title: Independent ready-materialization recovery review
relations:
- reviews: story:preserve-ready-materialization
revision: 1
---
approve

No actionable findings in the implementation reviewed against `36724ee067ac2c360ce930664b15ef66299a346b`.

The normal entry acquires and retains the existing per-session worker guard before preparation (`crates/workspace-service/src/main.rs:3335`). The added owner-scoped durable reload at `crates/workspace-service/src/main.rs:3418` therefore observes publication by an earlier worker before the delayed worker can request another Git lease, replay creation, or authorize materialization cleanup. Every durable state other than Preparing returns unchanged. A failed reload is classified as retryable, and the caller exits before cleanup (`crates/workspace-service/src/main.rs:3382`). The lookup preserves tenant and owner authority (`crates/workspace-service/src/store.rs:462`).

Close can still win after the reload; the existing Preparing-only record/publication updates and exact answered-reference cleanup remain intact (`crates/workspace-service/src/store.rs:511`, `crates/workspace-service/src/store.rs:536`, `crates/workspace-service/src/main.rs:3527`). The patch does not reinterpret a stale snapshot as cleanup ownership or alter source intent, authorization, resource limits, operation identity, or store transitions. Existing Substrate client setup precedes this reload, but its refusal path cannot retire a published materialization.

The regression extends the existing HTTP Connector and Unix Substrate recovery fixture. It first publishes Ready, stops both fixture services, and then passes the retained Preparing snapshot through provisioning. Exact returned and durable Ready equality prove that recovery needs no new provider operation. Additional stale replays preserve Closing and Closed (`crates/workspace-service/src/main.rs:6249`). The retained red run fails at the new Ready assertion; the green run passes. This directly tests the stale-snapshot boundary rather than relying on scheduler timing. It does not independently reproduce the entire router interleaving or an actual destructive replay.

Reviewed full `task check` evidence passes formatting, Clippy, 59 Rust tests and 20 publication tests. The reviewer inspected those logs and did not duplicate builds. Whitespace checks pass. Release changes consistently advance the three local packages to 0.2.21; dependencies are unchanged. The SHA256 of the reviewed diff for `crates/workspace-service/src/main.rs`, `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` is `a9e6ecc61c2ede56ca8039e19d9c151cc434e53a857a033badea8da2350d5c94`.

The worker registry remains process-local. Concurrent provisioning by separate service processes is an existing architectural limitation, not solved or introduced by this delayed-read repair. Release CI and deployed browser acceptance remain separate evidence. Recommend accepting this source fix; no blocker prevents the requested single-process recovery behavior. Planning records and publication were outside this read-only review.

```findings
[]
```
