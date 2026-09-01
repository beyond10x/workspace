# Workspace

Workspace is the product-neutral authority for opening source repositories as durable engineering
projects. It combines current Identity and Connector authority with commit-pinned repository
snapshots, personal conversation threads, and workflow admission.

The service never stores a forge credential. Repository discovery is performed through Connectors'
read-only datasource contract, and every project operation revalidates that the current subject can
still see the repository.

## Run locally

```console
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

## First contract

- discover all GitLab projects visible through the subject's current Connections;
- converge users on one tenant/project canonical project;
- select any branch and pin its observed head only through explicit refresh;
- keep personal branch-bound threads and commit-boundary messages durable;
- list the pinned root tree and project root files through exact-commit, read-only Connector
  operations;
- provision one project agent and dispatch typed conversation turns containing the prior personal
  thread and a bounded exact-commit context pack;
- query the official central AEP authority for entities whose indexed `space` is the canonical
  Workspace project, forwarding only the transient Identity session for fresh verification;
- admit the code review, security review, and reverse AEP + ESS workflow identities against an
  exact branch and commit.

Full source materialization and workflow execution remain delegated to Substrate and Workflow.
The Connector tree/file projection is the governed browse and chat-context bridge while
Substrate's existing Git source remains explicitly unserved; it is not presented as a materialized
workspace. Reverse-engineering publishers associate central artifacts by using the canonical
project id as the AEP locator space and record the exact commit as AEP source provenance.
Governed repository workspaces for agents and workflows
