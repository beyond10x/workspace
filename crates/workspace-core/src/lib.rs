#![forbid(unsafe_code)]

//! Provider-neutral repository project and conversation contracts.

use serde::{Deserialize, Serialize};

/// A repository visible through one current Connector grant.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositoryCandidate {
    /// Opaque forge-instance identity supplied by Connectors.
    pub forge_instance_ref: String,
    /// Opaque provider project identity.
    pub project_ref: String,
    /// Human-readable repository namespace and name.
    pub path_with_namespace: String,
    /// Human-readable repository name.
    pub name: String,
    /// Provider-declared default branch, when one exists.
    pub default_branch: Option<String>,
    /// Provider visibility label.
    pub visibility: String,
    /// Provider web address for human navigation.
    pub web_url: String,
    /// Canonical project already associated with this repository.
    pub opened_project_id: Option<String>,
}

/// Request to open a repository after live discovery revalidation.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OpenProject {
    /// Opaque forge-instance identity returned by discovery.
    pub forge_instance_ref: String,
    /// Opaque provider project identity returned by discovery.
    pub project_ref: String,
}

/// One canonical shared repository project.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// Workspace-owned canonical project identity.
    pub id: String,
    /// Opaque forge-instance identity.
    pub forge_instance_ref: String,
    /// Opaque provider project identity.
    pub project_ref: String,
    /// Repository namespace and name.
    pub path_with_namespace: String,
    /// Repository name.
    pub name: String,
    /// Provider-declared default branch.
    pub default_branch: Option<String>,
    /// Currently selected branch for this representation.
    pub selected_branch: String,
    /// Exact pinned commit, absent until branch materialization succeeds.
    pub pinned_commit: Option<String>,
    /// Whether `main` was absent and the provider default was used.
    pub default_branch_fallback: bool,
    /// Provider web address.
    pub web_url: String,
}

/// A selectable branch pinned to the head observed by Connectors.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Branch {
    /// Branch name.
    pub name: String,
    /// Exact head commit identifier.
    pub commit: String,
    /// Whether the provider marks this as its default branch.
    pub provider_default: bool,
    /// Whether the provider protects this branch.
    pub protected: bool,
}

/// Explicitly select or refresh one branch head.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SelectBranch {
    /// Branch to pin.
    pub branch: String,
}

/// Kind of one entry in an exact repository snapshot.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryEntryKind {
    Blob,
    Tree,
}

/// One read-only repository tree entry at the project's pinned commit.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositoryEntry {
    pub object_id: String,
    pub name: String,
    pub path: String,
    pub kind: RepositoryEntryKind,
    pub mode: String,
}

/// One personal, branch-bound conversation thread.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Thread {
    /// Workspace-owned thread identity.
    pub id: String,
    /// Canonical project identity.
    pub project_id: String,
    /// Branch this thread follows through explicit refreshes.
    pub branch: String,
    /// Commit at the latest boundary.
    pub pinned_commit: String,
    /// Human-readable thread title.
    pub title: String,
    /// Creation time in Unix milliseconds.
    pub created_at_ms: u64,
}

/// Request to create a personal project thread.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateThread {
    /// Branch selected for the thread.
    pub branch: String,
    /// Exact commit selected for the thread.
    pub pinned_commit: String,
    /// Human-readable title.
    pub title: String,
}

/// Role of one durable conversation message.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// Human-authored input.
    User,
    /// Agent-authored output.
    Assistant,
    /// Commit-boundary or refusal notice.
    System,
}

/// One durable, commit-attributed conversation message.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Message {
    /// Monotonic per-thread sequence.
    pub sequence: u64,
    /// Message role.
    pub role: MessageRole,
    /// Bounded message content.
    pub content: String,
    /// Branch observed by the message.
    pub branch: String,
    /// Exact commit observed by the message.
    pub commit: String,
    /// Creation time in Unix milliseconds.
    pub created_at_ms: u64,
}

/// Request to append a user message.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateMessage {
    /// Bounded human message content.
    pub content: String,
}

/// Immutable platform workflow definition exposed for a project.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDefinition {
    /// Stable definition identity.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Evidence produced by the workflow.
    pub description: String,
}

/// Durable workflow-run state.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunState {
    /// Run admitted and waiting for dispatch.
    Accepted,
    /// Run currently executing.
    Running,
    /// Run and all artifact writes converged.
    Succeeded,
    /// Run failed with a named reason.
    Failed,
    /// Current authority refused the run.
    Refused,
}

/// One commit-pinned workflow run.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRun {
    /// Workspace-owned run identity.
    pub id: String,
    /// Stable workflow definition identity.
    pub definition_id: String,
    /// Canonical project identity.
    pub project_id: String,
    /// Selected branch.
    pub branch: String,
    /// Exact commit analyzed by the run.
    pub commit: String,
    /// Current durable state.
    pub state: WorkflowRunState,
    /// Safe refusal or failure code.
    pub failure_code: Option<String>,
    /// Creation time in Unix milliseconds.
    pub created_at_ms: u64,
}

/// Request to start one pre-built workflow at an exact project snapshot.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StartWorkflow {
    /// Stable workflow definition identity.
    pub definition_id: String,
    /// Selected branch.
    pub branch: String,
    /// Exact commit to analyze.
    pub commit: String,
    /// Caller idempotency key.
    pub idempotency_key: String,
}

/// Safe HTTP problem document.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Problem {
    /// Stable refusal or failure code.
    pub code: String,
}
