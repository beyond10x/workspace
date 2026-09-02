# Changelog

## Unreleased

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
