---
format: aep.planning-md/3
id: story:page-workspace-root-through-substrate-root-listing
kind: story
status: draft
title: Page the workspace root through Substrate's root listing
summary: A truncated recursive tree answers 503 coding_tree_root_incomplete; Substrate will page the root as path . on the directory page
relations:
- depends_on: story:substrate-current-contract-paired-move
- informed_by: story:serve-workspace-root-tree
revision: 1
---
## Outcome

Opening the root of a coding workspace returns pages of its immediate children whatever the size of
the tree beneath them. A truncated recursive tree no longer makes the root unreadable.

## What was seen

- read: in `crates/workspace-service/src/main.rs`, `coding_directory_tree` (`:2144`) reads an empty
  path through the recursive tree (`:2151` to `:2158`), because the directory reader of the pinned
  SDK accepts only a non-empty relative path. `root_tree_page` (`:2197`) answers 503
  `coding_tree_root_incomplete` when that tree is truncated (`:2203` to `:2208`).
- read: the materialization ceiling is 1,000 files and 1,000 inodes (`:83`, `:91`), and
  `README.md:72` gives 1,000 files as the effective bound, Substrate's recursive-tree ceiling.
- run by the review, not repeated here: in a disposable workspace with one top-level directory and
  1,000 files inside it, the released Substrate 0.24.0 daemon returned 1,000 entries and marked the
  tree truncated. Workspace's answer to that flag was traced in source, not run.
- read: `story:serve-workspace-root-tree` (implemented) built the root page on the recursive tree
  and calls a direct root-page route in Substrate "separate capability work".
  `review-result:root-tree-pass-1` notes that every root page repeats the recursive observation.
- stated by the substrate session, not read in a released Substrate: the operator decided on
  2026-11-23 that Substrate's root listing is the path `.` on the existing directory page,
  `GET /v1/workspaces/{id}/files/.?mode=directory` with `cursor` and `limit_items`. It stays inside
  the published `substrate-wire/0.27.0` schemas, so there is no successor bundle. A daemon that
  does not serve it answers 422 `workspace.path-escape`. A Substrate ADR that records this is being
  written, and Substrate's story is `story:root-directory-listing-pages-immediate-children` in
  `beyond10x/substrate`.

## What changes

For the empty root Workspace reads Substrate's paged root listing instead of the recursive tree,
once a Substrate release serves it and Workspace is on that contract
(`story:substrate-current-contract-paired-move`). What Workspace answers to a daemon that still
refuses the root path is the owner's decision.

## Acceptance

Against a Substrate daemon that serves the root listing, a workspace with one top-level directory
holding 1,000 files lists its root in pages and never answers `coding_tree_root_incomplete`.

## Where the requirement comes from

A review of the Substrate release 0.24.0 dated 2026-11-23, finding 2 (its consumer side), which the
operator handed to the substrate session on 2026-11-23 with the order to address every finding.
Filed by the substrate session. Stakeholder: the operator.
