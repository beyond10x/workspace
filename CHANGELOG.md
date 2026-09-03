# Changelog

## Unreleased

## 0.2.5 - 2026-09-03

- Align the repository toolchain and release image with the declared Rust 1.98 minimum so the
  promoted Workspace runtime is built by the same compiler that passes the source gate.
- Gate the declared minimum, repository toolchain, and release-image Rust versions as one release
  invariant so a source gate cannot pass an image that is guaranteed to fail.

## 0.2.4 - 2026-09-03

- Derive AgentIDE agent views and intent authorization from a verified Agent Platform task, the
  generated AgentIDE Service SDK projections, and the existing Substrate-backed coding session;
  no second file store or caller-supplied actor identity is introduced.
- Dispatch pre-built repository workflows to Agent Platform with exact-snapshot context, durable
  owner-scoped status, idempotent task association, named terminal failures, and persisted Markdown
  results that clients can reload.

## 0.2.3 - 2026-09-03

- Pass nested repository paths to the GitLab Connector as canonical raw paths so exact source
  materialization no longer fails on every file below the repository root.
- Add deployment-declared interactive terminal profiles over the existing Substrate working
  materialization. Workspace verifies the authenticated human's exact AgentIDE
  `interactive_terminal` grant through generated Service SDK projections, persists lifecycle but
  never scrollback, and exposes bounded sequenced WebSocket replay with detach, explicit kill, and
  coding-session cleanup semantics.

## 0.2.2 - 2026-09-03

- Request Substrate scopes in Identity's canonical lexical order so the access-token client
  accepts the successfully minted authority before opening the hosted TLS connection.

## 0.2.1 - 2026-09-03

- Request only Substrate's admitted `observe`, `workspaces`, and `exec` scopes. Session routes are
  governed by `exec`; requesting a separate `session` scope caused hosted project opening to fail.

## 0.2.0 - 2026-09-03

- Add owner-scoped, exact-revision coding sessions under the existing project API. Workspace now
  recursively materializes a Connector-authorized GitLab commit into complete base and working
  Substrate workspaces, preserves executable modes, records a source-manifest digest, and refuses
  or marks uncertain cleanup without exposing partial references.
- Pin the governed GitLab publication Connector and the Substrate executable-mode preservation
  change by exact commit.
- Add bounded searchable coding-session trees, complete file projections, exact digest-based
  Save/create with structured conflicts, and one canonical server-side diff resolver shared by
  Devcenter and AgentIDE clients.

## 0.1.5 - 2026-09-02

- Select the AWS-LC Rustls crypto provider during service startup so Substrate HTTPS clients can
  coexist with Ring-enabled HTTP dependencies without panicking on first use.

## 0.1.4 - 2026-09-02

- Keep private runtime promotion in the existing private Devcenter package instead of publishing a
  package linked to this public source repository.

## 0.1.3 - 2026-09-02

- Bound repository discovery to one provider-side page per GitLab connection and accept an
  optional provider-side search query instead of serially loading every reachable repository.
- Revalidate project access with one exact datasource read when opening or accessing a project,
  preserving the repository's provider-declared default branch.

## 0.1.2 - 2026-09-02

- Admit Identity over plaintext only at internal Kubernetes service DNS when Workspace listens on
  a pod-reachable address, while continuing to reject public plaintext Identity origins.

## 0.1.1 - 2026-09-02

- Forward the transient Identity session to Agent Platform for repository chat so it can derive a
  fresh user-bound Connector model lease instead of receiving an already-narrowed access token.

## 0.1.0 - 2026-09-02

### Added

- Live, grant-derived GitLab repository and branch discovery through Connectors.
- Canonical projects with explicit commit refresh and durable personal threads.
- Exact-snapshot admission for code review, security review, and reverse AEP + ESS workflows.
- An official bounded HTTP client for Devcenter and other products.
- A pinned non-root runtime image build for hosted Workspace deployments.
