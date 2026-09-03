#![forbid(unsafe_code)]

//! Provider-neutral repository project and conversation contracts.

use std::collections::BTreeMap;

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

/// Effective source-materialization bounds after applying dependency ceilings.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MaterializationLimits {
    pub max_files: u32,
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
}

/// Durable lifecycle of one confined coding-session materialization.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodingSessionState {
    Preparing,
    Ready,
    Refused,
    Unknown,
    Closing,
    Closed,
}

/// One project coding session backed by an immutable base reference and writable working tree.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodingSession {
    pub id: String,
    pub project_id: String,
    pub source_revision: String,
    pub base_materialization_ref: Option<String>,
    pub working_materialization_ref: Option<String>,
    pub manifest_sha256: Option<String>,
    pub state: CodingSessionState,
    pub failure_code: Option<String>,
    pub limits: MaterializationLimits,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// Deployment-owned description of one interactive Substrate environment.
///
/// The browser selects only `id`. Every executable, argument, environment value, resource bound,
/// workspace posture and network posture is fixed by the deployment before a request arrives.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TerminalProfile {
    /// Stable deployment-local profile identity.
    pub id: String,
    /// Human-readable toolchain label.
    pub label: String,
    /// Visible immutable runtime or toolchain reference.
    pub runtime_ref: String,
    /// Executable launched inside Substrate confinement.
    pub shell: String,
    /// Fixed arguments supplied to the shell.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Fixed working directory. The first hosted release admits `/workspace` only.
    pub working_directory: String,
    /// Fixed environment values from a small, non-secret terminal allowlist.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Workspace access applied by Substrate.
    pub workspace_access: TerminalWorkspaceAccess,
    /// Network posture applied by Substrate. The first hosted release admits `none` only.
    pub network: TerminalNetworkPosture,
    /// Exact process and output bounds.
    pub limits: TerminalLimits,
}

/// Writable workspace posture of one deployment-declared terminal profile.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalWorkspaceAccess {
    ReadOnly,
    ReadWrite,
}

/// Network posture of a hosted terminal.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalNetworkPosture {
    None,
}

/// Deployment-owned execution bounds for a hosted terminal.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TerminalLimits {
    pub timeout_ms: u64,
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub processes: u32,
    pub output_bytes: u64,
    pub input_bytes: u64,
    pub frame_bytes: u64,
    pub queued_frames: u32,
    pub lease_ttl_ms: u64,
}

/// Request to create one confined interactive terminal.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateTerminal {
    /// `AgentIDE` coordination session bound to this exact Workspace session.
    pub agentide_session_id: String,
    /// Current explicit `interactive_terminal` grant to the authenticated human.
    pub authority_grant_id: String,
    /// Deployment-declared profile identity.
    pub profile_id: String,
    /// Initial PTY columns.
    pub columns: u16,
    /// Initial PTY rows.
    pub rows: u16,
    /// Caller idempotency key. Reuse with different intent is a conflict.
    pub idempotency_key: String,
}

/// Durable hosted-terminal lifecycle. Attachment is deliberately not a terminal state: closing a
/// browser pane detaches and leaves a running process alive.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    Preparing,
    Running,
    Exited,
    Terminated,
    Refused,
    Unknown,
}

/// Exit details observed from Substrate.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TerminalExit {
    pub code: Option<i32>,
    pub signal: Option<String>,
}

/// Durable terminal metadata. Raw PTY bytes and attachment credentials are never represented here.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TerminalSession {
    pub id: String,
    pub coding_session_id: String,
    pub agentide_session_id: String,
    pub authority_grant_id: String,
    pub profile: TerminalProfile,
    pub actor: String,
    pub process_id: Option<String>,
    pub state: TerminalState,
    pub exit: Option<TerminalExit>,
    pub failure_code: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// Idempotent request to materialize the project's exact current revision.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateCodingSession {
    pub source_revision: String,
    pub idempotency_key: String,
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

/// One central AEP entity associated with a repository project.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EngineeringArtifact {
    /// Canonical AEP entity identity.
    pub id: String,
    /// Logical AEP address.
    pub locator: String,
    /// Versioned AEP entity type.
    pub entity_type: String,
    /// Current AEP entity revision.
    pub revision: u64,
    /// Human-readable title projected from the entity body when present.
    pub title: Option<String>,
    /// Lifecycle status projected from the entity body when present.
    pub status: Option<String>,
    /// Last AEP update time in Unix milliseconds.
    pub updated_at_ms: u64,
    /// Exact source revision recorded by AEP provenance when present.
    pub source_revision: Option<String>,
}

/// Bounded project projection from the central AEP authority.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EngineeringArtifactPage {
    /// Entities whose indexed AEP space is this Workspace project.
    pub artifacts: Vec<EngineeringArtifact>,
    /// Whether another central page exists beyond this projection.
    pub has_more: bool,
}

/// One entry in a bounded working-materialization tree.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodingTreeEntry {
    pub path: String,
    pub kind: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

/// Bounded searchable project tree with explicit partial-result state.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodingTreeProjection {
    pub format: String,
    pub entries: Vec<CodingTreeEntry>,
    pub truncated: bool,
    pub omitted: Option<u64>,
}

/// Relationship between a working file and the immutable session base.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileModificationState {
    Unchanged,
    Added,
    Modified,
}

/// Complete file identity returned by Workspace, not by browser inference.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileRevision {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub language: Option<String>,
    pub modification: FileModificationState,
}

/// One complete editable UTF-8 file or an explicit binary refusal projection.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileProjection {
    pub format: String,
    pub revision: FileRevision,
    pub content: Option<String>,
    pub binary: bool,
    pub truncated: bool,
}

/// Expected destination state for one exact reversible write.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileExpectedState {
    Absent,
    Sha256 { sha256: String },
}

/// Exact content replacement used for both Save and create.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WriteFile {
    pub content: String,
    pub expected: FileExpectedState,
    pub create_parents: bool,
    pub operation_id: String,
}

/// Authoritative source selector for one canonical Workspace diff.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChangeSelector {
    Workspace,
    Plan { digest: String },
    AgentAttempt { attempt_id: String },
    Publication { publication_id: String },
    RevisionPair { old: String, new: String },
}

/// Amount of canonical diff detail requested.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiffMode {
    Patch,
    Stat,
    FilesOnly,
}

/// Request to resolve an authoritative diff selector.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResolveDiff {
    pub selector: ChangeSelector,
    pub mode: DiffMode,
}

/// One old or new range in a canonical hunk.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiffRange {
    pub start: u64,
    pub lines: u64,
}

/// One canonical text-diff line.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiffLine {
    pub kind: String,
    pub old_line: Option<u64>,
    pub new_line: Option<u64>,
    pub content: String,
}

/// One stable canonical diff hunk.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiffHunk {
    pub id: String,
    pub old: DiffRange,
    pub new: DiffRange,
    pub heading: Option<String>,
    pub lines: Vec<DiffLine>,
}

/// One changed file in the canonical projection.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiffFile {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub status: String,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub old_sha256: Option<String>,
    pub new_sha256: Option<String>,
    pub hunks: Vec<DiffHunk>,
    pub attribution: Vec<String>,
}

/// Canonical server-resolved diff consumed unchanged by every renderer and approval link.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiffProjection {
    pub format: String,
    pub selector: ChangeSelector,
    pub mode: DiffMode,
    pub digest: String,
    pub source_revision: String,
    pub files: Vec<DiffFile>,
    pub additions: u64,
    pub deletions: u64,
    pub partial: bool,
}

/// Conflict payload used by an editor to render base, local draft, and latest content.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileConflict {
    pub code: String,
    pub base: Option<FileProjection>,
    pub latest: FileProjection,
}

/// Safe HTTP problem document.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Problem {
    /// Stable refusal or failure code.
    pub code: String,
}
