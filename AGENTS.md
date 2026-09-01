# AGENTS.md — workspace

## Serves

- **O1 — governed reach.** Repository access is revalidated from Identity and current Connector Grants.
- **O4 — products run on the foundation.** Workspace composes released service seams into repository projects.
- **O5 — the generic agent platform.** Projects provide durable context for agents, chat, and workflows.

## Boundary

This public repository owns canonical repository projects, materialization references, personal
threads, project context leases, and project associations. Connectors owns provider credentials,
Substrate owns confinement, Agent Platform owns agents and tasks, Workflow owns run semantics, and
AEP owns engineering artifacts and decisions.

Tenant and actor are always server-derived. Credential bytes, deployment-specific identifiers,
private connector bundles, hostnames, and image coordinates never enter this repository. Anything
that runs is Rust. Public source remains proprietary unless its license is changed explicitly.

## Gate

```console
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Use managed worktrees. Preserve unrelated work, never commit secrets, and use the organization bot
for automated commits and pushes.
