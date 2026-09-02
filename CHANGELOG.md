# Changelog

## Unreleased

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
