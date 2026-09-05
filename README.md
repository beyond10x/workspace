# Workspace

Workspace is the product-neutral authority for opening source repositories as durable engineering
projects. It combines current Identity and Connector authority with commit-pinned repository
snapshots, personal conversation threads, and workflow admission.

The service never stores a forge credential. Repository discovery is performed through Connectors'
read-only datasource contract, and every project operation revalidates that the current subject can
still see the repository.

## Independent runtime publication

Workspace publishes its own service image from an exact version tag on the repository's default
branch. Native AMD64 and ARM64 images must serve both health and database readiness probes before
publication. The owner workflow signs and verifies the composed immutable image digest, then records
it as `artifacts.workspace_service` in the release's `release-manifest.json`, alongside the source
commit and both platform digests. Deployment composition consumes that manifest without rebuilding
Workspace or running Devcenter's release pipeline.

The registry target is configured through the repository's `WORKSPACE_IMAGE` variable. Existing
packages must be private before any publication; a confirmed missing GHCR package uses GHCR's
default private initial visibility and is checked again after publication. CI never changes package
visibility. Recovery dispatches require the same exact tag and source commit. A successful matching
release returns without allocating image builds. Drafts with an uploaded receipt finish from that
receipt and also skip image builds. Both paths verify that the private package, all recorded image
digests, the index's platform membership, and the owner's signature still exist; neither replaces
an existing manifest. Image build credentials only admit the source dependencies. No cluster
operation participates in publication, and no deployment coordinates are embedded in the manifest.

For a first package, a scoped CI token's `404` does not prove absence. An administrator must verify
the exact target is absent, temporarily set `WORKSPACE_BOOTSTRAP_SOURCE` to the configured image
followed by `@` and the exact release source commit, and remove that attestation after the first
successful private-package check. It admits only that initial image/source pair; it never bypasses
the post-publication privacy check or permits an existing release receipt to use a missing package.

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

Interactive terminals are another projection of the same ready coding session; they do not create
a filesystem or shell beside Workspace. `WORKSPACE_TERMINAL_PROFILES_PATH` may name a bounded JSON
array of deployment-declared profiles such as
[`terminal-profiles.example.json`](terminal-profiles.example.json). With no profile file, terminal
creation is explicitly unavailable. A profile fixes the visible runtime/toolchain reference,
absolute shell and arguments, `/workspace` working directory, sanitized non-secret environment,
read-only or read-write access to the existing working materialization, no-network posture, and
Substrate resource/lease bounds. Request bodies can select a profile but cannot add a command,
environment variable, network route, mount, or credential.

Workspace admits `POST /v1/sessions/{session_id}/terminals` only after it uses Connectors to read
the AgentIDE generated Service SDK projections and verifies an active `interactive_terminal` grant
for the authenticated human, exact AgentIDE/Workspace session binding, project, source revision,
and project-root path. Attach revalidates the grant so expiry or revocation takes effect before a
reconnect. `GET /v1/terminals/{terminal_id}/attach` upgrades to WebSocket: browser binary frames
are PTY input, server binary frames begin with an eight-byte big-endian monotonic sequence followed
by output, and JSON frames carry resize, signal, replay, lifecycle, refusal, and exit details.
Workspace retains only a 4 MiB process-local replay ring; durable storage contains lifecycle and
opaque Substrate references, never scrollback. Closing a socket detaches. Only explicit terminal
termination or coding-session close kills and retires the Substrate process.

The PTY is always created through the configured remote Substrate client against the coding
session's existing working materialization. It is never a Workspace, Devcenter, or host shell;
terminal profiles cannot enable ambient credentials or undeclared network access.

### Isolated terminal lab

`workspace-terminal-lab` is a loopback-only development binary for testing Devcenter's Ghostty
renderer against the production Workspace replay/broker primitives and a real Substrate daemon,
without bringing up Identity, Connectors, AgentIDE, or a forge. The lab creates an ephemeral
Substrate workspace containing a small `README.md` and `Cargo.toml`, starts `/bin/sh` in a
probe-verified confined PTY with no network, and serves only a health route and the terminal
WebSocket used by Devcenter review mode. It refuses non-loopback listeners and an environment that
cannot serve Substrate's `sessions.pty` capability.

Build `substrate-daemon` from the exact Substrate revision pinned by this repository, place the lab
inside a delegated cgroup carrying `cpu`, `memory`, and `pids`, and run:

```bash
cargo run --locked -p workspace-service --bin workspace-terminal-lab -- \
  --substrate-daemon "$SUBSTRATE_DAEMON" \
  --cgroup-root "$SUBSTRATE_DELEGATED_CGROUP"
```

Then start Devcenter review mode with
`DEVCENTER_REVIEW_TERMINAL_UPSTREAM=ws://127.0.0.1:8095`. The rest of the project workbench remains
sample review data; only the terminal transport is real. The production Workspace service never
enables this path and still requires authenticated session binding and an exact
`interactive_terminal` grant.

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
- open, reconnect to, detach from, and explicitly terminate profile-bound Substrate PTYs only under
  a current human `interactive_terminal` grant;
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
