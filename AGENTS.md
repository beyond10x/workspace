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
task check
```

The task runs formatting, Clippy with warnings denied, and the locked workspace test suite.

Use managed worktrees. Preserve unrelated work, never commit secrets, and use the organization bot
for automated commits and pushes.

<!-- b10x-docs-operations:start -->
## Public documentation operations

This repository owns the public source and presentation allowlist in `b10x.docs.yaml`. The generated credential-free `.github/workflows/b10x-docs-bundle.yml` passively packages only those declared files for the exact successful `main` commit; it must never run repository code. Atlas selects the latest successful bundle with every other catalog source, and Website plus Docs System own rendering, shared components, search, and feeds. Do not add a standalone docs deployer or put App credentials in this public repository. If Atlas catalogs a former Pages workflow, that file remains repository-owned validation: preserve its bespoke checks while keeping exact read-only permissions, an unconditional pull-request trigger, and no deployment primitives. Project Pages at `/workspace/` is only the generated stable redirect façade in `.github/workflows/b10x-docs-pages.yml`; content-only publication never rebuilds it.

From the complete organization workspace, verify the contract with a clean Atlas checkout at the current remote `main`. Set `B10X_ATLAS_CHECKOUT` to a managed Atlas worktree when the primary checkout is dirty or stale; never infer command availability from the primary alone.

```bash
atlas_checkout="${B10X_ATLAS_CHECKOUT:-atlas}"
atlas_head="$(git -C "$atlas_checkout" rev-parse HEAD)"
atlas_main="$(git -C "$atlas_checkout" ls-remote origin refs/heads/main | awk '{print $1}')"
test -z "$(git -C "$atlas_checkout" status --porcelain)"
test "$atlas_head" = "$atlas_main"
cargo run --manifest-path "$atlas_checkout/Cargo.toml" --locked -q -- \
  --store "$atlas_checkout/catalog/store" docs reconcile --workspace . --check
```

Keep internal plans, stories, ADRs, decisions, worklogs, security material, and research out of the public allowlist unless a repository authority explicitly declares them public.
<!-- b10x-docs-operations:end -->
