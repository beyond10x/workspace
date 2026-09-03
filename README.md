# Workspace

Workspace is the product-neutral authority for opening source repositories as durable engineering
projects. It combines current Identity and Connector authority with commit-pinned repository
snapshots, personal conversation threads, and workflow admission.

The service never stores a forge credential. Repository discovery is performed through Connectors'
read-only datasource contract, and every project operation revalidates that the current subject can
still see the repository.

## Run locally

```bash
cargo run --locked -p workspace-service -- \
  --identity-origin http://127.0.0.1:8081 \
  --connectors-api-base http://127.0.0.1:8091/api/connectors/v1 \
  --agent-platform-origin http://127.0.0.1:8090 \
  --project-agent-model default \
  --aep-service-origin http://127.0.0.1:8080 \
  --aep-realm engineering \
  --aep-workspace central
```

Use a local Identity session as the HTTP bearer. Hosted deployment supplies PostgreSQL through
`WORKSPACE_DATABASE_URL`; neither the URL nor credentials belong in source control.

Hosted deployments may configure the released Substrate remote client with
`WORKSPACE_SUBSTRATE_ORIGIN`, `WORKSPACE_SUBSTRATE_CA_BUNDLE`, and
`WORKSPACE_SUBSTRATE_SERVER_IDENTITY`. The three values are atomic: Workspace refuses partial
configuration, uses only HTTPS with the explicit trust roots and DNS identity, and obtains a fresh
Identity access credential for audience `urn:b10x:substrate` on each SDK request. Opening,
browsing, or changing a project snapshot then also proves that authenticated Substrate seam.

Coding sessions extend the existing `/v1/projects` authority instead of introducing another file
store. Workspace recursively reads the project's exact pinned GitLab commit through Connectors,
then creates two confined Substrate workspaces: an immutable-by-policy base used as the comparison
and publication authority, and a writable working materialization used by editors and processes.
Workspace persists only opaque Substrate references and the complete source-manifest digest; file
bytes remain in Substrate and forge credentials remain in Connectors. A session becomes `ready`
only after both complete materializations exist. Refusal cleans up both, while an uncertain cleanup
is explicit as `unknown` and never exposes partial references.

The intended development ceilings are 10,000 files, 256 MiB total and 4 MiB per file; effective
bounds are always the lowest limit of Workspace and its dependencies. The current effective bounds
are 1,000 files (Substrate's recursive-tree ceiling), 256 MiB total, and 180 KiB per file
(Connector's operation-result ceiling after base64 and JSON overhead). These are reported explicitly
rather than returning truncated content.

Ready coding sessions expose the same Workspace API to Devcenter, AgentIDE, and other clients:
bounded searchable trees, complete UTF-8 file reads, exact-state Save/create, and canonical
immutable-base-versus-working diffs. A tree says when content was truncated and whether the omitted
count is known. File reads return complete-content SHA-256, size, language hint, and base-relative
modification state; binary files are explicit read-only projections. Save carries either `absent` or
the digest originally loaded by the editor. A stale save returns HTTP 409 with both base and latest
Workspace projections, and no blind-overwrite operation exists.

`POST /v1/sessions/{session_id}/diff` is the authoritative diff resolver. Patch, stat, and
files-only modes are projections of the same server calculation and carry one digest, exact old/new
line numbers, stable hunk ids, file digests, counts, and server-derived attribution. The first
release resolves the `workspace` selector; declared plan, agent-attempt, publication, and revision
pair selectors fail explicitly until their owning services supply immutable references. Browsers do
not compute authoritative diffs.

## First contract

- discover all GitLab projects visible through the subject's current Connections;
- converge users on one tenant/project canonical project;
- select any branch and pin its observed head only through explicit refresh;
- keep personal branch-bound threads and commit-boundary messages durable;
- list the pinned root tree and project root files through exact-commit, read-only Connector
  operations;
- create/list/resume exact-revision coding sessions through the same project API used by Devcenter;
- browse and edit the session's Substrate working materialization through exact, bounded Workspace
  file contracts, and resolve its diff only against the immutable base materialization;
- provision one project agent and dispatch typed conversation turns containing the prior personal
  thread and a bounded exact-commit context pack;
- query the official central AEP authority for entities whose indexed `space` is the canonical
  Workspace project, forwarding only the transient Identity session for fresh verification;
- admit the code review, security review, and reverse AEP + ESS workflow identities against an
  exact branch and commit.

Source bytes and execution remain delegated to Substrate, while Connector tree/file operations are
the governed forge bridge. Workspace composes those existing authorities without asking Substrate
to hold forge credentials or treating mutable Git metadata as truth. Reverse-engineering
publishers associate central artifacts by using the canonical
project id as the AEP locator space and record the exact commit as AEP source provenance.
Governed repository workspaces for agents and workflows

<!-- b10x-docs:start -->
## Documentation

[Workspace documentation](https://beyond10x.github.io/docs/workspace/) · [Start](https://beyond10x.github.io/) · [Ecosystem](https://beyond10x.github.io/ecosystem/) · [Impact](https://beyond10x.github.io/changes/) · [Releases](https://beyond10x.github.io/releases/)
<!-- b10x-docs:end -->
