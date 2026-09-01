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
  --connectors-api-base http://127.0.0.1:8091/api/connectors/v1
```

Use a local Identity session as the HTTP bearer. Hosted deployment supplies PostgreSQL through
`WORKSPACE_DATABASE_URL`; neither the URL nor credentials belong in source control.

## First contract

- discover all GitLab projects visible through the subject's current Connections;
- converge users on one tenant/project canonical project;
- select any branch and pin its observed head only through explicit refresh;
- keep personal branch-bound threads and commit-boundary messages durable;
- admit the code review, security review, and reverse AEP + ESS workflow identities against an
  exact branch and commit.

Source materialization, agent task dispatch, and workflow execution remain delegated to Substrate,
Agent Platform, and Workflow respectively.
Governed repository workspaces for agents and workflows
