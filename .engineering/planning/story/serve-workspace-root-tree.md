---
format: aep.planning-md/1
id: story:serve-workspace-root-tree
kind: story
status: active
title: Serve the bounded workspace root tree through the released observation contract
relations:
- informed_by: story:preserve-ready-materialization
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: Cargo.lock
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: crates/workspace-service/src/main.rs
revision: 5
---
## Outcome

Opening a ready coding workspace lists its top-level files and folders, supports bounded pages, and retains ordinary child-directory reads. This composes the released Substrate tree observation; it introduces no foundation route, authority, resource or wire bundle.

## Evidence and boundary

After the ready-session lifecycle repair, authenticated file and terminal metadata reads succeed, but the empty-root tree query returns substrate_protocol_invalid. Workspace passes the empty root into SDK read_directory, whose relative-path validation deliberately rejects empty strings. The released foundation exposes a bounded recursive tree observation instead. SOURCE_MATERIALIZATION_INODES and MaterializationLimits.max_files are both 1000, explicitly ensuring an admitted workspace can be fully enumerated within that observation bound (crates/workspace-service/src/main.rs).

For the empty root, read that existing bounded tree with hidden entries included, select only immediate children, and produce the existing coding-tree/2 projection. Paginate those children with an opaque cursor bound to the materialization and observed root listing. Reject malformed, foreign or stale cursors. A truncated foundation tree cannot prove a complete root listing and must return a named refusal; never manufacture a complete page from it. Nonempty directory paths retain the native directory cursor and SDK read_directory path. Preserve current Identity, grant and ready-session checks, all materialization limits, strict relative-path validation and file-write semantics.

## Acceptance

A real SDK transport regression fails the original empty-root call and proves first/next root pages, nested entry exclusion, hidden child visibility and unchanged nested directory forwarding. Pure projection regressions cover empty roots, page boundaries, changed and foreign snapshots, malformed cursors, and explicit refusal of truncated observations. The full task check, independent review and immutable Workspace release precede downstream headless explorer/read/edit/save/restore/close acceptance.

The existing 1000-entry materialization ceiling remains a product bound. This repair uses a bounded recursive observation for each root page; introducing a direct foundation root-page route is separate capability work and is not required to serve this admitted bounded materialization profile. This is one bounded repair, so no multi-item decomposition panel applies.

## Verification

The real SDK transport regression fails the original empty-root read with HTTP502 before a tree request can be sent. With the correction it observes both root pages through the exact released v2 tree route, includes hidden root children, excludes nested descendants, and forwards the native child-directory cursor unchanged. The existing ambiguous-create recovery and stale-preparing replay assertions remain green in the same transport fixture. Pure projection checks reject malformed, foreign, changed, oversized and truncated observations and cover an empty root and deterministic root ordering.

The full task check passes formatting, workspace Clippy with warnings denied, all61 Rust cases and all20 publication policy/adversary cases. Runtime0.2.22 advances only the three local packages; external dependencies and the Substrate contract are unchanged. A direct authenticated observation using the already released bounded tree returned417 entries, six root children and no truncation. Independent review, immutable publication and downstream browser acceptance follow.
