#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aep_client::AepClient;
use aep_contract::query::{EntityQuery, QueryService};
use agent_platform_client::{AgentPlatformClient, ClientError as AgentPlatformClientError};
use agent_platform_core::{
    ActivateRevision, AgentId, ConversationInput, ConversationMessage, ConversationRole,
    CreateAgent, ProjectContext, ProjectContextFile, RevisionSpec, SubmitTask, Task, TaskId,
    TaskStatus,
};
use agentide_contracts::{
    ActorContext, ActorKind, ActorView, ActorWorkbench, AttachmentProvenance, AuthorityGrant,
    ChangeSelector as AgentIdeChangeSelector, ContextPack, ContextRecord, ContextSelection,
    CoordinationRevision, IntentProfile, OpenFileReference, Risk as AgentIdeRisk, SelectionKind,
    TerminalSession as AgentIdeTerminalSession, TerminalState as AgentIdeTerminalState,
    authorize_intent, canonical_json_sha256, resolve_intent_inventory,
};
use anyhow::{Context, Result, bail};
use axum::Router;
use axum::body::Body;
use axum::extract::ws::{Message as WebSocketMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, serve};
use b10x_substrate_sdk::{
    AccessToken, Client as SubstrateClient, ExecutionPolicy, ExpectedFileState, PipeChannel,
    PipeFrame, PipeSessionState, PtyWindow, RefusalClass as SubstrateRefusalClass,
    SdkError as SubstrateError, Signal, Workspace as SubstrateWorkspace, WorkspaceAccess,
};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use clap::Parser;
use connectors_client::HostedClient;
use connectors_client::datasource::{
    BindingSearchRequest, DatasourceRead, DatasourceRequest, DatasourceResult, DescribeRequest,
    ReadRequest,
};
use connectors_client::operation::{self, OwnerContext};
use futures_util::{StreamExt as _, TryStreamExt as _, stream};
use identity_client::{IdentityClient, SessionAuthority};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use similar::{ChangeTag, TextDiff};
use tokio::sync::Mutex;
use workspace_core::{
    Branch, ChangeSelector, CodingActorViewRequest, CodingIntentInvocation, CodingIntentResult,
    CodingSession, CodingSessionState, CodingTreeEntry, CodingTreeProjection, CreateCodingSession,
    CreateMessage, CreateTerminal, CreateThread, DiffFile, DiffHunk, DiffLine, DiffMode,
    DiffProjection, DiffRange, EngineeringArtifact, EngineeringArtifactPage, FileConflict,
    FileExpectedState, FileModificationState, FileProjection, FileRevision, MaterializationLimits,
    MessageRole, OpenProject, Problem, Project, RepositoryCandidate, RepositoryEntry,
    RepositoryEntryKind, ResolveDiff, SelectBranch, StartWorkflow, TerminalExit, TerminalProfile,
    TerminalSession, TerminalState, TerminalWorkspaceAccess, WorkflowDefinition, WorkflowRunState,
    WriteFile,
};

mod aep;
mod store;

use aep::{AepTransport, RequestCredential};
use store::{SessionReservation, Store, StoreError, StoredTerminal, TerminalReservation};
use workspace_service::terminal::{
    TerminalBroker, TerminalBrokerCommand, TerminalBrokerEvent, TerminalBrokers, TerminalProfiles,
    TerminalReplayHub,
};

const CONNECTORS_AUDIENCE: &str = "urn:b10x:connectors";
const CONNECTORS_SCOPE: &str = "connectors.catalog.read connectors.invoke";
const SUBSTRATE_AUDIENCE: &str = "urn:b10x:substrate";
// Identity canonicalizes admitted scopes lexically. Keep this exact order because the client
// rejects a minted authority whose returned scope string differs from the requested scope.
// Substrate's session routes are governed by `exec`; `session` is not a Substrate scope.
const SUBSTRATE_SCOPE: &str = "exec observe workspaces";
const SOURCE_MATERIALIZATION_LIMITS: MaterializationLimits = MaterializationLimits {
    max_files: b10x_substrate_sdk::MAX_LIST_ITEMS,
    max_total_bytes: 256 * 1024 * 1024,
    // Connector operation results are capped at 256 KiB and file bytes are base64 plus JSON.
    max_file_bytes: 180 * 1024,
};
const MAX_SOURCE_DIRECTORIES: usize = 4_096;
const SOURCE_FETCH_CONCURRENCY: usize = 16;
const MATERIALIZATION_UPLOAD_CONCURRENCY: usize = 8;
const MAX_MATERIALIZATION_KEY_BYTES: usize = 256;
const MAX_CONTEXT_SELECTIONS: usize = 8;
const MAX_CONTEXT_SELECTION_BYTES: usize = 32 * 1024;
const MAX_CONTEXT_TOTAL_BYTES: usize = 64 * 1024;
const MAX_CONTEXT_OPEN_FILES: usize = 64;
const MAX_AGENTIDE_SERVICE_ROWS: usize = 10_000;
const CODING_AGENT_INTENTS: [&str; 5] = [
    "code_read",
    "code_changes",
    "code_edit",
    "code_create",
    "terminal_list",
];
const PROJECT_CONTEXT_FILES: &[&str] = &[
    "AGENTS.md",
    "README.md",
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
];

#[derive(Debug, Parser)]
#[command(version, about = "Serve governed repository workspaces")]
struct Args {
    #[arg(long, env = "WORKSPACE_LISTEN", default_value = "127.0.0.1:8094")]
    listen: SocketAddr,
    #[arg(long, env = "WORKSPACE_IDENTITY_ORIGIN")]
    identity_origin: String,
    #[arg(
        long,
        env = "WORKSPACE_IDENTITY_AUDIENCE",
        default_value = "urn:b10x:workspace"
    )]
    identity_audience: String,
    #[arg(long, env = "WORKSPACE_CONNECTORS_API_BASE")]
    connectors_api_base: String,
    #[arg(long, env = "WORKSPACE_AGENT_PLATFORM_ORIGIN")]
    agent_platform_origin: Option<String>,
    #[arg(long, env = "WORKSPACE_PROJECT_AGENT_MODEL")]
    project_agent_model: Option<String>,
    #[arg(long, env = "WORKSPACE_AEP_SERVICE_ORIGIN")]
    aep_service_origin: Option<String>,
    #[arg(long, env = "WORKSPACE_AEP_REALM")]
    aep_realm: Option<String>,
    #[arg(long, env = "WORKSPACE_AEP_WORKSPACE")]
    aep_workspace: Option<String>,
    #[arg(long, env = "WORKSPACE_SUBSTRATE_ORIGIN")]
    substrate_origin: Option<String>,
    #[arg(long, env = "WORKSPACE_SUBSTRATE_CA_BUNDLE")]
    substrate_ca_bundle: Option<String>,
    #[arg(long, env = "WORKSPACE_SUBSTRATE_SERVER_IDENTITY")]
    substrate_server_identity: Option<String>,
    /// Bounded JSON array of deployment-declared interactive terminal profiles.
    #[arg(long, env = "WORKSPACE_TERMINAL_PROFILES_PATH")]
    terminal_profiles_path: Option<PathBuf>,
    #[arg(
        long,
        env = "WORKSPACE_DATABASE_URL",
        default_value = "sqlite://workspace.sqlite?mode=rwc"
    )]
    database_url: String,
}

#[derive(Clone)]
struct AppState {
    identity: IdentityClient,
    connectors: HostedClient,
    agent_platform: Option<AgentPlatformClient>,
    project_agent_model: Option<String>,
    aep: Option<AepConfiguration>,
    substrate: Option<SubstrateConfiguration>,
    terminal_profiles: TerminalProfiles,
    terminal_brokers: TerminalBrokers,
    terminal_replay: TerminalReplayHub,
    materialization_workers: MaterializationWorkers,
    workflow_observers: WorkflowObservers,
    store: Store,
}

#[derive(Clone, Default)]
struct MaterializationWorkers {
    active: Arc<Mutex<BTreeSet<String>>>,
}

impl MaterializationWorkers {
    async fn is_active(&self, session_id: &str) -> bool {
        self.active.lock().await.contains(session_id)
    }

    async fn begin(&self, session_id: &str) -> bool {
        self.active.lock().await.insert(session_id.to_owned())
    }

    async fn finish(&self, session_id: &str) {
        self.active.lock().await.remove(session_id);
    }
}

#[derive(Clone, Default)]
struct WorkflowObservers {
    active: Arc<Mutex<BTreeSet<String>>>,
}

impl WorkflowObservers {
    async fn begin(&self, run_id: &str) -> bool {
        self.active.lock().await.insert(run_id.to_owned())
    }

    async fn finish(&self, run_id: &str) {
        self.active.lock().await.remove(run_id);
    }
}

#[derive(Clone)]
struct AepConfiguration {
    transport: AepTransport,
    realm: String,
    workspace: String,
}

#[derive(Clone)]
struct SubstrateConfiguration {
    origin: String,
    ca_bundle: String,
    server_identity: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    install_crypto_provider()?;
    let args = Args::parse();
    validate_identity_transport(args.listen, &args.identity_origin)?;
    let identity = IdentityClient::new(&args.identity_origin, &args.identity_audience)
        .context("invalid Identity configuration")?;
    let connectors =
        HostedClient::new(&args.connectors_api_base).context("invalid Connectors configuration")?;
    if args.agent_platform_origin.is_some() != args.project_agent_model.is_some() {
        bail!("Agent Platform origin and project agent model must be configured together");
    }
    let agent_platform = args
        .agent_platform_origin
        .as_deref()
        .map(AgentPlatformClient::new)
        .transpose()
        .context("invalid Agent Platform configuration")?;
    let aep_values = [
        args.aep_service_origin.is_some(),
        args.aep_realm.is_some(),
        args.aep_workspace.is_some(),
    ];
    if aep_values.iter().any(|configured| *configured)
        && !aep_values.iter().all(|configured| *configured)
    {
        bail!("AEP Service origin, realm and workspace must be configured together");
    }
    let aep = match (
        args.aep_service_origin.as_deref(),
        args.aep_realm,
        args.aep_workspace,
    ) {
        (Some(origin), Some(realm), Some(workspace)) => Some(AepConfiguration {
            transport: AepTransport::new(origin).context("invalid AEP Service configuration")?,
            realm,
            workspace,
        }),
        _ => None,
    };
    let substrate_values = [
        args.substrate_origin.is_some(),
        args.substrate_ca_bundle.is_some(),
        args.substrate_server_identity.is_some(),
    ];
    if substrate_values.iter().any(|configured| *configured)
        && !substrate_values.iter().all(|configured| *configured)
    {
        bail!("Substrate origin, CA bundle and server identity must be configured together");
    }
    let substrate = match (
        args.substrate_origin,
        args.substrate_ca_bundle,
        args.substrate_server_identity,
    ) {
        (Some(origin), Some(ca_bundle), Some(server_identity)) => Some(SubstrateConfiguration {
            origin,
            ca_bundle,
            server_identity,
        }),
        _ => None,
    };
    let store =
        Store::connect_lazy(&args.database_url).context("invalid database configuration")?;
    let terminal_profiles = TerminalProfiles::load(args.terminal_profiles_path.as_deref())
        .map_err(|error| anyhow::anyhow!(error))?;
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("cannot bind {}", args.listen))?;
    serve(
        listener,
        router(AppState {
            identity,
            connectors,
            agent_platform,
            project_agent_model: args.project_agent_model,
            aep,
            substrate,
            terminal_profiles,
            terminal_brokers: TerminalBrokers::default(),
            terminal_replay: TerminalReplayHub::default(),
            materialization_workers: MaterializationWorkers::default(),
            workflow_observers: WorkflowObservers::default(),
            store,
        }),
    )
    .with_graceful_shutdown(shutdown())
    .await
    .context("Workspace HTTP server failed")?;
    Ok(())
}

fn install_crypto_provider() -> Result<()> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("a Rustls crypto provider was already installed"))
}

fn validate_identity_transport(listen: SocketAddr, identity_origin: &str) -> Result<()> {
    let internal_cluster_http = url::Url::parse(identity_origin).is_ok_and(|origin| {
        origin.scheme() == "http"
            && origin
                .host_str()
                .is_some_and(|host| host.ends_with(".svc.cluster.local"))
    });
    if listen.ip().is_unspecified()
        && identity_origin.starts_with("http://")
        && !internal_cluster_http
    {
        bail!(
            "an HTTP Identity origin is allowed only with a non-public listener or internal cluster DNS"
        );
    }
    Ok(())
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/v1/repositories", get(repositories))
        .route("/v1/projects", post(open_project))
        .route("/v1/projects/{project_id}", get(project))
        .route("/v1/projects/{project_id}/branches", get(branches))
        .route("/v1/projects/{project_id}/tree", get(repository_tree))
        .route("/v1/projects/{project_id}/branch", post(select_branch))
        .route(
            "/v1/projects/{project_id}/sessions",
            get(coding_sessions).post(create_coding_session),
        )
        .route(
            "/v1/sessions/{session_id}",
            get(coding_session).delete(close_coding_session),
        )
        .route(
            "/v1/sessions/{session_id}/actor-view",
            post(coding_actor_view),
        )
        .route(
            "/v1/sessions/{session_id}/intents",
            post(invoke_coding_intent),
        )
        .route("/v1/sessions/{session_id}/tree", get(coding_tree))
        .route("/v1/sessions/{session_id}/diff", post(resolve_diff))
        .route(
            "/v1/sessions/{session_id}/terminals",
            get(list_terminals).post(create_terminal),
        )
        .route(
            "/v1/sessions/{session_id}/terminal-profiles",
            get(list_terminal_profiles),
        )
        .route(
            "/v1/terminals/{terminal_id}",
            get(get_terminal).delete(terminate_terminal),
        )
        .route("/v1/terminals/{terminal_id}/attach", get(attach_terminal))
        .route(
            "/v1/sessions/{session_id}/files/{*path}",
            get(coding_file).put(write_coding_file),
        )
        .route(
            "/v1/projects/{project_id}/threads",
            get(threads).post(create_thread),
        )
        .route(
            "/v1/threads/{thread_id}/messages",
            get(messages).post(create_message),
        )
        .route(
            "/v1/threads/{thread_id}/messages/{message_sequence}/events",
            get(message_events),
        )
        .route("/v1/projects/{project_id}/workflows", get(workflows))
        .route(
            "/v1/projects/{project_id}/engineering-artifacts",
            get(engineering_artifacts),
        )
        .route(
            "/v1/projects/{project_id}/workflow-runs",
            get(workflow_runs).post(start_workflow),
        )
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(serde_json::json!({"status":"ok"}))
}

async fn ready(State(state): State<AppState>) -> Response {
    match state.store.ready().await {
        Ok(()) => Json(serde_json::json!({"status":"ready"})).into_response(),
        Err(_) => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "workspace_store_unavailable",
        ),
    }
}

#[derive(Debug, Deserialize)]
struct AgentIdeSessionRow {
    session_id: String,
    workspace_root: String,
    objective: String,
    project_id: Option<String>,
    source_revision: Option<String>,
    workspace_session_id: Option<String>,
    manifest_digest: Option<String>,
    owner: String,
    state: String,
}

#[derive(Debug, Deserialize)]
struct AgentIdeGrantRow {
    grant_id: String,
    session_id: String,
    grantee: String,
    allowed_intents: Vec<String>,
    path_prefixes: Vec<String>,
    maximum_risk: String,
    expires_at: Option<String>,
    revision: i64,
    owner: String,
    state: String,
}

#[derive(Debug, Deserialize)]
struct AgentIdePinRow {
    pin_id: String,
    session_id: String,
    kind: String,
    reference: String,
    start_line: Option<i64>,
    end_line: Option<i64>,
    sha256: String,
    owner: String,
    state: String,
}

struct VerifiedCodingTurn {
    actor: ActorContext,
    objective: String,
    focused_selections: Vec<ContextSelection>,
    open_files: Vec<OpenFileReference>,
    active_diff: Option<AgentIdeChangeSelector>,
}

#[derive(Debug, Deserialize)]
struct CodingSessionTurnContext {
    workspace_session_id: String,
    agentide_session_id: String,
    #[serde(default)]
    focused_selections: Vec<ContextSelection>,
    #[serde(default)]
    open_files: Vec<OpenFileReference>,
    active_diff: Option<AgentIdeChangeSelector>,
}

async fn agentide_service_rows(
    state: &AppState,
    authority: &Authority,
    operation_ref: &str,
    session_id: &str,
) -> Result<Vec<Value>, Response> {
    let mut rows = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut page = serde_json::json!({"limit": 1000});
        if let Some(current) = &cursor {
            page.as_object_mut()
                .expect("static page object")
                .insert("cursor".into(), Value::String(current.clone()));
        }
        let output = invoke_unique_operation(
            state,
            authority,
            operation_ref,
            serde_json::json!({"session_id": session_id, "$page": page}),
        )
        .await
        .map_err(|_| {
            problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "coding_context_authority_unavailable",
            )
        })?;
        if let Some(items) = output.as_array() {
            rows.extend(items.iter().cloned());
            break;
        }
        let object = output
            .as_object()
            .ok_or_else(|| problem(StatusCode::BAD_GATEWAY, "coding_context_authority_invalid"))?;
        if object.get("partial").and_then(Value::as_bool) != Some(false) {
            return Err(problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "coding_context_authority_partial",
            ));
        }
        let items = object
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| problem(StatusCode::BAD_GATEWAY, "coding_context_authority_invalid"))?;
        rows.extend(items.iter().cloned());
        if rows.len() > MAX_AGENTIDE_SERVICE_ROWS {
            return Err(problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                "coding_context_authority_limit_exceeded",
            ));
        }
        cursor = object
            .get("next_cursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    Ok(rows)
}

fn agentide_session_row(
    rows: Vec<Value>,
    authority: &Authority,
    session: &CodingSession,
    agentide_session_id: &str,
) -> Result<AgentIdeSessionRow, Response> {
    let mut matching = rows
        .into_iter()
        .filter_map(|row| serde_json::from_value::<AgentIdeSessionRow>(row).ok())
        .filter(|row| row.session_id == agentide_session_id);
    let row = matching
        .next()
        .ok_or_else(|| problem(StatusCode::FORBIDDEN, "coding_session_binding_refused"))?;
    if matching.next().is_some()
        || row.state != "Active"
        || row.owner != authority.subject
        || Some(row.workspace_root.as_str()) != session.working_materialization_ref.as_deref()
        || row.workspace_session_id.as_deref() != Some(session.id.as_str())
        || row.project_id.as_deref() != Some(session.project_id.as_str())
        || row.source_revision.as_deref() != Some(session.source_revision.as_str())
        || row.manifest_digest.as_deref() != session.manifest_sha256.as_deref()
    {
        return Err(problem(
            StatusCode::FORBIDDEN,
            "coding_session_binding_refused",
        ));
    }
    Ok(row)
}

async fn verified_coding_turn(
    state: &AppState,
    authority: &Authority,
    session: &CodingSession,
    agentide_session_id: &str,
    task_id: &str,
    attempt_id: &str,
) -> Result<VerifiedCodingTurn, Response> {
    let client = state.agent_platform.as_ref().ok_or_else(|| {
        problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "coding_agent_platform_unavailable",
        )
    })?;
    let bearer = authority.agent_platform_bearer.as_deref().ok_or_else(|| {
        problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "coding_agent_platform_unavailable",
        )
    })?;
    let task_id = TaskId::new(task_id).map_err(|_| {
        problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "coding_task_reference_invalid",
        )
    })?;
    let task: Task = client
        .get_task(bearer, &task_id)
        .await
        .map_err(|_| problem(StatusCode::FORBIDDEN, "coding_task_binding_refused"))?;
    if task.tenant_id.as_str() != authority.tenant_id
        || task.actor.as_str() != authority.subject
        || task.attempt_id.as_str() != attempt_id
        || !matches!(
            task.status,
            TaskStatus::Running | TaskStatus::AwaitingApproval
        )
    {
        return Err(problem(
            StatusCode::FORBIDDEN,
            "coding_task_binding_refused",
        ));
    }
    if task.input.get("kind").and_then(Value::as_str) != Some("coding_session_turn") {
        return Err(problem(
            StatusCode::FORBIDDEN,
            "coding_task_binding_refused",
        ));
    }
    // Normalize deliberate prompt attachments into Workspace's released AgentIDE contract at the
    // network boundary. Agent Platform remains the task authority, while Workspace remains the
    // context authority; neither crate's source revision becomes the other's runtime type identity.
    let input: CodingSessionTurnContext = serde_json::from_value(task.input.clone())
        .map_err(|_| problem(StatusCode::FORBIDDEN, "coding_task_binding_refused"))?;
    let CodingSessionTurnContext {
        workspace_session_id,
        agentide_session_id: task_agentide_session_id,
        focused_selections,
        open_files,
        active_diff,
    } = input;
    if workspace_session_id != session.id || task_agentide_session_id != agentide_session_id {
        return Err(problem(
            StatusCode::FORBIDDEN,
            "coding_task_binding_refused",
        ));
    }
    let session_rows = agentide_service_rows(
        state,
        authority,
        "agentide.get_session",
        agentide_session_id,
    )
    .await?;
    let agentide_session =
        agentide_session_row(session_rows, authority, session, agentide_session_id)?;
    let mut actor = ActorContext::new(ActorKind::Agent, task.agent_id.as_str())
        .map_err(|_| problem(StatusCode::BAD_GATEWAY, "coding_actor_context_invalid"))?;
    actor.agent = Some(task.agent_id.to_string());
    actor.attempt = Some(task.attempt_id.to_string());
    actor.delegation = task.delegation_id.map(|delegation| delegation.to_string());
    actor
        .validate()
        .map_err(|_| problem(StatusCode::BAD_GATEWAY, "coding_actor_context_invalid"))?;
    Ok(VerifiedCodingTurn {
        actor,
        objective: agentide_session.objective,
        focused_selections,
        open_files,
        active_diff,
    })
}

fn agentide_grants(
    rows: Vec<Value>,
    authority: &Authority,
    agentide_session_id: &str,
) -> Result<Vec<AuthorityGrant>, Response> {
    rows.into_iter()
        .map(|row| {
            let row: AgentIdeGrantRow = serde_json::from_value(row).map_err(|_| {
                problem(StatusCode::BAD_GATEWAY, "coding_context_authority_invalid")
            })?;
            if row.session_id != agentide_session_id
                || row.owner != authority.subject
                || !matches!(row.state.as_str(), "Active" | "Revoked")
            {
                return Err(problem(
                    StatusCode::BAD_GATEWAY,
                    "coding_context_authority_invalid",
                ));
            }
            let maximum_risk = match row.maximum_risk.as_str() {
                "Low" => AgentIdeRisk::Low,
                "Medium" => AgentIdeRisk::Medium,
                _ => {
                    return Err(problem(
                        StatusCode::BAD_GATEWAY,
                        "coding_context_authority_invalid",
                    ));
                }
            };
            let revision = u64::try_from(row.revision)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    problem(StatusCode::BAD_GATEWAY, "coding_context_authority_invalid")
                })?;
            let grant = AuthorityGrant {
                format: "agentide.authority-grant/2".into(),
                id: row.grant_id,
                session_id: row.session_id,
                grantee: row.grantee,
                allowed_intents: row.allowed_intents,
                path_prefixes: row.path_prefixes,
                maximum_risk,
                expires_at: row.expires_at,
                revision,
                revoked: row.state == "Revoked",
            };
            grant.validate().map_err(|_| {
                problem(StatusCode::BAD_GATEWAY, "coding_context_authority_invalid")
            })?;
            Ok(grant)
        })
        .collect()
}

fn coding_intent_profile() -> Result<(IntentProfile, BTreeSet<String>), Response> {
    let profile = IntentProfile::embedded().map_err(|_| {
        problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "coding_intent_profile_invalid",
        )
    })?;
    let implemented = CODING_AGENT_INTENTS
        .into_iter()
        .map(str::to_owned)
        .collect();
    Ok((profile, implemented))
}

fn sha256_text(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

fn context_activity(code: &str, reference: &str) -> ContextRecord {
    ContextRecord {
        id: format!("{code}:{}", sha256_text(reference)),
        kind: code.to_owned(),
        state: None,
        summary: reference.to_owned(),
        sha256: None,
        observed_at: None,
    }
}

fn valid_context_selection(selection: &ContextSelection) -> bool {
    selection.validate().is_ok()
        && !selection.id.trim().is_empty()
        && selection.id.len() <= MAX_MATERIALIZATION_KEY_BYTES
        && !selection.reference.trim().is_empty()
        && selection.reference.len() <= 4 * 1024
        && !selection.reference.contains('\0')
        && !selection.truncated
        && selection.content.len() <= MAX_CONTEXT_SELECTION_BYTES
        && valid_sha256(&selection.sha256)
        && selection.sha256 == sha256_text(&selection.content)
        && match (selection.start_line, selection.end_line) {
            (Some(start), Some(end)) => start > 0 && end >= start,
            (None, None) => true,
            _ => false,
        }
        && match selection.kind {
            SelectionKind::Editor => valid_repository_path(&selection.reference),
            SelectionKind::DiffHunk => selection.reference.starts_with("workspace-diff/"),
            SelectionKind::Terminal => selection.reference.starts_with("terminal/"),
            SelectionKind::Process | SelectionKind::Evidence => true,
        }
}

fn admit_context_selection(
    selection: &ContextSelection,
    selection_count: &mut usize,
    total_bytes: &mut usize,
) -> bool {
    if !valid_context_selection(selection)
        || *selection_count >= MAX_CONTEXT_SELECTIONS
        || total_bytes.saturating_add(selection.content.len()) > MAX_CONTEXT_TOTAL_BYTES
    {
        return false;
    }
    *selection_count += 1;
    *total_bytes += selection.content.len();
    true
}

fn pin_kind(value: &str) -> Option<SelectionKind> {
    match value {
        "Editor" => Some(SelectionKind::Editor),
        "DiffHunk" => Some(SelectionKind::DiffHunk),
        "Terminal" => Some(SelectionKind::Terminal),
        "Process" => Some(SelectionKind::Process),
        "Evidence" => Some(SelectionKind::Evidence),
        _ => None,
    }
}

fn pin_line(value: Option<i64>) -> Result<Option<u64>, ()> {
    match value {
        None => Ok(None),
        Some(line) => u64::try_from(line)
            .ok()
            .filter(|line| *line > 0)
            .map(Some)
            .ok_or(()),
    }
}

fn editor_pin_content(file: &CompleteWorkspaceFile, start: u64, end: u64) -> Option<String> {
    let content = std::str::from_utf8(&file.bytes).ok()?;
    let lines = content.split('\n').collect::<Vec<_>>();
    let start = usize::try_from(start.checked_sub(1)?).ok()?;
    let end = usize::try_from(end).ok()?;
    (start < end && end <= lines.len()).then(|| lines[start..end].join("\n"))
}

fn diff_pin_content(diff: &DiffProjection, reference: &str) -> Option<String> {
    let suffix = reference.strip_prefix("workspace-diff/")?;
    let (digest, suffix) = suffix.split_once('/')?;
    if digest != diff.digest {
        return None;
    }
    let (path, hunk_id) = suffix.rsplit_once('/')?;
    let hunk = diff
        .files
        .iter()
        .find(|file| {
            file.new_path.as_deref() == Some(path) || file.old_path.as_deref() == Some(path)
        })?
        .hunks
        .iter()
        .find(|hunk| hunk.id == hunk_id)?;
    Some(
        hunk.lines
            .iter()
            .map(|line| line.content.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

async fn hydrate_context_pin(
    row: AgentIdePinRow,
    agentide_session_id: &str,
    working: &SubstrateWorkspace,
    max_file_bytes: u64,
    diff: &DiffProjection,
) -> Result<ContextSelection, &'static str> {
    if row.session_id != agentide_session_id || row.state != "Active" || !valid_sha256(&row.sha256)
    {
        return Err("context_pin_invalid");
    }
    let kind = pin_kind(&row.kind).ok_or("context_pin_invalid")?;
    let start_line = pin_line(row.start_line).map_err(|()| "context_pin_invalid")?;
    let end_line = pin_line(row.end_line).map_err(|()| "context_pin_invalid")?;
    let content = match kind {
        SelectionKind::Editor => {
            let (Some(start), Some(end)) = (start_line, end_line) else {
                return Err("context_pin_invalid");
            };
            if !valid_repository_path(&row.reference) || end < start {
                return Err("context_pin_invalid");
            }
            let file = read_complete_file(working, &row.reference, max_file_bytes)
                .await
                .map_err(|_| "context_pin_content_unavailable")?
                .ok_or("context_pin_stale")?;
            editor_pin_content(&file, start, end).ok_or("context_pin_stale")?
        }
        SelectionKind::DiffHunk => {
            diff_pin_content(diff, &row.reference).ok_or("context_pin_stale")?
        }
        SelectionKind::Terminal | SelectionKind::Process | SelectionKind::Evidence => {
            return Err("context_pin_content_unavailable");
        }
    };
    let actor =
        ActorContext::new(ActorKind::Human, row.owner).map_err(|_| "context_pin_invalid")?;
    let expected_sha256 = row.sha256;
    let selection = ContextSelection::new(
        row.pin_id,
        kind,
        row.reference,
        start_line,
        end_line,
        content,
        AttachmentProvenance {
            format: "agentide.attachment-provenance/1".into(),
            actor,
            source: "agentide.context-pin".into(),
            source_revision: expected_sha256.clone(),
            observed_at: Utc::now().to_rfc3339(),
        },
    )
    .map_err(|_| "context_pin_stale")?;
    if selection.sha256 == expected_sha256 && valid_context_selection(&selection) {
        Ok(selection)
    } else {
        Err("context_pin_stale")
    }
}

async fn current_open_files(
    requested: Vec<OpenFileReference>,
    working: &SubstrateWorkspace,
    max_file_bytes: u64,
    recent_activity: &mut Vec<ContextRecord>,
) -> Vec<OpenFileReference> {
    let mut current = Vec::new();
    for requested in requested.into_iter().take(MAX_CONTEXT_OPEN_FILES) {
        if !valid_repository_path(&requested.path)
            || !valid_sha256(&requested.sha256)
            || requested
                .cursor
                .as_ref()
                .is_some_and(|cursor| cursor.line == 0 || cursor.column == 0)
        {
            recent_activity.push(context_activity("open_file_invalid", &requested.path));
            continue;
        }
        let Ok(Some(file)) = read_complete_file(working, &requested.path, max_file_bytes).await
        else {
            recent_activity.push(context_activity("open_file_unavailable", &requested.path));
            continue;
        };
        if requested.sha256 != file.sha256 {
            recent_activity.push(context_activity("open_file_stale", &requested.path));
        }
        current.push(OpenFileReference {
            path: requested.path,
            sha256: file.sha256,
            cursor: requested.cursor,
            dirty: requested.dirty,
        });
    }
    current
}

fn agentide_terminals(
    terminals: Vec<StoredTerminal>,
    agentide_session_id: &str,
    recent_activity: &mut Vec<ContextRecord>,
) -> Vec<AgentIdeTerminalSession> {
    let mut projected = Vec::new();
    for terminal in terminals {
        let terminal = terminal.public;
        if terminal.agentide_session_id != agentide_session_id {
            continue;
        }
        let state = match terminal.state {
            TerminalState::Running => AgentIdeTerminalState::Running,
            TerminalState::Exited => AgentIdeTerminalState::Exited,
            TerminalState::Terminated => AgentIdeTerminalState::Terminated,
            TerminalState::Preparing | TerminalState::Refused | TerminalState::Unknown => {
                recent_activity.push(context_activity(
                    "terminal_lifecycle_not_projected",
                    &terminal.id,
                ));
                continue;
            }
        };
        let Some(process_id) = terminal.process_id else {
            recent_activity.push(context_activity("terminal_process_unknown", &terminal.id));
            continue;
        };
        let Ok(actor) = ActorContext::new(ActorKind::Human, terminal.actor) else {
            recent_activity.push(context_activity("terminal_actor_invalid", &terminal.id));
            continue;
        };
        projected.push(AgentIdeTerminalSession {
            format: "agentide.terminal-session/2".into(),
            id: terminal.id,
            session_id: terminal.agentide_session_id,
            profile: terminal.profile.id,
            actor,
            process_id,
            working_directory: terminal.profile.working_directory,
            network: "none".into(),
            state,
            output_sequence: 0,
            exit_code: terminal.exit.and_then(|exit| exit.code),
        });
    }
    projected
}

fn pending_approvals(
    rows: Vec<Value>,
    agentide_session_id: &str,
    attempt_id: &str,
) -> Vec<ContextRecord> {
    rows.into_iter()
        .filter(|row| {
            row.get("session_id").and_then(Value::as_str) == Some(agentide_session_id)
                && row.get("attempt_ref").and_then(Value::as_str) == Some(attempt_id)
                && row.get("state").and_then(Value::as_str) == Some("Pending")
        })
        .filter_map(|row| {
            Some(ContextRecord {
                id: row.get("checkpoint_id")?.as_str()?.to_owned(),
                kind: "approval_checkpoint".into(),
                state: Some("pending".into()),
                summary: row.get("checkpoint_ref")?.as_str()?.to_owned(),
                sha256: Some(row.get("plan_digest")?.as_str()?.to_owned()),
                observed_at: None,
            })
        })
        .collect()
}

#[allow(clippy::too_many_lines)]
async fn coding_actor_view(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(input): Json<CodingActorViewRequest>,
) -> Response {
    if !valid_ref(&input.agentide_session_id)
        || !valid_ref(&input.task_id)
        || !valid_ref(&input.attempt_id)
        || input.turn == 0
    {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "coding_actor_view_invalid",
        );
    }
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let (session, base, working) =
        match ready_session_materializations(&state, &authority, &session_id).await {
            Ok(materializations) => materializations,
            Err(response) => return response,
        };
    let verified = match verified_coding_turn(
        &state,
        &authority,
        &session,
        &input.agentide_session_id,
        &input.task_id,
        &input.attempt_id,
    )
    .await
    {
        Ok(verified) => verified,
        Err(response) => return response,
    };
    let grant_rows = match agentide_service_rows(
        &state,
        &authority,
        "agentide.list_grants",
        &input.agentide_session_id,
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let coordination_grants = grant_rows.clone();
    let grants = match agentide_grants(grant_rows, &authority, &input.agentide_session_id) {
        Ok(grants) => grants,
        Err(response) => return response,
    };
    let (profile, implemented) = match coding_intent_profile() {
        Ok(profile) => profile,
        Err(response) => return response,
    };
    let Ok((inventory, withheld)) = resolve_intent_inventory(
        &profile,
        &input.agentide_session_id,
        &verified.actor,
        &implemented,
        &grants,
        true,
        Utc::now(),
        input.turn,
    ) else {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "coding_intent_inventory_invalid",
        );
    };
    let base_files = match materialization_files(&base, &session).await {
        Ok(files) => files,
        Err(response) => return response,
    };
    let working_files = match materialization_files(&working, &session).await {
        Ok(files) => files,
        Err(response) => return response,
    };
    let diff = canonical_diff(
        &ChangeSelector::Workspace,
        DiffMode::Patch,
        &session.source_revision,
        &base_files,
        &working_files,
        &verified.actor.subject,
    );
    let mut recent_activity = Vec::new();
    let mut focused_selections = Vec::new();
    let mut selection_count = 0;
    let mut selection_bytes = 0;
    for selection in verified.focused_selections {
        if admit_context_selection(&selection, &mut selection_count, &mut selection_bytes) {
            focused_selections.push(selection);
        } else {
            recent_activity.push(context_activity(
                "focused_selection_refused",
                &selection.reference,
            ));
        }
    }
    let pin_rows = match agentide_service_rows(
        &state,
        &authority,
        "agentide.list_context_pins",
        &input.agentide_session_id,
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let coordination_pins = pin_rows.clone();
    let mut pins = Vec::new();
    for row in pin_rows {
        let reference = row
            .get("reference")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let Ok(row) = serde_json::from_value::<AgentIdePinRow>(row) else {
            recent_activity.push(context_activity("context_pin_invalid", &reference));
            continue;
        };
        if row.state == "Removed" {
            continue;
        }
        match hydrate_context_pin(
            row,
            &input.agentide_session_id,
            &working,
            session.limits.max_file_bytes,
            &diff,
        )
        .await
        {
            Ok(selection)
                if admit_context_selection(
                    &selection,
                    &mut selection_count,
                    &mut selection_bytes,
                ) =>
            {
                pins.push(selection);
            }
            Ok(selection) => recent_activity.push(context_activity(
                "context_selection_limit_exceeded",
                &selection.reference,
            )),
            Err(code) => recent_activity.push(context_activity(code, &reference)),
        }
    }
    let open_files = current_open_files(
        verified.open_files,
        &working,
        session.limits.max_file_bytes,
        &mut recent_activity,
    )
    .await;
    let terminals = match state.store.terminals(&authority, &session.id).await {
        Ok(terminals) => {
            agentide_terminals(terminals, &input.agentide_session_id, &mut recent_activity)
        }
        Err(error) => return store_problem(&error),
    };
    let approval_rows = match agentide_service_rows(
        &state,
        &authority,
        "agentide.list_approval_checkpoints",
        &input.agentide_session_id,
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let coordination_approvals = approval_rows.clone();
    let approvals = pending_approvals(approval_rows, &input.agentide_session_id, &input.attempt_id);
    let active_diff = match verified.active_diff {
        None => None,
        Some(AgentIdeChangeSelector::Workspace) => Some(AgentIdeChangeSelector::Workspace),
        Some(_) => {
            recent_activity.push(context_activity(
                "active_diff_selector_unavailable",
                "active-diff",
            ));
            None
        }
    };
    let mut workbench = ActorWorkbench {
        tabs: open_files.iter().map(|file| file.path.clone()).collect(),
        focused_file: focused_selections
            .iter()
            .find(|selection| selection.kind == SelectionKind::Editor)
            .map(|selection| selection.reference.clone())
            .or_else(|| open_files.first().map(|file| file.path.clone())),
        dirty_paths: open_files
            .iter()
            .filter(|file| file.dirty)
            .map(|file| file.path.clone())
            .collect(),
        ..ActorWorkbench::default()
    };
    workbench
        .cursors
        .extend(open_files.iter().filter_map(|file| {
            file.cursor
                .clone()
                .map(|cursor| (file.path.clone(), cursor))
        }));
    let Ok(coordination_digest) = canonical_json_sha256(&serde_json::json!({
        "session_id": input.agentide_session_id,
        "workspace_session_id": session.id,
        "source_revision": session.source_revision,
        "grants": coordination_grants,
        "pins": coordination_pins,
        "approvals": coordination_approvals,
    })) else {
        return problem(
            StatusCode::BAD_GATEWAY,
            "coding_coordination_revision_invalid",
        );
    };
    let mut context = ContextPack {
        format: "agentide.context-pack/2".into(),
        objective: verified.objective,
        source_revision: session.source_revision,
        working_changes: Some(diff.digest),
        pins,
        focused_selections,
        open_files,
        active_diff,
        terminals,
        processes: Vec::new(),
        agent_lanes: Vec::new(),
        approvals,
        evidence: Vec::new(),
        recent_activity,
        revision: input.turn,
        digest: String::new(),
    };
    if context.seal().is_err() {
        return problem(StatusCode::BAD_GATEWAY, "coding_context_pack_invalid");
    }
    let view = ActorView {
        format: "agentide.actor-view/2".into(),
        actor: verified.actor,
        workbench,
        coordination: CoordinationRevision {
            revision: input.turn,
            digest: coordination_digest,
        },
        context,
        inventory,
        withheld,
    };
    if view.validate().is_err() {
        return problem(StatusCode::BAD_GATEWAY, "coding_actor_view_invalid");
    }
    confidential(Json(view).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodeReadArguments {
    path: String,
    offset: u64,
    limit_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodeEditArguments {
    operation_id: String,
    path: String,
    content: String,
    expected_sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodeCreateArguments {
    operation_id: String,
    path: String,
    content: String,
    expected_absent: bool,
}

fn coding_intent_path(intent: &str, arguments: &Value) -> Result<Option<String>, Response> {
    if matches!(intent, "code_read" | "code_edit" | "code_create") {
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| valid_repository_path(path))
            .ok_or_else(|| {
                problem(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "coding_intent_arguments_invalid",
                )
            })?;
        Ok(Some(path.to_owned()))
    } else {
        Ok(None)
    }
}

#[allow(clippy::too_many_lines)]
async fn invoke_coding_intent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(input): Json<CodingIntentInvocation>,
) -> Response {
    if !valid_ref(&input.agentide_session_id)
        || !valid_ref(&input.task_id)
        || !valid_ref(&input.attempt_id)
        || !valid_ref(&input.call_id)
        || !CODING_AGENT_INTENTS.contains(&input.intent.as_str())
        || !input.arguments.is_object()
    {
        return problem(StatusCode::UNPROCESSABLE_ENTITY, "coding_intent_invalid");
    }
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let (session, base, working) =
        match ready_session_materializations(&state, &authority, &session_id).await {
            Ok(materializations) => materializations,
            Err(response) => return response,
        };
    let verified = match verified_coding_turn(
        &state,
        &authority,
        &session,
        &input.agentide_session_id,
        &input.task_id,
        &input.attempt_id,
    )
    .await
    {
        Ok(verified) => verified,
        Err(response) => return response,
    };
    let grant_rows = match agentide_service_rows(
        &state,
        &authority,
        "agentide.list_grants",
        &input.agentide_session_id,
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let grants = match agentide_grants(grant_rows, &authority, &input.agentide_session_id) {
        Ok(grants) => grants,
        Err(response) => return response,
    };
    let (profile, implemented) = match coding_intent_profile() {
        Ok(profile) => profile,
        Err(response) => return response,
    };
    let Some(definition) = profile.find(&input.intent) else {
        return problem(StatusCode::NOT_FOUND, "coding_intent_unavailable");
    };
    let path = match coding_intent_path(&input.intent, &input.arguments) {
        Ok(path) => path,
        Err(response) => return response,
    };
    if let Err(reason) = authorize_intent(
        definition,
        &input.agentide_session_id,
        &verified.actor,
        &implemented,
        &grants,
        true,
        Utc::now(),
        path.as_deref(),
    ) {
        return problem(StatusCode::FORBIDDEN, &reason.code);
    }
    let output = match input.intent.as_str() {
        "code_read" => {
            let arguments = match serde_json::from_value::<CodeReadArguments>(input.arguments) {
                Ok(arguments)
                    if valid_repository_path(&arguments.path)
                        && arguments.limit_bytes > 0
                        && arguments.limit_bytes <= session.limits.max_file_bytes =>
                {
                    arguments
                }
                _ => {
                    return problem(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "coding_intent_arguments_invalid",
                    );
                }
            };
            let file = match working
                .read_file_v2(&arguments.path, arguments.offset, arguments.limit_bytes)
                .await
            {
                Ok(file) => file,
                Err(error) => return substrate_problem(&error),
            };
            let Ok(content) = String::from_utf8(file.bytes) else {
                return problem(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "coding_file_binary_refused",
                );
            };
            serde_json::json!({
                "path": file.path,
                "sha256": file.sha256,
                "size": file.size,
                "offset": file.offset,
                "next_offset": file.next_offset,
                "eof": file.eof,
                "content": content,
                "truncated": !file.eof
            })
        }
        "code_changes" => {
            if !input
                .arguments
                .as_object()
                .is_some_and(serde_json::Map::is_empty)
            {
                return problem(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "coding_intent_arguments_invalid",
                );
            }
            let base_files = match materialization_files(&base, &session).await {
                Ok(files) => files,
                Err(response) => return response,
            };
            let working_files = match materialization_files(&working, &session).await {
                Ok(files) => files,
                Err(response) => return response,
            };
            match serde_json::to_value(canonical_diff(
                &ChangeSelector::Workspace,
                DiffMode::Patch,
                &session.source_revision,
                &base_files,
                &working_files,
                &verified.actor.subject,
            )) {
                Ok(value) => value,
                Err(_) => {
                    return problem(StatusCode::BAD_GATEWAY, "coding_intent_result_invalid");
                }
            }
        }
        "code_edit" => {
            let arguments = match serde_json::from_value::<CodeEditArguments>(input.arguments) {
                Ok(arguments)
                    if valid_repository_path(&arguments.path)
                        && valid_ref(&arguments.operation_id)
                        && arguments.content.len() as u64 <= session.limits.max_file_bytes
                        && arguments
                            .expected_sha256
                            .as_deref()
                            .is_some_and(valid_sha256) =>
                {
                    arguments
                }
                _ => {
                    return problem(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "coding_intent_arguments_invalid",
                    );
                }
            };
            let expected_sha256 = arguments
                .expected_sha256
                .as_deref()
                .expect("validated expected digest");
            let operation_id = substrate_operation_id(
                &session.id,
                &[
                    &input.task_id,
                    &input.attempt_id,
                    &input.call_id,
                    &input.intent,
                    &arguments.operation_id,
                    &arguments.path,
                    expected_sha256,
                    &sha256_text(&arguments.content),
                ],
            );
            match working
                .replace_file(
                    &arguments.path,
                    arguments.content.as_bytes(),
                    ExpectedFileState::Sha256 {
                        sha256: expected_sha256.to_owned(),
                    },
                    false,
                    Some(operation_id),
                )
                .await
            {
                Ok(_) => {}
                Err(SubstrateError::Refusal(refusal))
                    if refusal.class == SubstrateRefusalClass::Conflict =>
                {
                    return problem(StatusCode::CONFLICT, "coding_intent_stale");
                }
                Err(error) => return substrate_problem(&error),
            }
            let file = match project_file(
                &arguments.path,
                &base,
                &working,
                session.limits.max_file_bytes,
            )
            .await
            {
                Ok(Some(file)) => file,
                Ok(None) => {
                    return problem(StatusCode::BAD_GATEWAY, "coding_intent_result_invalid");
                }
                Err(response) => return response,
            };
            match serde_json::to_value(file) {
                Ok(value) => value,
                Err(_) => {
                    return problem(StatusCode::BAD_GATEWAY, "coding_intent_result_invalid");
                }
            }
        }
        "code_create" => {
            let arguments = match serde_json::from_value::<CodeCreateArguments>(input.arguments) {
                Ok(arguments)
                    if valid_repository_path(&arguments.path)
                        && valid_ref(&arguments.operation_id)
                        && arguments.expected_absent
                        && arguments.content.len() as u64 <= session.limits.max_file_bytes =>
                {
                    arguments
                }
                _ => {
                    return problem(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "coding_intent_arguments_invalid",
                    );
                }
            };
            let operation_id = substrate_operation_id(
                &session.id,
                &[
                    &input.task_id,
                    &input.attempt_id,
                    &input.call_id,
                    &input.intent,
                    &arguments.operation_id,
                    &arguments.path,
                    &sha256_text(&arguments.content),
                ],
            );
            match working
                .replace_file(
                    &arguments.path,
                    arguments.content.as_bytes(),
                    ExpectedFileState::Absent,
                    true,
                    Some(operation_id),
                )
                .await
            {
                Ok(_) => {}
                Err(SubstrateError::Refusal(refusal))
                    if refusal.class == SubstrateRefusalClass::Conflict =>
                {
                    return problem(StatusCode::CONFLICT, "coding_intent_stale");
                }
                Err(error) => return substrate_problem(&error),
            }
            let file = match project_file(
                &arguments.path,
                &base,
                &working,
                session.limits.max_file_bytes,
            )
            .await
            {
                Ok(Some(file)) => file,
                Ok(None) => {
                    return problem(StatusCode::BAD_GATEWAY, "coding_intent_result_invalid");
                }
                Err(response) => return response,
            };
            match serde_json::to_value(file) {
                Ok(value) => value,
                Err(_) => {
                    return problem(StatusCode::BAD_GATEWAY, "coding_intent_result_invalid");
                }
            }
        }
        "terminal_list" => {
            if !input
                .arguments
                .as_object()
                .is_some_and(serde_json::Map::is_empty)
            {
                return problem(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "coding_intent_arguments_invalid",
                );
            }
            let terminals = match state.store.terminals(&authority, &session.id).await {
                Ok(terminals) => terminals,
                Err(error) => return store_problem(&error),
            };
            let mut recent_activity = Vec::new();
            serde_json::json!({
                "terminals": agentide_terminals(
                    terminals,
                    &input.agentide_session_id,
                    &mut recent_activity,
                ),
                "recent_activity": recent_activity
            })
        }
        _ => return problem(StatusCode::NOT_FOUND, "coding_intent_unavailable"),
    };
    confidential(Json(CodingIntentResult { output }).into_response())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RepositoryQuery {
    query: String,
}

async fn repositories(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<RepositoryQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return problem(StatusCode::UNPROCESSABLE_ENTITY, "repository_query_invalid");
    };
    if query.query.len() > 512 {
        return problem(StatusCode::UNPROCESSABLE_ENTITY, "repository_query_invalid");
    }
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    match search_projects(&state, &authority, query.query.trim()).await {
        Ok(mut projects) => {
            for project in &mut projects {
                project.opened_project_id = state
                    .store
                    .project_id_for(
                        &authority.tenant_id,
                        &project.forge_instance_ref,
                        &project.project_ref,
                    )
                    .await
                    .ok()
                    .flatten();
            }
            confidential(Json(projects).into_response())
        }
        Err(response) => response,
    }
}

async fn open_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<OpenProject>,
) -> Response {
    if !valid_ref(&input.forge_instance_ref) || !valid_ref(&input.project_ref) {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "project_reference_invalid",
        );
    }
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    if let Err(response) = substrate_client(&state, &authority).await {
        return response;
    }
    let candidate = match reachable_project(
        &state,
        &authority,
        &input.forge_instance_ref,
        &input.project_ref,
    )
    .await
    {
        Ok(candidate) => candidate,
        Err(response) => return response,
    };
    let Some(selected_branch) = candidate.default_branch.clone() else {
        return problem(StatusCode::CONFLICT, "project_default_branch_unavailable");
    };
    let project = Project {
        id: project_id(
            &authority.tenant_id,
            &candidate.forge_instance_ref,
            &candidate.project_ref,
        ),
        forge_instance_ref: candidate.forge_instance_ref,
        project_ref: candidate.project_ref,
        path_with_namespace: candidate.path_with_namespace,
        name: candidate.name,
        default_branch: candidate.default_branch,
        selected_branch,
        pinned_commit: None,
        web_url: candidate.web_url,
    };
    let opened = match state.store.open_project(&authority, &project).await {
        Ok(project) => project,
        Err(error) => return store_problem(&error),
    };
    let visible_branches = match discover_branches(&state, &authority, &opened).await {
        Ok(branches) => branches,
        Err(response) => return response,
    };
    let selected = visible_branches
        .iter()
        .find(|branch| branch.name == opened.selected_branch);
    let Some(selected) = selected else {
        return problem(StatusCode::CONFLICT, "project_default_branch_unavailable");
    };
    match state
        .store
        .select_branch(&authority, &opened, &selected.name, &selected.commit)
        .await
    {
        Ok(project) => confidential(Json(project).into_response()),
        Err(error) => store_problem(&error),
    }
}

async fn project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    match accessible_project(&state, &authority, &project_id).await {
        Ok(project) => confidential(Json(project).into_response()),
        Err(response) => response,
    }
}

async fn branches(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let project = match accessible_project(&state, &authority, &project_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    match discover_branches(&state, &authority, &project).await {
        Ok(branches) => confidential(Json(branches).into_response()),
        Err(response) => response,
    }
}

async fn repository_tree(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let project = match accessible_project(&state, &authority, &project_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    if let Err(response) = substrate_client(&state, &authority).await {
        return response;
    }
    match exact_repository_tree(&state, &authority, &project).await {
        Ok(entries) => confidential(Json(entries).into_response()),
        Err(response) => response,
    }
}

async fn select_branch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(input): Json<SelectBranch>,
) -> Response {
    if !valid_branch(&input.branch) {
        return problem(StatusCode::UNPROCESSABLE_ENTITY, "branch_invalid");
    }
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let project = match accessible_project(&state, &authority, &project_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let branch = match discover_branches(&state, &authority, &project).await {
        Ok(branches) => branches
            .into_iter()
            .find(|branch| branch.name == input.branch),
        Err(response) => return response,
    };
    let Some(branch) = branch else {
        return problem(StatusCode::NOT_FOUND, "branch_not_found");
    };
    if let Err(response) = substrate_client(&state, &authority).await {
        return response;
    }
    match state
        .store
        .select_branch(&authority, &project, &branch.name, &branch.commit)
        .await
    {
        Ok(project) => confidential(Json(project).into_response()),
        Err(error) => store_problem(&error),
    }
}

async fn coding_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    if let Err(response) = accessible_project(&state, &authority, &project_id).await {
        return response;
    }
    match state.store.coding_sessions(&authority, &project_id).await {
        Ok(sessions) => confidential(
            Json(sessions.into_iter().map(public_session).collect::<Vec<_>>()).into_response(),
        ),
        Err(error) => store_problem(&error),
    }
}

async fn coding_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let session = match state.store.coding_session(&authority, &session_id).await {
        Ok(session) => session,
        Err(error) => return store_problem(&error),
    };
    let project = match accessible_project(&state, &authority, &session.project_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    if session.state == CodingSessionState::Preparing
        && !state.materialization_workers.is_active(&session.id).await
        && let Ok(Some(client)) = substrate_client(&state, &authority).await
    {
        spawn_coding_session_materialization(
            state.clone(),
            authority,
            project,
            client,
            session.clone(),
        )
        .await;
    }
    confidential(Json(public_session(session)).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CodingTreeQuery {
    query: String,
    limit: u32,
}

impl Default for CodingTreeQuery {
    fn default() -> Self {
        Self {
            query: String::new(),
            limit: 1_000,
        }
    }
}

async fn coding_tree(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    query: Result<Query<CodingTreeQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "coding_tree_query_invalid",
        );
    };
    if query.limit == 0
        || query.limit > b10x_substrate_sdk::MAX_LIST_ITEMS
        || query.query.len() > 512
    {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "coding_tree_query_invalid",
        );
    }
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let (session, _, working) =
        match ready_session_materializations(&state, &authority, &session_id).await {
            Ok(materializations) => materializations,
            Err(response) => return response,
        };
    let tree = match working.tree(session.limits.max_files, true).await {
        Ok(tree) => tree,
        Err(error) => return substrate_problem(&error),
    };
    let needle = query.query.to_ascii_lowercase();
    let matching = tree
        .items
        .into_iter()
        .filter_map(|entry| {
            let kind = serde_json::to_value(entry.kind).ok()?.as_str()?.to_owned();
            if !needle.is_empty() && !entry.path.to_ascii_lowercase().contains(&needle) {
                return None;
            }
            Some(CodingTreeEntry {
                path: entry.path,
                kind,
                size: entry.size,
                sha256: None,
            })
        })
        .collect::<Vec<_>>();
    let returned = matching.len().min(query.limit as usize);
    let omitted = (!tree.truncated).then_some((matching.len() - returned) as u64);
    let projection = CodingTreeProjection {
        format: "workspace.coding-tree/1".to_owned(),
        entries: matching.into_iter().take(returned).collect(),
        truncated: tree.truncated || omitted.is_some_and(|count| count > 0),
        omitted: if tree.truncated { None } else { omitted },
    };
    confidential(Json(projection).into_response())
}

async fn coding_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, path)): Path<(String, String)>,
) -> Response {
    if !valid_repository_path(&path) {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "workspace_file_path_invalid",
        );
    }
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let (session, base, working) =
        match ready_session_materializations(&state, &authority, &session_id).await {
            Ok(materializations) => materializations,
            Err(response) => return response,
        };
    match project_file(&path, &base, &working, session.limits.max_file_bytes).await {
        Ok(Some(file)) => confidential(Json(file).into_response()),
        Ok(None) => problem(StatusCode::NOT_FOUND, "workspace_file_not_found"),
        Err(response) => response,
    }
}

async fn write_coding_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, path)): Path<(String, String)>,
    Json(input): Json<WriteFile>,
) -> Response {
    if !valid_repository_path(&path)
        || input.operation_id.trim().is_empty()
        || input.operation_id.len() > MAX_MATERIALIZATION_KEY_BYTES
        || matches!(
            &input.expected,
            FileExpectedState::Sha256 { sha256 } if !valid_sha256(sha256)
        )
    {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "workspace_file_write_invalid",
        );
    }
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let (session, base, working) =
        match ready_session_materializations(&state, &authority, &session_id).await {
            Ok(materializations) => materializations,
            Err(response) => return response,
        };
    if input.content.len() as u64 > session.limits.max_file_bytes {
        return problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            "workspace_file_limit_exceeded",
        );
    }
    let expected = match &input.expected {
        FileExpectedState::Absent => ExpectedFileState::Absent,
        FileExpectedState::Sha256 { sha256 } => ExpectedFileState::Sha256 {
            sha256: sha256.clone(),
        },
    };
    let operation_id = file_operation_id(&session.id, &path, &input);
    let result = working
        .replace_file(
            &path,
            input.content.as_bytes(),
            expected,
            input.create_parents,
            Some(operation_id),
        )
        .await;
    match result {
        Ok(_) => match project_file(&path, &base, &working, session.limits.max_file_bytes).await {
            Ok(Some(file)) => {
                let status = if matches!(input.expected, FileExpectedState::Absent) {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                };
                confidential((status, Json(file)).into_response())
            }
            Ok(None) => problem(StatusCode::BAD_GATEWAY, "workspace_file_write_inconsistent"),
            Err(response) => response,
        },
        Err(SubstrateError::Refusal(refusal))
            if refusal.class == SubstrateRefusalClass::Conflict =>
        {
            match project_file(&path, &base, &working, session.limits.max_file_bytes).await {
                Ok(Some(latest)) => {
                    let base =
                        match base_file_projection(&path, &base, session.limits.max_file_bytes)
                            .await
                        {
                            Ok(base) => base,
                            Err(response) => return response,
                        };
                    confidential(
                        (
                            StatusCode::CONFLICT,
                            Json(FileConflict {
                                code: "workspace_file_stale".to_owned(),
                                base,
                                latest,
                            }),
                        )
                            .into_response(),
                    )
                }
                Ok(None) => problem(StatusCode::CONFLICT, "workspace_file_stale"),
                Err(response) => response,
            }
        }
        Err(error) => substrate_problem(&error),
    }
}

async fn resolve_diff(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(input): Json<ResolveDiff>,
) -> Response {
    if !matches!(input.selector, ChangeSelector::Workspace) {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "diff_selector_unavailable",
        );
    }
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let (session, base, working) =
        match ready_session_materializations(&state, &authority, &session_id).await {
            Ok(materializations) => materializations,
            Err(response) => return response,
        };
    let base_files = match materialization_files(&base, &session).await {
        Ok(files) => files,
        Err(response) => return response,
    };
    let working_files = match materialization_files(&working, &session).await {
        Ok(files) => files,
        Err(response) => return response,
    };
    let projection = canonical_diff(
        &input.selector,
        input.mode,
        &session.source_revision,
        &base_files,
        &working_files,
        &authority.subject,
    );
    confidential(Json(projection).into_response())
}

async fn list_terminal_profiles(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    if let Err(response) = ready_session_materializations(&state, &authority, &session_id).await {
        return response;
    }
    confidential(Json(state.terminal_profiles.list()).into_response())
}

async fn list_terminals(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    if let Err(response) = ready_session_materializations(&state, &authority, &session_id).await {
        return response;
    }
    match state.store.terminals(&authority, &session_id).await {
        Ok(terminals) => confidential(
            Json(
                terminals
                    .into_iter()
                    .map(|terminal| terminal.public)
                    .collect::<Vec<_>>(),
            )
            .into_response(),
        ),
        Err(error) => store_problem(&error),
    }
}

async fn get_terminal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(terminal_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let terminal = match state.store.terminal(&authority, &terminal_id).await {
        Ok(terminal) => terminal,
        Err(error) => return store_problem(&error),
    };
    let coding_session = match state
        .store
        .coding_session(&authority, &terminal.public.coding_session_id)
        .await
    {
        Ok(session) => session,
        Err(error) => return store_problem(&error),
    };
    if let Err(response) = accessible_project(&state, &authority, &coding_session.project_id).await
    {
        return response;
    }
    confidential(Json(terminal.public).into_response())
}

#[allow(clippy::too_many_lines)] // Admission, durable reservation, Substrate start, and broker claim stay visibly ordered.
async fn create_terminal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(input): Json<CreateTerminal>,
) -> Response {
    let window = PtyWindow {
        columns: u64::from(input.columns),
        rows: u64::from(input.rows),
    };
    if !window.within_bounds()
        || !valid_ref(&input.agentide_session_id)
        || !valid_ref(&input.authority_grant_id)
        || !valid_ref(&input.profile_id)
        || input.idempotency_key.trim().is_empty()
        || input.idempotency_key.len() > MAX_MATERIALIZATION_KEY_BYTES
    {
        return problem(StatusCode::UNPROCESSABLE_ENTITY, "terminal_request_invalid");
    }
    let Some(profile) = state.terminal_profiles.get(&input.profile_id).cloned() else {
        return problem(StatusCode::FORBIDDEN, "terminal_profile_not_declared");
    };
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let (coding_session, _, working) =
        match ready_session_materializations(&state, &authority, &session_id).await {
            Ok(materializations) => materializations,
            Err(response) => return response,
        };
    if let Err(response) = verify_terminal_grant(&state, &authority, &coding_session, &input).await
    {
        return response;
    }
    let reservation = match state
        .store
        .reserve_terminal(&authority, &session_id, &input, &profile)
        .await
    {
        Ok(reservation) => reservation,
        Err(error) => return store_problem(&error),
    };
    let terminal = match reservation {
        TerminalReservation::Existing(terminal)
            if terminal.public.state != TerminalState::Preparing =>
        {
            if matches!(
                terminal.public.state,
                TerminalState::Running | TerminalState::Unknown
            ) && let Err(response) = ensure_terminal_broker(&state, &authority, &terminal).await
            {
                return response;
            }
            return confidential(Json(terminal.public).into_response());
        }
        TerminalReservation::New(terminal) | TerminalReservation::Existing(terminal) => terminal,
    };
    let policy = match terminal_policy(&profile) {
        Ok(policy) => policy,
        Err(error) => {
            let _ = state
                .store
                .refuse_terminal(
                    &authority,
                    &terminal.public.id,
                    "terminal_profile_invalid",
                    false,
                )
                .await;
            return substrate_problem(&error);
        }
    };
    let mut builder = working
        .pty_session(&profile.shell, window)
        .args(profile.arguments.clone())
        .allow_environment(b10x_substrate_sdk::BaselineEnvironment::Path)
        .workspace_access(match profile.workspace_access {
            TerminalWorkspaceAccess::ReadOnly => WorkspaceAccess::ReadOnly,
            TerminalWorkspaceAccess::ReadWrite => WorkspaceAccess::ReadWrite,
        })
        .policy(policy)
        .lease(Duration::from_millis(profile.limits.lease_ttl_ms))
        .input_limit_bytes(profile.limits.input_bytes)
        .frame_limit_bytes(profile.limits.frame_bytes)
        .queued_frames(profile.limits.queued_frames)
        .operation_id(substrate_operation_id(
            &session_id,
            &["terminal", &terminal.public.id, "create"],
        ));
    for (name, value) in &profile.environment {
        builder = builder.env(name, value);
    }
    match builder.start().await {
        Ok(session) => {
            let terminal = match state
                .store
                .complete_terminal(
                    &authority,
                    &terminal.public.id,
                    session.id(),
                    &session.observation().exec_id,
                )
                .await
            {
                Ok(terminal) => terminal,
                Err(error) => return store_problem(&error),
            };
            let channel = match session.attach().await {
                Ok(channel) => channel,
                Err(error) => {
                    let _ = state
                        .store
                        .observe_terminal(
                            &authority.tenant_id,
                            &authority.subject,
                            &terminal.public.id,
                            TerminalState::Unknown,
                            None,
                        )
                        .await;
                    return substrate_problem(&error);
                }
            };
            let _broker = register_terminal_broker(&state, &authority, &terminal, channel).await;
            confidential((StatusCode::CREATED, Json(terminal.public)).into_response())
        }
        Err(error) => {
            let unknown = matches!(error, SubstrateError::UnknownOperation { .. });
            let _ = state
                .store
                .refuse_terminal(
                    &authority,
                    &terminal.public.id,
                    if unknown {
                        "terminal_creation_unknown"
                    } else {
                        "terminal_creation_refused"
                    },
                    unknown,
                )
                .await;
            substrate_problem(&error)
        }
    }
}

async fn terminate_terminal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(terminal_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let terminal = match state.store.terminal(&authority, &terminal_id).await {
        Ok(terminal) => terminal,
        Err(error) => return store_problem(&error),
    };
    let coding_session = match state
        .store
        .coding_session(&authority, &terminal.public.coding_session_id)
        .await
    {
        Ok(session) => session,
        Err(error) => return store_problem(&error),
    };
    if let Err(response) = accessible_project(&state, &authority, &coding_session.project_id).await
    {
        return response;
    }
    if terminal.public.state == TerminalState::Terminated {
        return confidential(Json(terminal.public).into_response());
    }
    let Some(substrate_ref) = terminal.substrate_session_ref.as_deref() else {
        return problem(StatusCode::CONFLICT, "terminal_process_unavailable");
    };
    let client = match substrate_client(&state, &authority).await {
        Ok(Some(client)) => client,
        Ok(None) => return problem(StatusCode::SERVICE_UNAVAILABLE, "substrate_unavailable"),
        Err(response) => return response,
    };
    let mut session = match client.get_pipe_session(substrate_ref).await {
        Ok(session) => session,
        Err(SubstrateError::Refusal(refusal)) if refusal.code == "resource.not-found" => {
            let stored = state
                .store
                .complete_terminal_termination(&authority, &terminal_id, None)
                .await;
            state.terminal_replay.remove(&terminal_id).await;
            return match stored {
                Ok(terminal) => confidential(Json(terminal.public).into_response()),
                Err(error) => store_problem(&error),
            };
        }
        Err(error) => return substrate_problem(&error),
    };
    if let Err(error) = session
        .signal_with_operation_id(
            Signal::Kill,
            Duration::from_millis(1),
            Some(substrate_operation_id(
                &terminal.public.coding_session_id,
                &["terminal", &terminal_id, "kill"],
            )),
        )
        .await
    {
        return substrate_problem(&error);
    }
    let exit = terminal_exit(session.observation().exit.as_ref());
    if let Err(error) = session
        .retire_with_operation_id(Some(substrate_operation_id(
            &terminal.public.coding_session_id,
            &["terminal", &terminal_id, "retire"],
        )))
        .await
    {
        return substrate_problem(&error);
    }
    state.terminal_replay.remove(&terminal_id).await;
    match state
        .store
        .complete_terminal_termination(&authority, &terminal_id, exit.as_ref())
        .await
    {
        Ok(terminal) => confidential(Json(terminal.public).into_response()),
        Err(error) => store_problem(&error),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TerminalAttachQuery {
    from_sequence: Option<u64>,
}

async fn attach_terminal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(terminal_id): Path<String>,
    query: Result<Query<TerminalAttachQuery>, axum::extract::rejection::QueryRejection>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Ok(Query(query)) = query else {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "terminal_replay_cursor_invalid",
        );
    };
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let terminal = match state.store.terminal(&authority, &terminal_id).await {
        Ok(terminal) => terminal,
        Err(error) => return store_problem(&error),
    };
    if !matches!(
        terminal.public.state,
        TerminalState::Running | TerminalState::Unknown
    ) {
        return problem(StatusCode::CONFLICT, "terminal_not_running");
    }
    let coding_session = match state
        .store
        .coding_session(&authority, &terminal.public.coding_session_id)
        .await
    {
        Ok(session) => session,
        Err(error) => return store_problem(&error),
    };
    if let Err(response) = accessible_project(&state, &authority, &coding_session.project_id).await
    {
        return response;
    }
    let grant_input = CreateTerminal {
        agentide_session_id: terminal.public.agentide_session_id.clone(),
        authority_grant_id: terminal.public.authority_grant_id.clone(),
        profile_id: terminal.public.profile.id.clone(),
        columns: terminal.initial_columns,
        rows: terminal.initial_rows,
        idempotency_key: "attachment-revalidation".to_owned(),
    };
    if let Err(response) =
        verify_terminal_grant(&state, &authority, &coding_session, &grant_input).await
    {
        return response;
    }
    let broker = match ensure_terminal_broker(&state, &authority, &terminal).await {
        Ok(broker) => broker,
        Err(response) => return response,
    };
    let maximum_frame_bytes =
        usize::try_from(b10x_substrate_sdk::MAX_SESSION_FRAME_BYTES).unwrap_or(usize::MAX);
    upgrade
        .max_frame_size(maximum_frame_bytes)
        .max_message_size(maximum_frame_bytes)
        .on_upgrade(move |socket| {
            terminal_websocket(socket, state, terminal.public, query.from_sequence, broker)
        })
        .into_response()
}

async fn ensure_terminal_broker(
    state: &AppState,
    authority: &Authority,
    terminal: &StoredTerminal,
) -> Result<TerminalBroker, Response> {
    if let Some(broker) = state.terminal_brokers.get(&terminal.public.id).await {
        return Ok(broker);
    }
    let Some(substrate_ref) = terminal.substrate_session_ref.as_deref() else {
        return Err(problem(
            StatusCode::CONFLICT,
            "terminal_process_unavailable",
        ));
    };
    let client = match substrate_client(state, authority).await {
        Ok(Some(client)) => client,
        Ok(None) => {
            return Err(problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "substrate_unavailable",
            ));
        }
        Err(response) => return Err(response),
    };
    let session = client
        .get_pipe_session(substrate_ref)
        .await
        .map_err(|error| substrate_problem(&error))?;
    if matches!(
        session.observation().state,
        PipeSessionState::Exited | PipeSessionState::Cancelled | PipeSessionState::Expired
    ) {
        let exit = terminal_exit(session.observation().exit.as_ref());
        let _ = state
            .store
            .observe_terminal(
                &authority.tenant_id,
                &authority.subject,
                &terminal.public.id,
                TerminalState::Exited,
                exit.as_ref(),
            )
            .await;
        return Err(problem(StatusCode::CONFLICT, "terminal_not_running"));
    }
    if matches!(
        session.observation().attachment,
        b10x_substrate_sdk::SessionAttachmentState::Attached
            | b10x_substrate_sdk::SessionAttachmentState::Consumed
            | b10x_substrate_sdk::SessionAttachmentState::Uncertain
    ) {
        if let Some(broker) = state.terminal_brokers.get(&terminal.public.id).await {
            return Ok(broker);
        }
        let _ = state
            .store
            .observe_terminal(
                &authority.tenant_id,
                &authority.subject,
                &terminal.public.id,
                TerminalState::Unknown,
                None,
            )
            .await;
        return Err(problem(
            StatusCode::CONFLICT,
            "terminal_attachment_unrecoverable",
        ));
    }
    let channel = match session.attach().await {
        Ok(channel) => channel,
        Err(error) => {
            if let Some(broker) = state.terminal_brokers.get(&terminal.public.id).await {
                return Ok(broker);
            }
            return Err(substrate_problem(&error));
        }
    };
    Ok(register_terminal_broker(state, authority, terminal, channel).await)
}

async fn register_terminal_broker(
    state: &AppState,
    authority: &Authority,
    terminal: &StoredTerminal,
    channel: PipeChannel,
) -> TerminalBroker {
    let (candidate, commands) = TerminalBroker::pair();
    if let Err(existing) = state
        .terminal_brokers
        .insert(&terminal.public.id, candidate.clone())
        .await
    {
        return existing;
    }
    let state = state.clone();
    let terminal = terminal.public.clone();
    let tenant_id = authority.tenant_id.clone();
    let owner_subject = authority.subject.clone();
    let broker = candidate.clone();
    tokio::spawn(async move {
        run_terminal_broker(
            state,
            terminal,
            tenant_id,
            owner_subject,
            broker,
            commands,
            channel,
        )
        .await;
    });
    candidate
}

#[allow(clippy::too_many_lines)] // The select loop keeps every terminal event and terminal transition adjacent.
async fn run_terminal_broker(
    state: AppState,
    terminal: TerminalSession,
    tenant_id: String,
    owner_subject: String,
    broker: TerminalBroker,
    mut commands: tokio::sync::mpsc::Receiver<TerminalBrokerCommand>,
    mut channel: PipeChannel,
) {
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                let result = match command {
                    TerminalBrokerCommand::Input(bytes) => channel.write(bytes).await,
                    TerminalBrokerCommand::Resize { columns, rows } => {
                        channel.resize(PtyWindow { columns, rows }).await
                    }
                    TerminalBrokerCommand::Signal { signal, grace_ms } => {
                        let signal = match signal.as_str() {
                            "INT" => Signal::Interrupt,
                            "TERM" => Signal::Terminate,
                            "KILL" => Signal::Kill,
                            _ => unreachable!("browser controls are validated before the broker"),
                        };
                        channel.signal(signal, Duration::from_millis(grace_ms)).await
                    }
                };
                if result.is_err() {
                    let _ = state.store.observe_terminal(
                        &tenant_id,
                        &owner_subject,
                        &terminal.id,
                        TerminalState::Unknown,
                        None,
                    ).await;
                    broker.publish(TerminalBrokerEvent::Detached {
                        code: "terminal_transport_unavailable".to_owned(),
                    });
                    break;
                }
            }
            substrate = channel.next_frame() => {
                match substrate {
                    Ok(Some(PipeFrame::Output { bytes, .. })) => {
                        let frame = state.terminal_replay.push(&terminal.id, &bytes).await;
                        broker.publish(TerminalBrokerEvent::Output(frame));
                    }
                    Ok(Some(PipeFrame::Exit { state: observed, exit, .. })) => {
                        let exit = terminal_exit(exit.as_ref());
                        let _ = state.store.observe_terminal(
                            &tenant_id,
                            &owner_subject,
                            &terminal.id,
                            TerminalState::Exited,
                            exit.as_ref(),
                        ).await;
                        broker.publish(TerminalBrokerEvent::Exit {
                            observed_state: format!("{observed:?}").to_ascii_lowercase(),
                            exit,
                        });
                        break;
                    }
                    Ok(Some(PipeFrame::ProtocolError { code, .. })) => {
                        let _ = state.store.observe_terminal(
                            &tenant_id,
                            &owner_subject,
                            &terminal.id,
                            TerminalState::Unknown,
                            None,
                        ).await;
                        broker.publish(TerminalBrokerEvent::Refused {
                            code: "terminal_protocol_refused".to_owned(),
                            substrate_code: Some(code),
                        });
                        break;
                    }
                    Ok(Some(_)) => {
                        let _ = state.store.observe_terminal(
                            &tenant_id,
                            &owner_subject,
                            &terminal.id,
                            TerminalState::Unknown,
                            None,
                        ).await;
                        broker.publish(TerminalBrokerEvent::Refused {
                            code: "terminal_frame_unsupported".to_owned(),
                            substrate_code: None,
                        });
                        break;
                    }
                    Ok(None) | Err(_) => {
                        let _ = state.store.observe_terminal(
                            &tenant_id,
                            &owner_subject,
                            &terminal.id,
                            TerminalState::Unknown,
                            None,
                        ).await;
                        broker.publish(TerminalBrokerEvent::Detached {
                            code: "terminal_transport_unavailable".to_owned(),
                        });
                        break;
                    }
                }
            }
        }
    }
    let _ = channel.close().await;
    state.terminal_brokers.remove(&terminal.id).await;
    state.terminal_replay.remove(&terminal.id).await;
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TerminalControl {
    Resize { columns: u64, rows: u64 },
    Signal { signal: String, grace_ms: u64 },
}

#[allow(clippy::too_many_lines)] // The closed browser and broker frame vocabularies remain one auditable loop.
async fn terminal_websocket(
    mut socket: WebSocket,
    state: AppState,
    terminal: TerminalSession,
    from_sequence: Option<u64>,
    broker: TerminalBroker,
) {
    let mut events = broker.subscribe();
    let replay = state
        .terminal_replay
        .replay(&terminal.id, from_sequence)
        .await;
    if !send_terminal_json(
        &mut socket,
        serde_json::json!({
            "kind": "attached",
            "terminal": terminal,
            "replay": {
                "earliest_sequence": replay.earliest_sequence,
                "latest_sequence": replay.latest_sequence,
                "complete": replay.complete,
            }
        }),
    )
    .await
    {
        return;
    }
    for frame in replay.frames {
        if !send_terminal_output(&mut socket, frame.sequence, &frame.bytes).await {
            return;
        }
    }
    loop {
        tokio::select! {
            browser = socket.recv() => {
                let Some(browser) = browser else { break };
                let Ok(browser) = browser else { break };
                match browser {
                    WebSocketMessage::Binary(bytes) => {
                        if bytes.is_empty()
                            || u64::try_from(bytes.len()).map_or(
                                true,
                                |length| length > terminal.profile.limits.frame_bytes,
                            )
                            || broker.command(TerminalBrokerCommand::Input(bytes)).await.is_err()
                        {
                            let _ = send_terminal_json(&mut socket, serde_json::json!({
                                "kind": "refused",
                                "code": "terminal_input_refused"
                            })).await;
                            break;
                        }
                    }
                    WebSocketMessage::Text(text) => {
                        let control = serde_json::from_str::<TerminalControl>(&text);
                        let accepted = match control {
                            Ok(TerminalControl::Resize { columns, rows }) => {
                                let window = PtyWindow { columns, rows };
                                if window.within_bounds() {
                                    broker.command(TerminalBrokerCommand::Resize { columns, rows }).await.is_ok()
                                } else {
                                    false
                                }
                            }
                            Ok(TerminalControl::Signal { signal, grace_ms }) if grace_ms <= 30_000 => {
                                if matches!(signal.as_str(), "INT" | "TERM" | "KILL") {
                                    broker.command(TerminalBrokerCommand::Signal { signal, grace_ms }).await.is_ok()
                                } else {
                                    false
                                }
                            }
                            _ => false,
                        };
                        if !accepted {
                            let _ = send_terminal_json(&mut socket, serde_json::json!({
                                "kind": "refused",
                                "code": "terminal_control_refused"
                            })).await;
                        }
                    }
                    WebSocketMessage::Close(_) => break,
                    WebSocketMessage::Ping(_) | WebSocketMessage::Pong(_) => {}
                }
            }
            event = events.recv() => {
                match event {
                    Ok(TerminalBrokerEvent::Output(frame)) => {
                        if !send_terminal_output(&mut socket, frame.sequence, &frame.bytes).await {
                            break;
                        }
                    }
                    Ok(TerminalBrokerEvent::Exit { observed_state, exit }) => {
                        let _ = send_terminal_json(&mut socket, serde_json::json!({
                            "kind": "exit",
                            "state": observed_state,
                            "exit": exit,
                        })).await;
                        break;
                    }
                    Ok(TerminalBrokerEvent::Refused { code, substrate_code }) => {
                        if !send_terminal_json(&mut socket, serde_json::json!({
                            "kind": "refused",
                            "code": code,
                            "substrate_code": substrate_code
                        })).await {
                            break;
                        }
                        break;
                    }
                    Ok(TerminalBrokerEvent::Detached { code }) => {
                        let _ = send_terminal_json(&mut socket, serde_json::json!({
                            "kind": "detached",
                            "code": code
                        })).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let _ = send_terminal_json(&mut socket, serde_json::json!({
                            "kind": "refused",
                            "code": "terminal_slow_reader"
                        })).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    // Dropping this browser's broker clone never signals or retires the Substrate process.
}

async fn send_terminal_output(socket: &mut WebSocket, sequence: u64, bytes: &[u8]) -> bool {
    let mut framed = Vec::with_capacity(8 + bytes.len());
    framed.extend_from_slice(&sequence.to_be_bytes());
    framed.extend_from_slice(bytes);
    tokio::time::timeout(
        Duration::from_secs(2),
        socket.send(WebSocketMessage::Binary(framed.into())),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

async fn send_terminal_json(socket: &mut WebSocket, value: Value) -> bool {
    tokio::time::timeout(
        Duration::from_secs(2),
        socket.send(WebSocketMessage::Text(value.to_string().into())),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

fn terminal_policy(profile: &TerminalProfile) -> Result<ExecutionPolicy, SubstrateError> {
    ExecutionPolicy::builder()
        .timeout(Duration::from_millis(profile.limits.timeout_ms))
        .cpu_time(Duration::from_millis(profile.limits.cpu_millis))
        .memory_bytes(profile.limits.memory_bytes)
        .processes(profile.limits.processes)
        .output_bytes(profile.limits.output_bytes)
        .build()
}

fn terminal_exit(exit: Option<&b10x_substrate_sdk::ExecExit>) -> Option<TerminalExit> {
    exit.map(|exit| TerminalExit {
        code: exit.code.map(i32::from),
        signal: exit.signal.map(|signal| match signal {
            Signal::Interrupt => "INT".to_owned(),
            Signal::Terminate => "TERM".to_owned(),
            Signal::Kill => "KILL".to_owned(),
        }),
    })
}

async fn verify_terminal_grant(
    state: &AppState,
    authority: &Authority,
    coding_session: &CodingSession,
    input: &CreateTerminal,
) -> Result<(), Response> {
    let session_output = invoke_unique_operation(
        state,
        authority,
        "agentide.get_session",
        serde_json::json!({
            "session_id": input.agentide_session_id,
            "$page": {"limit": 2}
        }),
    )
    .await
    .map_err(|_| {
        problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "terminal_authority_unavailable",
        )
    })?;
    let session_rows = service_rows(&session_output)
        .ok_or_else(|| problem(StatusCode::BAD_GATEWAY, "terminal_authority_invalid"))?;
    let session_matches = session_rows.iter().any(|row| {
        terminal_session_row_matches(row, authority, coding_session, &input.agentide_session_id)
    });
    if !session_matches {
        return Err(problem(
            StatusCode::FORBIDDEN,
            "terminal_session_binding_refused",
        ));
    }
    let grants_output = invoke_unique_operation(
        state,
        authority,
        "agentide.list_grants",
        serde_json::json!({
            "session_id": input.agentide_session_id,
            "$page": {"limit": 1000}
        }),
    )
    .await
    .map_err(|_| {
        problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "terminal_authority_unavailable",
        )
    })?;
    let grant_rows = service_rows(&grants_output)
        .ok_or_else(|| problem(StatusCode::BAD_GATEWAY, "terminal_authority_invalid"))?;
    let now = Utc::now();
    let granted = grant_rows.iter().any(|row| {
        terminal_grant_row_matches(
            row,
            authority,
            &input.agentide_session_id,
            &input.authority_grant_id,
            now,
        )
    });
    if granted {
        Ok(())
    } else {
        Err(problem(
            StatusCode::FORBIDDEN,
            "interactive_terminal_grant_required",
        ))
    }
}

fn terminal_session_row_matches(
    row: &Value,
    authority: &Authority,
    coding_session: &CodingSession,
    agentide_session_id: &str,
) -> bool {
    row.get("session_id").and_then(Value::as_str) == Some(agentide_session_id)
        && row.get("workspace_root").and_then(Value::as_str)
            == coding_session.working_materialization_ref.as_deref()
        && row.get("workspace_session_id").and_then(Value::as_str)
            == Some(coding_session.id.as_str())
        && row.get("project_id").and_then(Value::as_str) == Some(coding_session.project_id.as_str())
        && row.get("source_revision").and_then(Value::as_str)
            == Some(coding_session.source_revision.as_str())
        && row.get("manifest_digest").and_then(Value::as_str)
            == coding_session.manifest_sha256.as_deref()
        && row.get("owner").and_then(Value::as_str) == Some(authority.subject.as_str())
        && row.get("state").and_then(Value::as_str) == Some("Active")
}

fn terminal_grant_row_matches(
    row: &Value,
    authority: &Authority,
    agentide_session_id: &str,
    grant_id: &str,
    now: DateTime<Utc>,
) -> bool {
    row.get("grant_id").and_then(Value::as_str) == Some(grant_id)
        && row.get("session_id").and_then(Value::as_str) == Some(agentide_session_id)
        && row.get("grantee").and_then(Value::as_str) == Some(authority.subject.as_str())
        && row.get("state").and_then(Value::as_str) == Some("Active")
        && row.get("maximum_risk").and_then(Value::as_str) == Some("Medium")
        && row
            .get("allowed_intents")
            .and_then(Value::as_array)
            .is_some_and(|intents| {
                intents
                    .iter()
                    .any(|intent| intent.as_str() == Some("interactive_terminal"))
            })
        && row
            .get("path_prefixes")
            .and_then(Value::as_array)
            .is_some_and(|prefixes| prefixes.iter().any(|prefix| prefix.as_str() == Some("")))
        && match row.get("expires_at") {
            None | Some(Value::Null) => true,
            Some(Value::String(expires_at)) => {
                DateTime::parse_from_rfc3339(expires_at).is_ok_and(|expires_at| expires_at > now)
            }
            Some(_) => false,
        }
}

fn service_rows(output: &Value) -> Option<Vec<&Value>> {
    output
        .as_array()
        .or_else(|| output.get("items").and_then(Value::as_array))
        .or_else(|| output.get("rows").and_then(Value::as_array))
        .map(|rows| rows.iter().collect())
}

async fn invoke_unique_operation(
    state: &AppState,
    authority: &Authority,
    operation_ref: &str,
    input: Value,
) -> Result<Value, Response> {
    let described = connector_operation(
        state,
        authority,
        operation::OperationRequest::Describe(operation::DescribeRequest {
            operation_ref: operation_ref.to_owned(),
        }),
    )
    .await?;
    let operation::OperationResult::Describe(description) = described else {
        return Err(problem(
            StatusCode::BAD_GATEWAY,
            "connector_protocol_invalid",
        ));
    };
    let [connection] = description.connections.as_slice() else {
        return Err(problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_operation_binding_invalid",
        ));
    };
    invoke_operation(
        state,
        authority,
        operation::InvokeRequest {
            operation_ref: operation_ref.to_owned(),
            connection_ref: connection.connection_ref.clone(),
            description_ref: description.description_ref,
            input,
            approval_evidence_ref: None,
        },
    )
    .await
}

async fn close_coding_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let session = match state
        .store
        .begin_close_coding_session(&authority, &session_id)
        .await
    {
        Ok(session) => session,
        Err(error) => return store_problem(&error),
    };
    if session.state == CodingSessionState::Closed {
        return confidential(Json(public_session(session)).into_response());
    }
    let client = match substrate_client(&state, &authority).await {
        Ok(Some(client)) => client,
        Ok(None) => {
            return problem(StatusCode::SERVICE_UNAVAILABLE, "substrate_unavailable");
        }
        Err(response) => return response,
    };
    let terminal_cleanup_unknown = cleanup_terminals(&state, &authority, &client, &session).await;
    let cleanup_unknown =
        terminal_cleanup_unknown || cleanup_materializations(&client, &session).await;
    let stored = if cleanup_unknown {
        state
            .store
            .mark_close_unknown(&authority, &session.id)
            .await
    } else {
        state
            .store
            .complete_close_coding_session(&authority, &session.id)
            .await
    };
    match stored {
        Ok(session) => confidential(Json(public_session(session)).into_response()),
        Err(error) => store_problem(&error),
    }
}

async fn create_coding_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(input): Json<CreateCodingSession>,
) -> Response {
    if !valid_commit(&input.source_revision)
        || input.idempotency_key.trim().is_empty()
        || input.idempotency_key.len() > MAX_MATERIALIZATION_KEY_BYTES
    {
        return problem(StatusCode::UNPROCESSABLE_ENTITY, "coding_session_invalid");
    }
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let project = match accessible_project(&state, &authority, &project_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    if project.pinned_commit.as_deref() != Some(input.source_revision.as_str()) {
        return problem(StatusCode::CONFLICT, "project_snapshot_stale");
    }
    let client = match substrate_client(&state, &authority).await {
        Ok(Some(client)) => client,
        Ok(None) => {
            return problem(StatusCode::SERVICE_UNAVAILABLE, "substrate_unavailable");
        }
        Err(response) => return response,
    };
    let reservation = match state
        .store
        .reserve_coding_session(
            &authority,
            &project_id,
            &input,
            SOURCE_MATERIALIZATION_LIMITS,
        )
        .await
    {
        Ok(reservation) => reservation,
        Err(error) => return store_problem(&error),
    };
    let session = match reservation {
        SessionReservation::Existing(session) if session.state != CodingSessionState::Preparing => {
            return confidential(Json(public_session(session)).into_response());
        }
        SessionReservation::New(session) | SessionReservation::Existing(session) => session,
    };

    spawn_coding_session_materialization(state, authority, project, client, session.clone()).await;
    confidential((StatusCode::ACCEPTED, Json(public_session(session))).into_response())
}

async fn spawn_coding_session_materialization(
    state: AppState,
    authority: Authority,
    project: Project,
    client: SubstrateClient,
    session: CodingSession,
) {
    if !state.materialization_workers.begin(&session.id).await {
        return;
    }
    tokio::spawn(async move {
        Box::pin(prepare_coding_session_materialization(
            state, authority, project, client, session,
        ))
        .await;
    });
}

async fn prepare_coding_session_materialization(
    state: AppState,
    authority: Authority,
    project: Project,
    client: SubstrateClient,
    session: CodingSession,
) {
    let session_id = session.id.clone();
    if let Ok(files) = collect_source_files(&state, &authority, &project).await {
        if let Err((failed, _)) = Box::pin(provision_materializations(
            state.clone(),
            authority.clone(),
            client.clone(),
            session,
            files,
        ))
        .await
        {
            let cleanup_unknown =
                cleanup_materializations_owned(client.clone(), failed.clone()).await;
            record_materialization_refusal(
                state.store.clone(),
                authority.clone(),
                failed.id,
                if cleanup_unknown {
                    "materialization_cleanup_unknown"
                } else {
                    "substrate_materialization_refused"
                },
                cleanup_unknown,
            )
            .await;
        }
    } else {
        let cleanup_unknown = cleanup_materializations_owned(client, session.clone()).await;
        record_materialization_refusal(
            state.store.clone(),
            authority,
            session.id,
            if cleanup_unknown {
                "materialization_cleanup_unknown"
            } else {
                "source_materialization_refused"
            },
            cleanup_unknown,
        )
        .await;
    }
    state.materialization_workers.finish(&session_id).await;
}

async fn record_materialization_refusal(
    store: Store,
    authority: Authority,
    session_id: String,
    failure_code: &'static str,
    cleanup_unknown: bool,
) {
    let _ = store
        .refuse_coding_session(&authority, &session_id, failure_code, cleanup_unknown)
        .await;
}

async fn threads(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    if let Err(response) = accessible_project(&state, &authority, &project_id).await {
        return response;
    }
    match state.store.threads(&authority, &project_id).await {
        Ok(threads) => confidential(Json(threads).into_response()),
        Err(error) => store_problem(&error),
    }
}

async fn create_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(input): Json<CreateThread>,
) -> Response {
    if !valid_branch(&input.branch)
        || !valid_commit(&input.pinned_commit)
        || input.title.trim().is_empty()
        || input.title.len() > 160
    {
        return problem(StatusCode::UNPROCESSABLE_ENTITY, "thread_invalid");
    }
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let project = match accessible_project(&state, &authority, &project_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    if project.selected_branch != input.branch
        || project.pinned_commit.as_deref() != Some(input.pinned_commit.as_str())
    {
        return problem(StatusCode::CONFLICT, "project_snapshot_stale");
    }
    match state
        .store
        .create_thread(&authority, &project_id, &input)
        .await
    {
        Ok(thread) => confidential(Json(thread).into_response()),
        Err(error) => store_problem(&error),
    }
}

async fn messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    match state.store.messages(&authority, &thread_id).await {
        Ok(messages) => confidential(Json(messages).into_response()),
        Err(error) => store_problem(&error),
    }
}

#[allow(clippy::too_many_lines)] // Context resolution, durable append and task admission remain visibly ordered.
async fn create_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
    Json(input): Json<CreateMessage>,
) -> Response {
    if input.content.trim().is_empty() || input.content.len() > 32 * 1024 {
        return problem(StatusCode::UNPROCESSABLE_ENTITY, "message_invalid");
    }
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let Some(agent_platform) = state.agent_platform.as_ref() else {
        return problem(StatusCode::SERVICE_UNAVAILABLE, "project_agent_unavailable");
    };
    let Some(agent_platform_bearer) = authority.agent_platform_bearer.as_deref() else {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "project_agent_authority_unavailable",
        );
    };
    let thread = match state.store.thread(&authority, &thread_id).await {
        Ok(thread) => thread,
        Err(error) => return store_problem(&error),
    };
    let project = match accessible_project(&state, &authority, &thread.project_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    if project.selected_branch != thread.branch
        || project.pinned_commit.as_deref() != Some(thread.pinned_commit.as_str())
    {
        return problem(StatusCode::CONFLICT, "thread_snapshot_stale");
    }
    let context = match project_context(&state, &authority, &project).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let agent_id = match ensure_project_agent(
        &state,
        agent_platform,
        agent_platform_bearer,
        &authority,
        &project,
    )
    .await
    {
        Ok(agent_id) => agent_id,
        Err(response) => return response,
    };
    let prior = match state.store.messages(&authority, &thread_id).await {
        Ok(messages) => messages,
        Err(error) => return store_problem(&error),
    };
    let message = match state
        .store
        .create_message(&authority, &thread_id, &input)
        .await
    {
        Ok(message) => message,
        Err(error) => return store_problem(&error),
    };
    let conversation = ConversationInput::ProjectConversation {
        prompt: input.content,
        messages: prior
            .into_iter()
            .map(|message| ConversationMessage {
                role: match message.role {
                    MessageRole::User => ConversationRole::User,
                    MessageRole::Assistant => ConversationRole::Assistant,
                    MessageRole::System => ConversationRole::System,
                },
                content: message.content,
            })
            .collect(),
        context,
    };
    let task = SubmitTask {
        agent_id,
        idempotency_key: format!("{}:{}", thread.id, message.sequence),
        input: match serde_json::to_value(conversation) {
            Ok(input) => input,
            Err(_) => {
                return problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "project_context_unavailable",
                );
            }
        },
    };
    match agent_platform
        .submit_task(agent_platform_bearer, &task)
        .await
    {
        Ok(task) => {
            if let Err(error) = state
                .store
                .record_message_task(&authority, &thread.id, message.sequence, task.id.as_str())
                .await
            {
                return store_problem(&error);
            }
            spawn_task_completion(
                state.store.clone(),
                agent_platform.clone(),
                agent_platform_bearer.to_owned(),
                authority.tenant_id,
                authority.subject,
                thread.id,
                task.id,
            );
        }
        Err(_) => {
            let _ = state
                .store
                .append_agent_message(
                    &authority.tenant_id,
                    &authority.subject,
                    &thread.id,
                    MessageRole::System,
                    "The project agent refused this turn before execution.",
                )
                .await;
        }
    }
    confidential(Json(message).into_response())
}

async fn message_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((thread_id, message_sequence)): Path<(String, u64)>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let Some(agent_platform) = state.agent_platform.as_ref() else {
        return problem(StatusCode::SERVICE_UNAVAILABLE, "project_agent_unavailable");
    };
    let Some(agent_platform_bearer) = authority.agent_platform_bearer.as_deref() else {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "project_agent_authority_unavailable",
        );
    };
    let task_id = match state
        .store
        .message_task(&authority, &thread_id, message_sequence)
        .await
    {
        Ok(task_id) => task_id,
        Err(error) => return store_problem(&error),
    };
    let Ok(task_id) = TaskId::new(task_id) else {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "project_agent_task_invalid",
        );
    };
    let Ok(upstream) = agent_platform
        .task_events(agent_platform_bearer, &task_id)
        .await
    else {
        return problem(StatusCode::BAD_GATEWAY, "project_agent_stream_unavailable");
    };
    let mut response = Response::new(Body::from_stream(upstream.bytes_stream()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/event-stream"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    confidential(response)
}

async fn workflows(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    if let Err(response) = accessible_project(&state, &authority, &project_id).await {
        return response;
    }
    confidential(Json(workflow_definitions()).into_response())
}

async fn engineering_artifacts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let project = match accessible_project(&state, &authority, &project_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let Some(configuration) = state.aep.as_ref() else {
        return problem(StatusCode::SERVICE_UNAVAILABLE, "aep_service_unavailable");
    };
    let Ok(credentials) = RequestCredential::from_authorization(&authority.session_authorization)
    else {
        return problem(StatusCode::UNAUTHORIZED, "authentication_refused");
    };
    let Ok(client) = AepClient::new(
        configuration.transport.clone(),
        credentials,
        &configuration.realm,
        &configuration.workspace,
    ) else {
        return problem(StatusCode::SERVICE_UNAVAILABLE, "aep_service_unavailable");
    };
    let query = EntityQuery {
        space: Some(project.id),
        limit: Some(100),
        ..EntityQuery::default()
    };
    let Ok(page) = client.query(&query).await else {
        return problem(StatusCode::BAD_GATEWAY, "aep_query_refused");
    };
    let artifacts = page
        .items
        .into_iter()
        .map(|entity| {
            let body = entity.data.as_map();
            let title = body
                .and_then(|body| body.get("title").or_else(|| body.get("name")))
                .and_then(|value| value.as_text())
                .map(str::to_owned);
            let status = body
                .and_then(|body| body.get("status").or_else(|| body.get("state")))
                .and_then(|value| value.as_text())
                .map(str::to_owned);
            EngineeringArtifact {
                id: entity.metadata.id.to_string(),
                locator: entity.metadata.locator.to_string(),
                entity_type: entity.metadata.entity_type.to_string(),
                revision: entity.metadata.revision.get(),
                title,
                status,
                updated_at_ms: entity.metadata.updated_at.epoch_millis(),
                source_revision: entity
                    .metadata
                    .provenance
                    .source_revision
                    .map(|revision| revision.to_string()),
            }
        })
        .collect();
    confidential(
        Json(EngineeringArtifactPage {
            artifacts,
            has_more: page.next.is_some(),
        })
        .into_response(),
    )
}

#[allow(clippy::too_many_lines)] // Validation, context binding, dispatch and durable admission stay visibly ordered.
async fn start_workflow(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(input): Json<StartWorkflow>,
) -> Response {
    if !workflow_definitions()
        .iter()
        .any(|definition| definition.id == input.definition_id)
        || !valid_branch(&input.branch)
        || !valid_commit(&input.commit)
        || !valid_ref(&input.idempotency_key)
    {
        return problem(StatusCode::UNPROCESSABLE_ENTITY, "workflow_run_invalid");
    }
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    let Some(agent_platform) = state.agent_platform.as_ref() else {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "workflow_executor_unavailable",
        );
    };
    let Some(agent_platform_bearer) = authority.agent_platform_bearer.as_deref() else {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "workflow_authority_unavailable",
        );
    };
    let project = match accessible_project(&state, &authority, &project_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    if project.selected_branch != input.branch
        || project.pinned_commit.as_deref() != Some(input.commit.as_str())
    {
        return problem(StatusCode::CONFLICT, "project_snapshot_stale");
    }
    let context = match project_context(&state, &authority, &project).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let agent_id = match ensure_project_agent(
        &state,
        agent_platform,
        agent_platform_bearer,
        &authority,
        &project,
    )
    .await
    {
        Ok(agent_id) => agent_id,
        Err(response) => return response,
    };
    let run = match state
        .store
        .start_workflow(&authority, &project_id, &input)
        .await
    {
        Ok(run) => run,
        Err(error) => return store_problem(&error),
    };
    if matches!(
        run.state,
        WorkflowRunState::Succeeded | WorkflowRunState::Failed | WorkflowRunState::Refused
    ) {
        return confidential(Json(run).into_response());
    }
    match state.store.workflow_task(&authority, &run.id).await {
        Ok(Some(task_id)) => {
            let Ok(task_id) = TaskId::new(task_id) else {
                let _ = state
                    .store
                    .update_workflow_run(
                        &authority.tenant_id,
                        &authority.subject,
                        &run.id,
                        WorkflowRunState::Failed,
                        Some("workflow_task_invalid"),
                        None,
                    )
                    .await;
                return problem(StatusCode::SERVICE_UNAVAILABLE, "workflow_task_invalid");
            };
            spawn_workflow_completion(WorkflowObservation {
                store: state.store.clone(),
                client: agent_platform.clone(),
                observers: state.workflow_observers.clone(),
                bearer: agent_platform_bearer.to_owned(),
                tenant_id: authority.tenant_id,
                subject: authority.subject,
                run_id: run.id.clone(),
                task_id,
            })
            .await;
            return confidential(Json(run).into_response());
        }
        Ok(None) => {}
        Err(error) => return store_problem(&error),
    }
    let task = SubmitTask {
        agent_id,
        idempotency_key: format!("workspace-workflow:{}", run.id),
        input: match serde_json::to_value(ConversationInput::ProjectConversation {
            prompt: workflow_prompt(&input.definition_id).to_owned(),
            messages: Vec::new(),
            context,
        }) {
            Ok(input) => input,
            Err(_) => {
                return problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "workflow_context_unavailable",
                );
            }
        },
    };
    let Ok(task) = submit_workflow_task(agent_platform, agent_platform_bearer, &task).await else {
        return problem(StatusCode::BAD_GATEWAY, "workflow_dispatch_refused");
    };
    if let Err(error) = state
        .store
        .record_workflow_task(&authority, &run.id, task.id.as_str())
        .await
    {
        return store_problem(&error);
    }
    spawn_workflow_completion(WorkflowObservation {
        store: state.store.clone(),
        client: agent_platform.clone(),
        observers: state.workflow_observers.clone(),
        bearer: agent_platform_bearer.to_owned(),
        tenant_id: authority.tenant_id,
        subject: authority.subject,
        run_id: run.id.clone(),
        task_id: task.id,
    })
    .await;
    confidential(Json(run).into_response())
}

async fn workflow_runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    if let Err(response) = accessible_project(&state, &authority, &project_id).await {
        return response;
    }
    if let Err(error) = resume_workflow_completions(&state, &authority, &project_id).await {
        return store_problem(&error);
    }
    match state.store.workflow_runs(&authority, &project_id).await {
        Ok(runs) => confidential(Json(runs).into_response()),
        Err(error) => store_problem(&error),
    }
}

async fn accessible_project(
    state: &AppState,
    authority: &Authority,
    project_id: &str,
) -> Result<Project, Response> {
    let project = state
        .store
        .project(authority, project_id)
        .await
        .map_err(|error| store_problem(&error))?;
    reachable_project(
        state,
        authority,
        &project.forge_instance_ref,
        &project.project_ref,
    )
    .await?;
    state
        .store
        .record_access(authority, project_id)
        .await
        .map_err(|error| store_problem(&error))?;
    Ok(project)
}

#[derive(Clone)]
struct Authority {
    tenant_id: String,
    subject: String,
    connector_bearer: String,
    agent_platform_bearer: Option<String>,
    session_authorization: String,
    context: OwnerContext,
}

async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<Authority, Response> {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.starts_with("Bearer ") && value.len() <= 8 * 1024)
        .ok_or_else(|| problem(StatusCode::UNAUTHORIZED, "authentication_required"))?;
    let session = state
        .identity
        .resolve_session(authorization)
        .await
        .map_err(|_| problem(StatusCode::UNAUTHORIZED, "authentication_refused"))?;
    authority(state, authorization, session).await
}

async fn substrate_client(
    state: &AppState,
    authority: &Authority,
) -> Result<Option<SubstrateClient>, Response> {
    let Some(configuration) = state.substrate.as_ref() else {
        return Ok(None);
    };
    let identity = state.identity.clone();
    let authorization = authority.session_authorization.clone();
    SubstrateClient::builder()
        .https_endpoint(&configuration.origin)
        .trust_roots(&configuration.ca_bundle)
        .server_identity(&configuration.server_identity)
        .token_provider(move |_| {
            let identity = identity.clone();
            let authorization = authorization.clone();
            async move {
                let access = identity
                    .issue_access_token(&authorization, SUBSTRATE_AUDIENCE, SUBSTRATE_SCOPE)
                    .await
                    .map_err(|_| SubstrateError::TokenUnavailable)?;
                AccessToken::new(
                    access
                        .credential
                        .expose_at_authorization_boundary()
                        .to_owned(),
                )
            }
        })
        .connect()
        .await
        .map(Some)
        .map_err(|_| problem(StatusCode::SERVICE_UNAVAILABLE, "substrate_unavailable"))
}

async fn ready_session_materializations(
    state: &AppState,
    authority: &Authority,
    session_id: &str,
) -> Result<(CodingSession, SubstrateWorkspace, SubstrateWorkspace), Response> {
    let session = state
        .store
        .coding_session(authority, session_id)
        .await
        .map_err(|error| store_problem(&error))?;
    accessible_project(state, authority, &session.project_id).await?;
    if session.state != CodingSessionState::Ready {
        return Err(problem(StatusCode::CONFLICT, "coding_session_not_ready"));
    }
    let base_ref = session.base_materialization_ref.as_deref().ok_or_else(|| {
        problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "coding_session_inconsistent",
        )
    })?;
    let working_ref = session
        .working_materialization_ref
        .as_deref()
        .ok_or_else(|| {
            problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "coding_session_inconsistent",
            )
        })?;
    let client = substrate_client(state, authority)
        .await?
        .ok_or_else(|| problem(StatusCode::SERVICE_UNAVAILABLE, "substrate_unavailable"))?;
    let base = client
        .get_workspace(base_ref)
        .await
        .map_err(|error| substrate_problem(&error))?;
    let working = client
        .get_workspace(working_ref)
        .await
        .map_err(|error| substrate_problem(&error))?;
    Ok((session, base, working))
}

async fn authority(
    state: &AppState,
    authorization: &str,
    session: SessionAuthority,
) -> Result<Authority, Response> {
    let access = state
        .identity
        .issue_access_token(authorization, CONNECTORS_AUDIENCE, CONNECTORS_SCOPE)
        .await
        .map_err(|_| {
            problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "connector_authority_unavailable",
            )
        })?;
    // Agent Platform must receive the transient Identity session so it can validate the user and
    // perform its own current-grant exchange for a user-bound model lease. Exchanging here first
    // would hand it a narrowed access token that cannot legitimately acquire Connector authority.
    let agent_platform_bearer = state
        .agent_platform
        .as_ref()
        .map(|_| authorization.to_owned());
    let mut digest = Sha256::new();
    digest.update(session.tenant_id.as_bytes());
    digest.update(b"\0");
    digest.update(session.subject.as_bytes());
    let snapshot = hex::encode(digest.finalize());
    Ok(Authority {
        tenant_id: session.tenant_id.clone(),
        subject: session.subject.clone(),
        connector_bearer: access
            .credential
            .expose_at_authorization_boundary()
            .to_owned(),
        agent_platform_bearer,
        session_authorization: authorization.to_owned(),
        context: OwnerContext {
            tenant_id: session.tenant_id,
            agent_id: format!("workspace:{}", session.subject),
            agent_revision: 1,
            authority_snapshot_id: format!("identity:{snapshot}"),
            authority_snapshot_sha256: snapshot,
        },
    })
}

async fn search_projects(
    state: &AppState,
    authority: &Authority,
    query: &str,
) -> Result<Vec<RepositoryCandidate>, Response> {
    let described = connector_operation(
        state,
        authority,
        operation::OperationRequest::Describe(operation::DescribeRequest {
            operation_ref: "gitlab-project-list".to_owned(),
        }),
    )
    .await?;
    let operation::OperationResult::Describe(description) = described else {
        return Err(problem(
            StatusCode::BAD_GATEWAY,
            "connector_protocol_invalid",
        ));
    };
    let mut projects = Vec::new();
    for connection in description.connections {
        let mut input = serde_json::json!({
            "membership": true,
            "page": 1,
            "per_page": 25
        });
        if !query.is_empty() {
            input
                .as_object_mut()
                .expect("static object")
                .insert("search".to_owned(), Value::String(query.to_owned()));
        }
        let output = invoke_operation(
            state,
            authority,
            operation::InvokeRequest {
                operation_ref: "gitlab-project-list".to_owned(),
                connection_ref: connection.connection_ref.clone(),
                description_ref: description.description_ref.clone(),
                input,
                approval_evidence_ref: None,
            },
        )
        .await?;
        let values = output
            .as_array()
            .ok_or_else(|| problem(StatusCode::BAD_GATEWAY, "connector_protocol_invalid"))?;
        for value in values {
            if let Some(candidate) = repository_candidate(value, &connection.connection_ref) {
                projects.push(candidate);
            }
        }
    }
    projects.dedup_by(|left, right| {
        left.forge_instance_ref == right.forge_instance_ref && left.project_ref == right.project_ref
    });
    projects.truncate(25);
    Ok(projects)
}

async fn reachable_project(
    state: &AppState,
    authority: &Authority,
    connection_ref: &str,
    project_ref: &str,
) -> Result<RepositoryCandidate, Response> {
    let project_id = project_ref
        .parse::<u64>()
        .ok()
        .filter(|project_id| *project_id > 0)
        .ok_or_else(|| {
            problem(
                StatusCode::UNPROCESSABLE_ENTITY,
                "project_reference_invalid",
            )
        })?;
    let description_ref = describe(state, authority, "gitlab.projects").await?;
    let binding = bindings(state, authority, "gitlab.projects", "")
        .await?
        .into_iter()
        .find(|binding| binding.connection_ref == connection_ref)
        .ok_or_else(|| problem(StatusCode::FORBIDDEN, "project_access_refused"))?;
    let result = datasource(
        state,
        authority,
        DatasourceRequest::Read(ReadRequest {
            datasource_ref: "gitlab.projects".to_owned(),
            binding_ref: binding.binding_ref,
            description_ref,
            read: DatasourceRead::Get {
                key: Value::from(project_id),
            },
        }),
    )
    .await?;
    let DatasourceResult::Read(page) = result else {
        return Err(problem(
            StatusCode::BAD_GATEWAY,
            "connector_protocol_invalid",
        ));
    };
    page.records
        .into_iter()
        .next()
        .and_then(|record| repository_candidate(&record.value, connection_ref))
        .filter(|candidate| candidate.project_ref == project_ref)
        .ok_or_else(|| problem(StatusCode::FORBIDDEN, "project_access_refused"))
}

fn repository_candidate(value: &Value, connection_ref: &str) -> Option<RepositoryCandidate> {
    let project_ref = value.get("id")?.as_u64()?.to_string();
    let path = value.get("path_with_namespace")?.as_str()?;
    Some(RepositoryCandidate {
        forge_instance_ref: connection_ref.to_owned(),
        project_ref,
        path_with_namespace: path.to_owned(),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(path)
            .to_owned(),
        default_branch: value
            .get("default_branch")
            .and_then(Value::as_str)
            .map(str::to_owned),
        visibility: value
            .get("visibility")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        web_url: value
            .get("web_url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        opened_project_id: None,
    })
}

async fn discover_branches(
    state: &AppState,
    authority: &Authority,
    project: &Project,
) -> Result<Vec<Branch>, Response> {
    let description = describe(state, authority, "gitlab.branches").await?;
    let binding = bindings(
        state,
        authority,
        "gitlab.branches",
        &project.path_with_namespace,
    )
    .await?
    .into_iter()
    .find(|binding| {
        binding.connection_ref == project.forge_instance_ref
            && binding.label == project.path_with_namespace
    })
    .ok_or_else(|| problem(StatusCode::FORBIDDEN, "project_access_refused"))?;
    let mut branches = Vec::new();
    let mut cursor = None;
    loop {
        let result = datasource(
            state,
            authority,
            DatasourceRequest::Read(ReadRequest {
                datasource_ref: "gitlab.branches".to_owned(),
                binding_ref: binding.binding_ref.clone(),
                description_ref: description.clone(),
                read: DatasourceRead::List { limit: 25, cursor },
            }),
        )
        .await?;
        let DatasourceResult::Read(page) = result else {
            return Err(problem(
                StatusCode::BAD_GATEWAY,
                "connector_protocol_invalid",
            ));
        };
        for record in page.records {
            let value = record.value;
            let Some(name) = value.get("name").and_then(Value::as_str) else {
                continue;
            };
            let Some(commit) = value
                .get("commit")
                .and_then(|commit| commit.get("id"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            branches.push(Branch {
                name: name.to_owned(),
                commit: commit.to_owned(),
                provider_default: value
                    .get("default")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                protected: value
                    .get("protected")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            });
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok(branches)
}

async fn describe(
    state: &AppState,
    authority: &Authority,
    reference: &str,
) -> Result<String, Response> {
    match datasource(
        state,
        authority,
        DatasourceRequest::Describe(DescribeRequest {
            datasource_ref: reference.to_owned(),
        }),
    )
    .await?
    {
        DatasourceResult::Describe(description) => Ok(description.description_ref),
        _ => Err(problem(
            StatusCode::BAD_GATEWAY,
            "connector_protocol_invalid",
        )),
    }
}

async fn bindings(
    state: &AppState,
    authority: &Authority,
    reference: &str,
    query: &str,
) -> Result<Vec<connectors_client::datasource::DatasourceBinding>, Response> {
    match datasource(
        state,
        authority,
        DatasourceRequest::Bindings(BindingSearchRequest {
            datasource_ref: reference.to_owned(),
            query: query.to_owned(),
            limit: 25,
        }),
    )
    .await?
    {
        DatasourceResult::Bindings { bindings } => Ok(bindings),
        _ => Err(problem(
            StatusCode::BAD_GATEWAY,
            "connector_protocol_invalid",
        )),
    }
}

async fn datasource(
    state: &AppState,
    authority: &Authority,
    request: DatasourceRequest,
) -> Result<DatasourceResult, Response> {
    let response = state
        .connectors
        .datasource(&authority.connector_bearer, &authority.context, request)
        .await
        .map_err(|_| problem(StatusCode::SERVICE_UNAVAILABLE, "connectors_unavailable"))?;
    response
        .response
        .ok_or_else(|| match response.error.map(|error| error.code) {
            Some(connectors_client::datasource::DatasourceErrorCode::NotGranted) => {
                problem(StatusCode::FORBIDDEN, "connector_access_refused")
            }
            Some(connectors_client::datasource::DatasourceErrorCode::StaleAuthority) => {
                problem(StatusCode::CONFLICT, "connector_authority_stale")
            }
            _ => problem(StatusCode::BAD_GATEWAY, "connector_read_refused"),
        })
}

async fn exact_repository_tree(
    state: &AppState,
    authority: &Authority,
    project: &Project,
) -> Result<Vec<RepositoryEntry>, Response> {
    let commit = project
        .pinned_commit
        .as_deref()
        .ok_or_else(|| problem(StatusCode::CONFLICT, "project_snapshot_unpinned"))?;
    let description = operation_description(
        state,
        authority,
        "gitlab-repository-tree-list",
        &project.forge_instance_ref,
    )
    .await?;
    let output = invoke_operation(
        state,
        authority,
        operation::InvokeRequest {
            operation_ref: "gitlab-repository-tree-list".to_owned(),
            connection_ref: project.forge_instance_ref.clone(),
            description_ref: description,
            input: serde_json::json!({
                "project_id": project.project_ref.parse::<u64>().map_err(|_| problem(StatusCode::BAD_GATEWAY, "provider_project_invalid"))?,
                "ref": commit,
                "page": 1,
                "per_page": 100
            }),
            approval_evidence_ref: None,
        },
    )
    .await?;
    let values = output
        .as_array()
        .ok_or_else(|| problem(StatusCode::BAD_GATEWAY, "connector_protocol_invalid"))?;
    let mut entries = Vec::with_capacity(values.len());
    for value in values {
        let Some(object_id) = value.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(name) = value.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(path) = value.get("path").and_then(Value::as_str) else {
            continue;
        };
        let Some(mode) = value.get("mode").and_then(Value::as_str) else {
            continue;
        };
        let kind = match value.get("type").and_then(Value::as_str) {
            Some("blob") => RepositoryEntryKind::Blob,
            Some("tree") => RepositoryEntryKind::Tree,
            _ => continue,
        };
        entries.push(RepositoryEntry {
            object_id: object_id.to_owned(),
            name: name.to_owned(),
            path: path.to_owned(),
            kind,
            mode: mode.to_owned(),
        });
    }
    entries.sort_by(|left, right| {
        matches!(left.kind, RepositoryEntryKind::Blob)
            .cmp(&matches!(right.kind, RepositoryEntryKind::Blob))
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(entries)
}

#[derive(Clone)]
struct MaterializedFile {
    path: String,
    bytes: Vec<u8>,
    sha256: String,
    executable: bool,
}

async fn collect_source_files(
    state: &AppState,
    authority: &Authority,
    project: &Project,
) -> Result<Vec<MaterializedFile>, Response> {
    let commit = project
        .pinned_commit
        .as_deref()
        .ok_or_else(|| problem(StatusCode::CONFLICT, "project_snapshot_unpinned"))?;
    let project_id = project
        .project_ref
        .parse::<u64>()
        .map_err(|_| problem(StatusCode::BAD_GATEWAY, "provider_project_invalid"))?;
    let tree_description = operation_description(
        state,
        authority,
        "gitlab-repository-tree-list",
        &project.forge_instance_ref,
    )
    .await?;
    let file_description = operation_description(
        state,
        authority,
        "gitlab-repository-file-get",
        &project.forge_instance_ref,
    )
    .await?;
    let blobs = collect_source_entries(
        state,
        authority,
        project,
        &tree_description,
        project_id,
        commit,
    )
    .await?;
    let mut files = stream::iter(blobs)
        .map(|entry| {
            let file_description = file_description.clone();
            async move {
                read_source_file(state, authority, project, &file_description, &entry, commit).await
            }
        })
        .buffer_unordered(SOURCE_FETCH_CONCURRENCY)
        .try_collect::<Vec<_>>()
        .await?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let mut total_bytes = 0_u64;
    for file in &files {
        total_bytes = total_bytes
            .checked_add(file.bytes.len() as u64)
            .ok_or_else(|| {
                problem(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "repository_total_limit_exceeded",
                )
            })?;
        if total_bytes > SOURCE_MATERIALIZATION_LIMITS.max_total_bytes {
            return Err(problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                "repository_total_limit_exceeded",
            ));
        }
    }
    Ok(files)
}

async fn collect_source_entries(
    state: &AppState,
    authority: &Authority,
    project: &Project,
    tree_description: &str,
    project_id: u64,
    commit: &str,
) -> Result<Vec<RepositoryEntry>, Response> {
    let mut directories = VecDeque::from([String::new()]);
    let mut visited_directories = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();
    let mut blobs = Vec::new();
    while let Some(directory) = directories.pop_front() {
        if !visited_directories.insert(directory.clone())
            || visited_directories.len() > MAX_SOURCE_DIRECTORIES
        {
            return Err(problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                "repository_tree_limit_exceeded",
            ));
        }
        let mut page = 1_u64;
        loop {
            let mut input = serde_json::json!({
                "project_id": project_id,
                "ref": commit,
                "page": page,
                "per_page": 100
            });
            if !directory.is_empty() {
                input
                    .as_object_mut()
                    .expect("static object")
                    .insert("path".to_owned(), Value::String(directory.clone()));
            }
            let output = invoke_operation(
                state,
                authority,
                operation::InvokeRequest {
                    operation_ref: "gitlab-repository-tree-list".to_owned(),
                    connection_ref: project.forge_instance_ref.clone(),
                    description_ref: tree_description.to_owned(),
                    input,
                    approval_evidence_ref: None,
                },
            )
            .await?;
            let values = output
                .as_array()
                .ok_or_else(|| problem(StatusCode::BAD_GATEWAY, "connector_protocol_invalid"))?;
            for value in values {
                let entry = strict_repository_entry(value)
                    .map_err(|error| problem(error.status, error.code))?;
                if !seen_paths.insert(entry.path.clone()) {
                    return Err(problem(
                        StatusCode::BAD_GATEWAY,
                        "repository_tree_inconsistent",
                    ));
                }
                match entry.kind {
                    RepositoryEntryKind::Tree => directories.push_back(entry.path),
                    RepositoryEntryKind::Blob => {
                        if blobs.len()
                            >= usize::try_from(SOURCE_MATERIALIZATION_LIMITS.max_files)
                                .expect("file limit fits usize")
                        {
                            return Err(problem(
                                StatusCode::PAYLOAD_TOO_LARGE,
                                "repository_file_limit_exceeded",
                            ));
                        }
                        blobs.push(entry);
                    }
                }
            }
            if values.len() < 100 {
                break;
            }
            page = page.checked_add(1).ok_or_else(|| {
                problem(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "repository_tree_limit_exceeded",
                )
            })?;
        }
    }
    blobs.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(blobs)
}

#[derive(Clone, Copy)]
struct EntryProblem {
    status: StatusCode,
    code: &'static str,
}

fn entry_problem(status: StatusCode, code: &'static str) -> EntryProblem {
    EntryProblem { status, code }
}

fn strict_repository_entry(value: &Value) -> Result<RepositoryEntry, EntryProblem> {
    let object_id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| entry_problem(StatusCode::BAD_GATEWAY, "connector_protocol_invalid"))?;
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| entry_problem(StatusCode::BAD_GATEWAY, "connector_protocol_invalid"))?;
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| valid_repository_path(value))
        .ok_or_else(|| entry_problem(StatusCode::BAD_GATEWAY, "repository_path_refused"))?;
    let mode = value
        .get("mode")
        .and_then(Value::as_str)
        .ok_or_else(|| entry_problem(StatusCode::BAD_GATEWAY, "connector_protocol_invalid"))?;
    let kind = match (value.get("type").and_then(Value::as_str), mode) {
        (Some("blob"), "100644" | "100755") => RepositoryEntryKind::Blob,
        (Some("tree"), "040000") => RepositoryEntryKind::Tree,
        (Some("commit"), _) | (_, "120000" | "160000") => {
            return Err(entry_problem(
                StatusCode::UNPROCESSABLE_ENTITY,
                "repository_entry_unsupported",
            ));
        }
        _ => {
            return Err(entry_problem(
                StatusCode::BAD_GATEWAY,
                "repository_tree_inconsistent",
            ));
        }
    };
    Ok(RepositoryEntry {
        object_id: object_id.to_owned(),
        name: name.to_owned(),
        path: path.to_owned(),
        kind,
        mode: mode.to_owned(),
    })
}

fn valid_repository_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1_024
        && !path.starts_with('/')
        && !path.contains(['\0', '\\'])
        && path
            .split('/')
            .all(|component| !component.is_empty() && !matches!(component, "." | ".." | ".git"))
}

async fn read_source_file(
    state: &AppState,
    authority: &Authority,
    project: &Project,
    description_ref: &str,
    entry: &RepositoryEntry,
    commit: &str,
) -> Result<MaterializedFile, Response> {
    let project_id = project
        .project_ref
        .parse::<u64>()
        .map_err(|_| problem(StatusCode::BAD_GATEWAY, "provider_project_invalid"))?;
    let output = invoke_operation(
        state,
        authority,
        operation::InvokeRequest {
            operation_ref: "gitlab-repository-file-get".to_owned(),
            connection_ref: project.forge_instance_ref.clone(),
            description_ref: description_ref.to_owned(),
            input: source_file_input(project_id, &entry.path, commit),
            approval_evidence_ref: None,
        },
    )
    .await?;
    if output.get("encoding").and_then(Value::as_str) != Some("base64")
        || output.get("file_path").and_then(Value::as_str) != Some(entry.path.as_str())
        || output.get("commit_id").and_then(Value::as_str) != Some(commit)
    {
        return Err(problem(
            StatusCode::BAD_GATEWAY,
            "repository_file_inconsistent",
        ));
    }
    let size = output
        .get("size")
        .and_then(Value::as_u64)
        .ok_or_else(|| problem(StatusCode::BAD_GATEWAY, "connector_protocol_invalid"))?;
    if size > SOURCE_MATERIALIZATION_LIMITS.max_file_bytes {
        return Err(problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            "repository_file_limit_exceeded",
        ));
    }
    let encoded = output
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| problem(StatusCode::BAD_GATEWAY, "connector_protocol_invalid"))?;
    let compact = encoded
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(compact)
        .map_err(|_| problem(StatusCode::BAD_GATEWAY, "connector_protocol_invalid"))?;
    let observed_sha256 = output
        .get("content_sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| problem(StatusCode::BAD_GATEWAY, "connector_protocol_invalid"))?;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let executable = entry.mode == "100755";
    if bytes.len() as u64 != size
        || observed_sha256 != sha256
        || output
            .get("execute_filemode")
            .and_then(Value::as_bool)
            .is_some_and(|observed| observed != executable)
    {
        return Err(problem(
            StatusCode::BAD_GATEWAY,
            "repository_file_inconsistent",
        ));
    }
    Ok(MaterializedFile {
        path: entry.path.clone(),
        bytes,
        sha256,
        executable,
    })
}

fn source_file_input(project_id: u64, path: &str, commit: &str) -> Value {
    serde_json::json!({
        "project_id": project_id,
        "file_path": path,
        "ref": commit
    })
}

async fn provision_materializations(
    state: AppState,
    authority: Authority,
    client: SubstrateClient,
    mut session: CodingSession,
    files: Vec<MaterializedFile>,
) -> Result<CodingSession, (CodingSession, SubstrateError)> {
    let base = ensure_materialization(&state, &authority, &client, &mut session, true).await?;
    let working = ensure_materialization(&state, &authority, &client, &mut session, false).await?;
    let files = Arc::new(files);
    tokio::try_join!(
        upload_materialization(base, session.id.clone(), "base", Arc::clone(&files)),
        upload_materialization(working, session.id.clone(), "working", Arc::clone(&files)),
    )
    .map_err(|error| (session.clone(), error))?;
    let manifest_sha256 = source_manifest_sha256(&session.source_revision, &files);
    state
        .store
        .complete_coding_session(&authority, &session.id, &manifest_sha256)
        .await
        .map_err(|error| {
            (
                session,
                SubstrateError::Protocol(format!("Workspace store refused completion: {error}")),
            )
        })
}

async fn ensure_materialization(
    state: &AppState,
    authority: &Authority,
    client: &SubstrateClient,
    session: &mut CodingSession,
    base: bool,
) -> Result<SubstrateWorkspace, (CodingSession, SubstrateError)> {
    let existing = if base {
        session.base_materialization_ref.as_deref()
    } else {
        session.working_materialization_ref.as_deref()
    };
    if let Some(reference) = existing {
        return client
            .get_workspace(reference)
            .await
            .map_err(|error| (session.clone(), error));
    }
    let role = if base { "base" } else { "working" };
    let workspace = client
        .workspace()
        .empty()
        .label("coding.session", &session.id)
        .label("materialization.role", role)
        .label("source.revision", &session.source_revision)
        .operation_id(substrate_operation_id(&session.id, &["create", role]))
        .create()
        .await
        .map_err(|error| (session.clone(), error))?;
    *session = state
        .store
        .record_materialization_ref(authority, &session.id, base, workspace.id())
        .await
        .map_err(|error| {
            (
                session.clone(),
                SubstrateError::Protocol(format!(
                    "Workspace store refused materialization reference: {error}"
                )),
            )
        })?;
    Ok(workspace)
}

async fn upload_materialization(
    workspace: SubstrateWorkspace,
    session_id: String,
    role: &'static str,
    files: Arc<Vec<MaterializedFile>>,
) -> Result<(), SubstrateError> {
    stream::iter(0..files.len())
        .map(|index| {
            let workspace = workspace.clone();
            let session_id = session_id.clone();
            let files = Arc::clone(&files);
            async move {
                let file = &files[index];
                workspace
                    .replace_file(
                        &file.path,
                        &file.bytes,
                        ExpectedFileState::Absent,
                        true,
                        Some(substrate_operation_id(
                            &session_id,
                            &["file", role, &file.path, &file.sha256],
                        )),
                    )
                    .await
            }
        })
        .buffer_unordered(MATERIALIZATION_UPLOAD_CONCURRENCY)
        .try_collect::<Vec<_>>()
        .await?;
    let executable = files
        .iter()
        .filter(|file| file.executable)
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    for (index, chunk) in executable.chunks(64).enumerate() {
        let mut arguments = Vec::with_capacity(chunk.len() + 2);
        arguments.push("u+x");
        arguments.push("--");
        arguments.extend(chunk.iter().copied());
        let policy = ExecutionPolicy::builder()
            .timeout(Duration::from_secs(30))
            .cpu_time(Duration::from_secs(5))
            .memory_bytes(64 * 1024 * 1024)
            .processes(16)
            .output_bytes(64 * 1024)
            .build()?;
        let output = workspace
            .command("/usr/bin/chmod")
            .args(arguments)
            .policy(policy)
            .operation_id(substrate_operation_id(
                &session_id,
                &["chmod", role, &index.to_string()],
            ))
            .run()
            .await?;
        if output.exec.exit.as_ref().and_then(|exit| exit.code) != Some(0) {
            return Err(SubstrateError::Protocol(
                "confined executable-mode application failed".to_owned(),
            ));
        }
    }
    Ok(())
}

fn source_manifest_sha256(source_revision: &str, files: &[MaterializedFile]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"b10x/workspace/source-manifest/v1");
    append_digest_part(&mut digest, source_revision.as_bytes());
    for file in files {
        append_digest_part(&mut digest, file.path.as_bytes());
        append_digest_part(&mut digest, file.sha256.as_bytes());
        digest.update((file.bytes.len() as u64).to_be_bytes());
        digest.update([u8::from(file.executable)]);
    }
    hex::encode(digest.finalize())
}

#[derive(Clone)]
struct CompleteWorkspaceFile {
    bytes: Vec<u8>,
    sha256: String,
    size: u64,
}

async fn project_file(
    path: &str,
    base: &SubstrateWorkspace,
    working: &SubstrateWorkspace,
    max_file_bytes: u64,
) -> Result<Option<FileProjection>, Response> {
    let Some(current) = read_complete_file(working, path, max_file_bytes).await? else {
        return Ok(None);
    };
    let base = read_complete_file(base, path, max_file_bytes).await?;
    let modification = match base {
        None => FileModificationState::Added,
        Some(base) if base.sha256 == current.sha256 => FileModificationState::Unchanged,
        Some(_) => FileModificationState::Modified,
    };
    Ok(Some(file_projection(path, current, modification)))
}

async fn base_file_projection(
    path: &str,
    base: &SubstrateWorkspace,
    max_file_bytes: u64,
) -> Result<Option<FileProjection>, Response> {
    Ok(read_complete_file(base, path, max_file_bytes)
        .await?
        .map(|file| file_projection(path, file, FileModificationState::Unchanged)))
}

fn file_projection(
    path: &str,
    file: CompleteWorkspaceFile,
    modification: FileModificationState,
) -> FileProjection {
    let content = String::from_utf8(file.bytes).ok();
    FileProjection {
        format: "workspace.file-projection/1".to_owned(),
        revision: FileRevision {
            path: path.to_owned(),
            sha256: file.sha256,
            size: file.size,
            language: language_for_path(path).map(str::to_owned),
            modification,
        },
        binary: content.is_none(),
        content,
        truncated: false,
    }
}

async fn read_complete_file(
    workspace: &SubstrateWorkspace,
    path: &str,
    max_file_bytes: u64,
) -> Result<Option<CompleteWorkspaceFile>, Response> {
    match workspace.read_file_v2(path, 0, max_file_bytes).await {
        Ok(file) if file.eof => Ok(Some(CompleteWorkspaceFile {
            bytes: file.bytes,
            sha256: file.sha256,
            size: file.size,
        })),
        Ok(_) => Err(problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            "workspace_file_limit_exceeded",
        )),
        Err(SubstrateError::Refusal(refusal)) if refusal.code == "resource.not-found" => Ok(None),
        Err(error) => Err(substrate_problem(&error)),
    }
}

fn language_for_path(path: &str) -> Option<&'static str> {
    let extension = path.rsplit_once('.').map(|(_, extension)| extension)?;
    match extension.to_ascii_lowercase().as_str() {
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hh" | "hpp" => Some("cpp"),
        "css" => Some("css"),
        "go" => Some("go"),
        "html" | "htm" => Some("html"),
        "java" => Some("java"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "json" | "jsonc" => Some("json"),
        "md" | "mdx" => Some("markdown"),
        "py" => Some("python"),
        "rb" => Some("ruby"),
        "rs" => Some("rust"),
        "sh" | "bash" => Some("shell"),
        "sql" => Some("sql"),
        "toml" => Some("toml"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("typescriptreact"),
        "xml" => Some("xml"),
        "yaml" | "yml" => Some("yaml"),
        _ => None,
    }
}

fn file_operation_id(session_id: &str, path: &str, input: &WriteFile) -> String {
    let mut digest = Sha256::new();
    digest.update(b"b10x/workspace/file-operation/v1");
    append_digest_part(&mut digest, session_id.as_bytes());
    append_digest_part(&mut digest, path.as_bytes());
    append_digest_part(&mut digest, input.operation_id.as_bytes());
    append_digest_part(&mut digest, input.content.as_bytes());
    append_digest_part(
        &mut digest,
        &serde_json::to_vec(&input.expected).expect("file expected state serializes"),
    );
    digest.update([u8::from(input.create_parents)]);
    hex::encode(digest.finalize())
}

async fn materialization_files(
    workspace: &SubstrateWorkspace,
    session: &CodingSession,
) -> Result<BTreeMap<String, CompleteWorkspaceFile>, Response> {
    let tree = workspace
        .tree(session.limits.max_files, true)
        .await
        .map_err(|error| substrate_problem(&error))?;
    if tree.truncated {
        return Err(problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            "diff_tree_truncated",
        ));
    }
    let mut files = BTreeMap::new();
    for entry in tree.items {
        if serde_json::to_value(entry.kind)
            .ok()
            .and_then(|kind| kind.as_str().map(str::to_owned))
            .as_deref()
            != Some("file")
        {
            continue;
        }
        let file = read_complete_file(workspace, &entry.path, session.limits.max_file_bytes)
            .await?
            .ok_or_else(|| problem(StatusCode::BAD_GATEWAY, "workspace_tree_file_inconsistent"))?;
        files.insert(entry.path, file);
    }
    Ok(files)
}

fn canonical_diff(
    selector: &ChangeSelector,
    mode: DiffMode,
    source_revision: &str,
    base: &BTreeMap<String, CompleteWorkspaceFile>,
    working: &BTreeMap<String, CompleteWorkspaceFile>,
    actor: &str,
) -> DiffProjection {
    let paths = base
        .keys()
        .chain(working.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut files = Vec::new();
    let mut additions = 0_u64;
    let mut deletions = 0_u64;
    for path in paths {
        let old = base.get(&path);
        let new = working.get(&path);
        if old
            .zip(new)
            .is_some_and(|(old, new)| old.sha256 == new.sha256)
        {
            continue;
        }
        let (file, file_additions, file_deletions) = canonical_file_diff(
            &path,
            old,
            new,
            mode,
            &[format!("actor:{actor}"), "operation:workspace".to_owned()],
        );
        additions = additions.saturating_add(file_additions);
        deletions = deletions.saturating_add(file_deletions);
        files.push(file);
    }
    let mut seal = serde_json::json!({
        "format": "workspace.diff-projection/1",
        "selector": selector,
        "mode": mode,
        "source_revision": source_revision,
        "files": files,
        "additions": additions,
        "deletions": deletions,
        "partial": false
    });
    let digest = hex::encode(Sha256::digest(
        serde_json::to_vec(&seal).expect("canonical diff serializes"),
    ));
    seal.as_object_mut()
        .expect("static diff seal is an object")
        .insert("digest".to_owned(), Value::String(digest));
    serde_json::from_value(seal).expect("canonical diff contract is self-consistent")
}

fn canonical_file_diff(
    path: &str,
    old: Option<&CompleteWorkspaceFile>,
    new: Option<&CompleteWorkspaceFile>,
    mode: DiffMode,
    attribution: &[String],
) -> (DiffFile, u64, u64) {
    let status = match (old, new) {
        (None, Some(_)) => "added",
        (Some(_), None) => "deleted",
        (Some(_), Some(_)) => "modified",
        (None, None) => unreachable!("union path exists on at least one side"),
    };
    let old_text = old.and_then(|file| std::str::from_utf8(&file.bytes).ok());
    let new_text = new.and_then(|file| std::str::from_utf8(&file.bytes).ok());
    let text_representable =
        old.is_none_or(|_| old_text.is_some()) && new.is_none_or(|_| new_text.is_some());
    let (additions, deletions, mut hunks) = if text_representable {
        text_diff(
            old_text.unwrap_or_default(),
            new_text.unwrap_or_default(),
            path,
        )
    } else {
        (0, 0, Vec::new())
    };
    if mode != DiffMode::Patch {
        hunks.clear();
    }
    let counts_visible = text_representable && mode != DiffMode::FilesOnly;
    (
        DiffFile {
            old_path: old.map(|_| path.to_owned()),
            new_path: new.map(|_| path.to_owned()),
            status: status.to_owned(),
            additions: counts_visible.then_some(additions),
            deletions: counts_visible.then_some(deletions),
            old_sha256: old.map(|file| file.sha256.clone()),
            new_sha256: new.map(|file| file.sha256.clone()),
            hunks,
            attribution: attribution.to_vec(),
        },
        additions,
        deletions,
    )
}

fn text_diff(old: &str, new: &str, path: &str) -> (u64, u64, Vec<DiffHunk>) {
    let diff = TextDiff::from_lines(old, new);
    let mut additions = 0_u64;
    let mut deletions = 0_u64;
    let mut hunks = Vec::new();
    for group in diff.grouped_ops(3) {
        let first = group.first().expect("diff group is non-empty");
        let last = group.last().expect("diff group is non-empty");
        let old_start = first.old_range().start;
        let old_end = last.old_range().end;
        let new_start = first.new_range().start;
        let new_end = last.new_range().end;
        let mut lines = Vec::new();
        for operation in group {
            for change in diff.iter_changes(&operation) {
                let kind = match change.tag() {
                    ChangeTag::Equal => "context",
                    ChangeTag::Delete => {
                        deletions = deletions.saturating_add(1);
                        "deletion"
                    }
                    ChangeTag::Insert => {
                        additions = additions.saturating_add(1);
                        "addition"
                    }
                };
                lines.push(DiffLine {
                    kind: kind.to_owned(),
                    old_line: change.old_index().map(|line| line as u64 + 1),
                    new_line: change.new_index().map(|line| line as u64 + 1),
                    content: change
                        .value()
                        .strip_suffix('\n')
                        .unwrap_or(change.value())
                        .to_owned(),
                });
            }
        }
        let old_range = DiffRange {
            start: old_start as u64 + 1,
            lines: (old_end - old_start) as u64,
        };
        let new_range = DiffRange {
            start: new_start as u64 + 1,
            lines: (new_end - new_start) as u64,
        };
        let hunk_id = hex::encode(Sha256::digest(
            serde_json::to_vec(&(path, &old_range, &new_range, &lines))
                .expect("diff hunk serializes"),
        ));
        hunks.push(DiffHunk {
            id: hunk_id,
            old: old_range,
            new: new_range,
            heading: None,
            lines,
        });
    }
    (additions, deletions, hunks)
}

fn substrate_operation_id(session_id: &str, parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"b10x/workspace/substrate-operation/v1");
    append_digest_part(&mut digest, session_id.as_bytes());
    for part in parts {
        append_digest_part(&mut digest, part.as_bytes());
    }
    hex::encode(digest.finalize())
}

fn append_digest_part(digest: &mut Sha256, part: &[u8]) {
    digest.update((part.len() as u64).to_be_bytes());
    digest.update(part);
}

async fn cleanup_terminals(
    state: &AppState,
    authority: &Authority,
    client: &SubstrateClient,
    coding_session: &CodingSession,
) -> bool {
    let Ok(terminals) = state.store.terminals(authority, &coding_session.id).await else {
        return true;
    };
    let mut unknown = false;
    for terminal in terminals {
        let Some(reference) = terminal.substrate_session_ref.as_deref() else {
            if matches!(
                terminal.public.state,
                TerminalState::Preparing | TerminalState::Running | TerminalState::Unknown
            ) {
                unknown = true;
            }
            continue;
        };
        let mut process = match client.get_pipe_session(reference).await {
            Ok(process) => process,
            Err(SubstrateError::Refusal(refusal)) if refusal.code == "resource.not-found" => {
                if state
                    .store
                    .complete_terminal_termination(authority, &terminal.public.id, None)
                    .await
                    .is_err()
                {
                    unknown = true;
                }
                state.terminal_replay.remove(&terminal.public.id).await;
                continue;
            }
            Err(_) => {
                unknown = true;
                continue;
            }
        };
        if matches!(
            process.observation().state,
            PipeSessionState::Accepted | PipeSessionState::Ready | PipeSessionState::Attached
        ) && process
            .signal_with_operation_id(
                Signal::Kill,
                Duration::from_millis(1),
                Some(substrate_operation_id(
                    &coding_session.id,
                    &["terminal", &terminal.public.id, "session-close-kill"],
                )),
            )
            .await
            .is_err()
        {
            unknown = true;
            continue;
        }
        let exit = terminal_exit(process.observation().exit.as_ref());
        if process
            .retire_with_operation_id(Some(substrate_operation_id(
                &coding_session.id,
                &["terminal", &terminal.public.id, "session-close-retire"],
            )))
            .await
            .is_err()
            || state
                .store
                .complete_terminal_termination(authority, &terminal.public.id, exit.as_ref())
                .await
                .is_err()
        {
            unknown = true;
            continue;
        }
        state.terminal_replay.remove(&terminal.public.id).await;
    }
    unknown
}

async fn cleanup_materializations(client: &SubstrateClient, session: &CodingSession) -> bool {
    let mut unknown = false;
    for (role, reference) in [
        ("base", session.base_materialization_ref.as_deref()),
        ("working", session.working_materialization_ref.as_deref()),
    ] {
        let Some(reference) = reference else {
            continue;
        };
        match client.get_workspace(reference).await {
            Ok(workspace) => {
                if workspace
                    .destroy_with_operation_id(Some(substrate_operation_id(
                        &session.id,
                        &["cleanup", role],
                    )))
                    .await
                    .is_err()
                {
                    unknown = true;
                }
            }
            Err(SubstrateError::Refusal(refusal)) if refusal.code == "resource.not-found" => {}
            Err(_) => unknown = true,
        }
    }
    unknown
}

async fn cleanup_materializations_owned(client: SubstrateClient, session: CodingSession) -> bool {
    cleanup_materializations(&client, &session).await
}

fn public_session(mut session: CodingSession) -> CodingSession {
    if session.state != CodingSessionState::Ready {
        session.base_materialization_ref = None;
        session.working_materialization_ref = None;
        session.manifest_sha256 = None;
    }
    session
}

async fn project_context(
    state: &AppState,
    authority: &Authority,
    project: &Project,
) -> Result<ProjectContext, Response> {
    let commit = project
        .pinned_commit
        .clone()
        .ok_or_else(|| problem(StatusCode::CONFLICT, "project_snapshot_unpinned"))?;
    let tree = exact_repository_tree(state, authority, project).await?;
    let description = operation_description(
        state,
        authority,
        "gitlab-repository-file-get",
        &project.forge_instance_ref,
    )
    .await?;
    let mut files = Vec::new();
    for candidate in PROJECT_CONTEXT_FILES {
        if !tree
            .iter()
            .any(|entry| entry.kind == RepositoryEntryKind::Blob && entry.path == *candidate)
        {
            continue;
        }
        if let Some(file) =
            read_context_file(state, authority, project, &description, candidate, &commit).await?
        {
            files.push(file);
        }
    }
    Ok(ProjectContext {
        project_id: project.id.clone(),
        provider: "gitlab".to_owned(),
        provider_project_ref: project.project_ref.clone(),
        path_with_namespace: project.path_with_namespace.clone(),
        branch: project.selected_branch.clone(),
        commit,
        files,
    })
}

async fn read_context_file(
    state: &AppState,
    authority: &Authority,
    project: &Project,
    description_ref: &str,
    path: &str,
    commit: &str,
) -> Result<Option<ProjectContextFile>, Response> {
    let output = invoke_operation(
        state,
        authority,
        operation::InvokeRequest {
            operation_ref: "gitlab-repository-file-get".to_owned(),
            connection_ref: project.forge_instance_ref.clone(),
            description_ref: description_ref.to_owned(),
            input: serde_json::json!({
                "project_id": project.project_ref.parse::<u64>().map_err(|_| problem(StatusCode::BAD_GATEWAY, "provider_project_invalid"))?,
                "file_path": path,
                "ref": commit
            }),
            approval_evidence_ref: None,
        },
    )
    .await?;
    if output.get("encoding").and_then(Value::as_str) != Some("base64") {
        return Ok(None);
    }
    let encoded = output
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| problem(StatusCode::BAD_GATEWAY, "connector_protocol_invalid"))?;
    let compact = encoded
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(compact)
        .map_err(|_| problem(StatusCode::BAD_GATEWAY, "connector_protocol_invalid"))?;
    let Ok(mut content) = String::from_utf8(decoded) else {
        return Ok(None);
    };
    let truncated = content.len() > 32 * 1024;
    if truncated {
        let mut boundary = 32 * 1024;
        while !content.is_char_boundary(boundary) {
            boundary -= 1;
        }
        content.truncate(boundary);
    }
    Ok(Some(ProjectContextFile {
        path: path.to_owned(),
        content,
        truncated,
    }))
}

async fn operation_description(
    state: &AppState,
    authority: &Authority,
    operation_ref: &str,
    connection_ref: &str,
) -> Result<String, Response> {
    let result = connector_operation(
        state,
        authority,
        operation::OperationRequest::Describe(operation::DescribeRequest {
            operation_ref: operation_ref.to_owned(),
        }),
    )
    .await?;
    let operation::OperationResult::Describe(description) = result else {
        return Err(problem(
            StatusCode::BAD_GATEWAY,
            "connector_protocol_invalid",
        ));
    };
    if !description
        .connections
        .iter()
        .any(|connection| connection.connection_ref == connection_ref)
    {
        return Err(problem(StatusCode::FORBIDDEN, "project_access_refused"));
    }
    Ok(description.description_ref)
}

async fn invoke_operation(
    state: &AppState,
    authority: &Authority,
    request: operation::InvokeRequest,
) -> Result<Value, Response> {
    match connector_operation(
        state,
        authority,
        operation::OperationRequest::Invoke(request),
    )
    .await?
    {
        operation::OperationResult::Invoke(result) => Ok(result.output),
        _ => Err(problem(
            StatusCode::BAD_GATEWAY,
            "connector_protocol_invalid",
        )),
    }
}

async fn connector_operation(
    state: &AppState,
    authority: &Authority,
    request: operation::OperationRequest,
) -> Result<operation::OperationResult, Response> {
    let response = state
        .connectors
        .operation(&authority.connector_bearer, &authority.context, request)
        .await
        .map_err(|_| problem(StatusCode::SERVICE_UNAVAILABLE, "connectors_unavailable"))?;
    response
        .response
        .ok_or_else(|| match response.error.map(|error| error.code) {
            Some(operation::OperationErrorCode::NotGranted) => {
                problem(StatusCode::FORBIDDEN, "connector_access_refused")
            }
            Some(operation::OperationErrorCode::StaleAuthority) => {
                problem(StatusCode::CONFLICT, "connector_authority_stale")
            }
            Some(operation::OperationErrorCode::ResultTooLarge) => problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                "repository_context_too_large",
            ),
            _ => problem(StatusCode::BAD_GATEWAY, "connector_read_refused"),
        })
}

async fn ensure_project_agent(
    state: &AppState,
    client: &AgentPlatformClient,
    bearer: &str,
    authority: &Authority,
    project: &Project,
) -> Result<AgentId, Response> {
    if let Some(agent_id) = state
        .store
        .project_agent(&authority.tenant_id, &project.id)
        .await
        .map_err(|error| store_problem(&error))?
    {
        return AgentId::new(agent_id)
            .map_err(|_| problem(StatusCode::SERVICE_UNAVAILABLE, "project_agent_invalid"));
    }
    let name = format!("Repository project {}", project.id);
    let agent = match client.list_agents(bearer).await {
        Ok(agents) => agents.into_iter().find(|agent| agent.name == name),
        Err(_) => {
            return Err(problem(
                StatusCode::BAD_GATEWAY,
                "project_agent_unavailable",
            ));
        }
    };
    let agent = match agent {
        Some(agent) => agent,
        None => client
            .create_agent(bearer, &CreateAgent { name })
            .await
            .map_err(|_| problem(StatusCode::BAD_GATEWAY, "project_agent_unavailable"))?,
    };
    if agent.active_revision.is_none() {
        let model = state
            .project_agent_model
            .as_ref()
            .ok_or_else(|| problem(StatusCode::SERVICE_UNAVAILABLE, "project_agent_unavailable"))?;
        let revision = client
            .create_revision(
                bearer,
                &agent.id,
                &RevisionSpec {
                    instructions: "You are the analysis-only agent for one repository project. Ground every answer in the exact commit and files supplied in the typed project context. State when the supplied context is insufficient. Never claim write, merge, or deployment authority.".to_owned(),
                    model: model.clone(),
                    capability_profile_id: None,
                    metadata: Some(serde_json::json!({"workspace_project_id": project.id})),
                },
            )
            .await
            .map_err(|_| problem(StatusCode::BAD_GATEWAY, "project_agent_unavailable"))?;
        client
            .activate_revision(
                bearer,
                &agent.id,
                &ActivateRevision {
                    revision: revision.revision,
                    expected_active_revision: None,
                },
            )
            .await
            .map_err(|_| problem(StatusCode::BAD_GATEWAY, "project_agent_unavailable"))?;
    }
    let recorded = state
        .store
        .record_project_agent(&authority.tenant_id, &project.id, agent.id.as_str())
        .await
        .map_err(|error| store_problem(&error))?;
    AgentId::new(recorded)
        .map_err(|_| problem(StatusCode::SERVICE_UNAVAILABLE, "project_agent_invalid"))
}

fn spawn_task_completion(
    store: Store,
    client: AgentPlatformClient,
    bearer: String,
    tenant_id: String,
    subject: String,
    thread_id: String,
    task_id: agent_platform_core::TaskId,
) {
    tokio::spawn(async move {
        for _ in 0..300 {
            let Ok(task) = client.get_task(&bearer, &task_id).await else {
                break;
            };
            match task.status {
                TaskStatus::Succeeded => {
                    let content = task.output.unwrap_or_else(|| {
                        "The project agent completed without a text response.".to_owned()
                    });
                    let _ = store
                        .append_agent_message(
                            &tenant_id,
                            &subject,
                            &thread_id,
                            MessageRole::Assistant,
                            &content,
                        )
                        .await;
                    return;
                }
                TaskStatus::Failed
                | TaskStatus::Cancelled
                | TaskStatus::Refused
                | TaskStatus::OutcomeUnknown => {
                    let _ = store
                        .append_agent_message(
                            &tenant_id,
                            &subject,
                            &thread_id,
                            MessageRole::System,
                            "The project agent did not complete this turn.",
                        )
                        .await;
                    return;
                }
                TaskStatus::Accepted | TaskStatus::Running | TaskStatus::AwaitingApproval => {}
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let _ = store
            .append_agent_message(
                &tenant_id,
                &subject,
                &thread_id,
                MessageRole::System,
                "The project agent result was not available before the observation window closed.",
            )
            .await;
    });
}

async fn resume_workflow_completions(
    state: &AppState,
    authority: &Authority,
    project_id: &str,
) -> Result<(), StoreError> {
    let (Some(client), Some(bearer)) = (
        state.agent_platform.as_ref(),
        authority.agent_platform_bearer.as_deref(),
    ) else {
        return Ok(());
    };
    let tasks = state
        .store
        .recoverable_workflow_tasks(authority, project_id)
        .await?;
    for task in tasks {
        let Ok(task_id) = TaskId::new(task.task_id) else {
            let _ = state
                .store
                .update_workflow_run(
                    &authority.tenant_id,
                    &authority.subject,
                    &task.run_id,
                    WorkflowRunState::Failed,
                    Some("workflow_task_invalid"),
                    None,
                )
                .await?;
            continue;
        };
        spawn_workflow_completion(WorkflowObservation {
            store: state.store.clone(),
            client: client.clone(),
            observers: state.workflow_observers.clone(),
            bearer: bearer.to_owned(),
            tenant_id: authority.tenant_id.clone(),
            subject: authority.subject.clone(),
            run_id: task.run_id,
            task_id,
        })
        .await;
    }
    Ok(())
}

struct WorkflowObservation {
    store: Store,
    client: AgentPlatformClient,
    observers: WorkflowObservers,
    bearer: String,
    tenant_id: String,
    subject: String,
    run_id: String,
    task_id: agent_platform_core::TaskId,
}

#[derive(Debug, PartialEq, Eq)]
enum WorkflowTaskOutcome {
    Accepted,
    Running,
    Succeeded(String),
    Failed(&'static str),
}

impl WorkflowObservation {
    fn owns(&self, task: &Task) -> bool {
        task.id == self.task_id
            && task.tenant_id.as_str() == self.tenant_id
            && task.actor.as_str() == self.subject
            && task.idempotency_key == format!("workspace-workflow:{}", self.run_id)
    }

    async fn transition(
        &self,
        state: WorkflowRunState,
        failure_code: Option<&str>,
        output: Option<&str>,
    ) {
        let _ = self
            .store
            .update_workflow_run(
                &self.tenant_id,
                &self.subject,
                &self.run_id,
                state,
                failure_code,
                output,
            )
            .await;
    }

    async fn observe(&self) {
        self.observe_window(300, Duration::from_millis(500)).await;
    }

    async fn observe_window(&self, attempts: usize, interval: Duration) {
        for attempt in 0..attempts {
            let task = match self.client.get_task(&self.bearer, &self.task_id).await {
                Ok(task) => task,
                Err(error) if retryable_workflow_observation(&error) => {
                    if attempt + 1 < attempts {
                        tokio::time::sleep(interval).await;
                    }
                    continue;
                }
                Err(_) => return,
            };
            if !self.owns(&task) {
                self.transition(
                    WorkflowRunState::Failed,
                    Some("workflow_task_binding_refused"),
                    None,
                )
                .await;
                return;
            }
            match workflow_task_outcome(task.status, task.output) {
                WorkflowTaskOutcome::Accepted => {}
                WorkflowTaskOutcome::Running => {
                    self.transition(WorkflowRunState::Running, None, None).await;
                }
                WorkflowTaskOutcome::Succeeded(output) => {
                    self.transition(WorkflowRunState::Succeeded, None, Some(&output))
                        .await;
                    return;
                }
                WorkflowTaskOutcome::Failed(code) => {
                    self.transition(WorkflowRunState::Failed, Some(code), None)
                        .await;
                    return;
                }
            }
            if attempt + 1 < attempts {
                tokio::time::sleep(interval).await;
            }
        }
    }
}

fn retryable_workflow_observation(error: &AgentPlatformClientError) -> bool {
    match error {
        AgentPlatformClientError::Transport(_) => true,
        AgentPlatformClientError::Refused(status) => {
            matches!(*status, 408 | 425 | 429 | 500..=599)
        }
        AgentPlatformClientError::Configuration => false,
    }
}

async fn submit_workflow_task(
    client: &AgentPlatformClient,
    bearer: &str,
    task: &SubmitTask,
) -> Result<Task, AgentPlatformClientError> {
    client.submit_task(bearer, task).await
}

fn workflow_task_outcome(status: TaskStatus, output: Option<String>) -> WorkflowTaskOutcome {
    match status {
        TaskStatus::Accepted => WorkflowTaskOutcome::Accepted,
        TaskStatus::Running | TaskStatus::AwaitingApproval => WorkflowTaskOutcome::Running,
        TaskStatus::Succeeded => WorkflowTaskOutcome::Succeeded(
            output
                .filter(|output| !output.trim().is_empty())
                .unwrap_or_else(|| {
                    "# Workflow result\n\nThe workflow completed without a Markdown result."
                        .to_owned()
                }),
        ),
        TaskStatus::Failed => WorkflowTaskOutcome::Failed("workflow_execution_failed"),
        TaskStatus::Cancelled => WorkflowTaskOutcome::Failed("workflow_execution_cancelled"),
        TaskStatus::Refused => WorkflowTaskOutcome::Failed("workflow_execution_refused"),
        TaskStatus::OutcomeUnknown => WorkflowTaskOutcome::Failed("workflow_outcome_unknown"),
    }
}

async fn spawn_workflow_completion(observation: WorkflowObservation) {
    if !observation.observers.begin(&observation.run_id).await {
        return;
    }
    let observers = observation.observers.clone();
    let run_id = observation.run_id.clone();
    tokio::spawn(async move {
        observation.observe().await;
        observers.finish(&run_id).await;
    });
}

fn workflow_prompt(definition_id: &str) -> &'static str {
    match definition_id {
        "review.code/v1" => {
            "Review this exact repository snapshot for correctness, regressions, maintainability risks, and missing tests. Return a concise Markdown report ordered by severity. Cite every finding with repository paths from the supplied context and say explicitly when the bounded context is insufficient."
        }
        "review.security/v1" => {
            "Perform a security review of this exact repository snapshot. Return a concise Markdown report with severity, exploit preconditions, impact, and remediation for each finding. Cite repository paths from the supplied context and do not claim evidence that was not supplied."
        }
        "reverse.aep-ess/v1" => {
            "Reverse-engineer this exact repository snapshot into an evidence-backed current-state system specification and a proposed AEP plan. Return Markdown with system boundaries, interfaces, invariants, risks, and sequenced work. Cite repository paths from the supplied context and distinguish observed facts from proposals."
        }
        _ => {
            "Analyze this exact repository snapshot and return an evidence-backed Markdown report."
        }
    }
}

fn workflow_definitions() -> Vec<WorkflowDefinition> {
    vec![
        WorkflowDefinition {
            id: "review.code/v1".to_owned(),
            name: "Code review".to_owned(),
            description:
                "Commit-pinned correctness and maintainability findings with file citations."
                    .to_owned(),
        },
        WorkflowDefinition {
            id: "review.security/v1".to_owned(),
            name: "Security review".to_owned(),
            description: "Commit-pinned security findings with typed severity and evidence."
                .to_owned(),
        },
        WorkflowDefinition {
            id: "reverse.aep-ess/v1".to_owned(),
            name: "Reverse AEP + ESS".to_owned(),
            description:
                "Evidence-backed draft planning entities and a current-state system specification."
                    .to_owned(),
        },
    ]
}

fn project_id(tenant: &str, instance: &str, project: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"b10x/workspace/project/v1\0");
    digest.update(tenant.as_bytes());
    digest.update(b"\0");
    digest.update(instance.as_bytes());
    digest.update(b"\0");
    digest.update(project.as_bytes());
    format!("project-{}", &hex::encode(digest.finalize())[..32])
}

fn valid_ref(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn valid_branch(value: &str) -> bool {
    valid_ref(value)
        && !value.starts_with('-')
        && !value.contains("..")
        && !value.contains(['~', '^', ':', '?', '*', '[', '\\'])
}

fn valid_commit(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn problem(status: StatusCode, code: &str) -> Response {
    confidential(
        (
            status,
            Json(Problem {
                code: code.to_owned(),
            }),
        )
            .into_response(),
    )
}

fn store_problem(error: &StoreError) -> Response {
    match error {
        StoreError::NotFound => problem(StatusCode::NOT_FOUND, "workspace_resource_not_found"),
        StoreError::Conflict => problem(StatusCode::CONFLICT, "workspace_conflict"),
        StoreError::Database(_) | StoreError::Configuration | StoreError::Corrupt => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "workspace_store_unavailable",
        ),
    }
}

fn substrate_problem(error: &SubstrateError) -> Response {
    match error {
        SubstrateError::Refusal(refusal) if refusal.code == "resource.not-found" => {
            problem(StatusCode::NOT_FOUND, "workspace_file_not_found")
        }
        SubstrateError::Refusal(refusal) if refusal.code == "workspace.stale-content" => {
            problem(StatusCode::CONFLICT, "workspace_file_stale")
        }
        SubstrateError::Refusal(refusal) => match refusal.class {
            SubstrateRefusalClass::Refused => {
                problem(StatusCode::UNPROCESSABLE_ENTITY, "substrate_refused")
            }
            SubstrateRefusalClass::Conflict => problem(StatusCode::CONFLICT, "substrate_conflict"),
            SubstrateRefusalClass::Unserved => problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "substrate_capability_unavailable",
            ),
            SubstrateRefusalClass::Exhausted => problem(
                StatusCode::TOO_MANY_REQUESTS,
                "substrate_capacity_exhausted",
            ),
            SubstrateRefusalClass::Failed => problem(StatusCode::BAD_GATEWAY, "substrate_failed"),
        },
        SubstrateError::UnknownOperation { .. } => {
            problem(StatusCode::SERVICE_UNAVAILABLE, "substrate_outcome_unknown")
        }
        SubstrateError::Transport(_)
        | SubstrateError::TokenUnavailable
        | SubstrateError::Startup(_)
        | SubstrateError::Shutdown(_) => {
            problem(StatusCode::SERVICE_UNAVAILABLE, "substrate_unavailable")
        }
        SubstrateError::Protocol(_)
        | SubstrateError::Builder { .. }
        | SubstrateError::EventGap { .. }
        | SubstrateError::ContractMismatch { .. } => {
            problem(StatusCode::BAD_GATEWAY, "substrate_protocol_invalid")
        }
        _ => problem(StatusCode::BAD_GATEWAY, "substrate_protocol_invalid"),
    }
}

fn confidential(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static header"),
    );
    response
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use connectors_client::operation::OwnerContext;
    use sha2::Digest as _;

    use super::{
        AppState, Authority, CompleteWorkspaceFile, MaterializationWorkers, MaterializedFile,
        SUBSTRATE_SCOPE, WorkflowObservation, WorkflowObservers, WorkflowTaskOutcome,
        agentide_grants, agentide_session_row, canonical_diff, coding_intent_profile,
        diff_pin_content, file_operation_id, install_crypto_provider, language_for_path,
        repository_candidate, resume_workflow_completions, sha256_text, source_file_input,
        source_manifest_sha256, spawn_workflow_completion, strict_repository_entry,
        submit_workflow_task, terminal_grant_row_matches, terminal_session_row_matches,
        valid_repository_path, validate_identity_transport, workflow_task_outcome,
    };
    use crate::store::Store;
    use agent_platform_client::AgentPlatformClient;
    use agent_platform_core::{AgentId, SubmitTask, TaskId, TaskStatus};
    use agentide_contracts::{ActorContext, ActorKind, authorize_intent};
    use axum::Json;
    use axum::Router;
    use axum::http::{HeaderMap, header};
    use axum::response::IntoResponse as _;
    use axum::routing::{get, post};
    use workspace_core::{
        ChangeSelector, CodingSession, CodingSessionState, DiffMode, FileExpectedState,
        MaterializationLimits, Project, StartWorkflow, WorkflowRunState, WriteFile,
    };
    use workspace_service::terminal::{TerminalBrokers, TerminalProfiles, TerminalReplayHub};

    #[test]
    fn installs_the_process_crypto_provider_before_tls_clients_are_built() {
        install_crypto_provider().expect("AWS-LC provider should install");
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }

    #[tokio::test]
    async fn materialization_workers_are_single_flight_and_recoverable() {
        let workers = MaterializationWorkers::default();
        assert!(!workers.is_active("session-one").await);
        assert!(workers.begin("session-one").await);
        assert!(workers.is_active("session-one").await);
        assert!(!workers.begin("session-one").await);
        workers.finish("session-one").await;
        assert!(workers.begin("session-one").await);
    }

    #[test]
    fn workflow_task_terminal_states_project_to_named_safe_results() {
        assert_eq!(
            workflow_task_outcome(TaskStatus::Succeeded, None),
            WorkflowTaskOutcome::Succeeded(
                "# Workflow result\n\nThe workflow completed without a Markdown result.".to_owned()
            )
        );
        for (status, code) in [
            (TaskStatus::Failed, "workflow_execution_failed"),
            (TaskStatus::Cancelled, "workflow_execution_cancelled"),
            (TaskStatus::Refused, "workflow_execution_refused"),
            (TaskStatus::OutcomeUnknown, "workflow_outcome_unknown"),
        ] {
            assert_eq!(
                workflow_task_outcome(status, None),
                WorkflowTaskOutcome::Failed(code)
            );
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // Full restart seam includes durable store and HTTP task recovery.
    async fn persisted_workflow_task_resumes_with_fresh_observer_and_session() {
        let database_url = "sqlite:file:workflow-observer-restart?mode=memory&cache=shared";
        let original = Store::connect_lazy(database_url).expect("original store");
        original.ready().await.expect("original schema");
        let authority = Authority {
            tenant_id: "tenant-one".to_owned(),
            subject: "person:owner".to_owned(),
            connector_bearer: "not-retained".to_owned(),
            agent_platform_bearer: None,
            session_authorization: "Bearer expired-session".to_owned(),
            context: OwnerContext {
                tenant_id: "tenant-one".to_owned(),
                agent_id: "workspace:test".to_owned(),
                agent_revision: 1,
                authority_snapshot_id: "identity:test".to_owned(),
                authority_snapshot_sha256: "a".repeat(64),
            },
        };
        original
            .open_project(
                &authority,
                &Project {
                    id: "project-workflow-recovery".to_owned(),
                    forge_instance_ref: "connection:git:one".to_owned(),
                    project_ref: "project-workflow-recovery".to_owned(),
                    path_with_namespace: "group/project".to_owned(),
                    name: "project".to_owned(),
                    default_branch: Some("trunk".to_owned()),
                    selected_branch: "trunk".to_owned(),
                    pinned_commit: Some("c".repeat(40)),
                    web_url: "https://git.example.test/group/project".to_owned(),
                },
            )
            .await
            .expect("project");
        let run = original
            .start_workflow(
                &authority,
                "project-workflow-recovery",
                &StartWorkflow {
                    definition_id: "review.code/v1".to_owned(),
                    branch: "trunk".to_owned(),
                    commit: "c".repeat(40),
                    idempotency_key: "restart-observation".to_owned(),
                },
            )
            .await
            .expect("run");
        original
            .record_workflow_task(&authority, &run.id, "task-one")
            .await
            .expect("task reference");

        let restarted = Store::connect_lazy(database_url).expect("restarted store");
        restarted.ready().await.expect("restarted schema");
        let recoverable = restarted
            .recoverable_workflow_tasks(&authority, "project-workflow-recovery")
            .await
            .expect("recoverable task");
        assert_eq!(recoverable.len(), 1);

        let task = serde_json::json!({
            "id": "task-one",
            "tenant_id": "tenant-one",
            "agent_id": "agent-one",
            "agent_revision": 1,
            "capability_profile_id": null,
            "idempotency_key": format!("workspace-workflow:{}", run.id),
            "input": {},
            "status": "succeeded",
            "attempt_id": "attempt-one",
            "output": "# Durable review\n\nNo findings.",
            "actor": "person:owner",
            "executor": null,
            "delegation_id": null,
            "request_id": "request-one",
            "accepted_at_ms": 1,
            "completed_at_ms": 2
        });
        let app = Router::new().route(
            "/v1/tasks/{task_id}",
            get(move |headers: HeaderMap| {
                let task = task.clone();
                async move {
                    assert_eq!(
                        headers
                            .get(header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok()),
                        Some("Bearer fresh-session")
                    );
                    Json(task)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });
        let client =
            AgentPlatformClient::new(&format!("http://{address}")).expect("Agent Platform client");
        spawn_workflow_completion(WorkflowObservation {
            store: restarted.clone(),
            client,
            observers: WorkflowObservers::default(),
            bearer: "Bearer fresh-session".to_owned(),
            tenant_id: authority.tenant_id.clone(),
            subject: authority.subject.clone(),
            run_id: recoverable[0].run_id.clone(),
            task_id: TaskId::new(recoverable[0].task_id.clone()).expect("task id"),
        })
        .await;

        let completed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let observed = restarted
                    .workflow_runs(&authority, "project-workflow-recovery")
                    .await
                    .expect("workflow runs")
                    .remove(0);
                if observed.state == WorkflowRunState::Succeeded {
                    break observed;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("recovered observer completed");
        assert_eq!(completed.failure_code, None);
        assert_eq!(
            completed.output.as_deref(),
            Some("# Durable review\n\nNo findings.")
        );
        server.abort();
    }

    #[tokio::test]
    async fn recoverable_workflow_survives_a_session_without_agent_platform_authority() {
        let store = Store::connect_lazy("sqlite::memory:").expect("store");
        store.ready().await.expect("schema");
        let authority = Authority {
            tenant_id: "tenant-one".to_owned(),
            subject: "person:owner".to_owned(),
            connector_bearer: "not-retained".to_owned(),
            agent_platform_bearer: None,
            session_authorization: "Bearer session-without-agent-platform".to_owned(),
            context: OwnerContext {
                tenant_id: "tenant-one".to_owned(),
                agent_id: "workspace:test".to_owned(),
                agent_revision: 1,
                authority_snapshot_id: "identity:test".to_owned(),
                authority_snapshot_sha256: "a".repeat(64),
            },
        };
        store
            .open_project(
                &authority,
                &Project {
                    id: "project-workflow-recovery".to_owned(),
                    forge_instance_ref: "connection:git:one".to_owned(),
                    project_ref: "project-workflow-recovery".to_owned(),
                    path_with_namespace: "group/project".to_owned(),
                    name: "project".to_owned(),
                    default_branch: Some("trunk".to_owned()),
                    selected_branch: "trunk".to_owned(),
                    pinned_commit: Some("c".repeat(40)),
                    web_url: "https://git.example.test/group/project".to_owned(),
                },
            )
            .await
            .expect("project");
        let run = store
            .start_workflow(
                &authority,
                "project-workflow-recovery",
                &StartWorkflow {
                    definition_id: "review.code/v1".to_owned(),
                    branch: "trunk".to_owned(),
                    commit: "c".repeat(40),
                    idempotency_key: "missing-authority".to_owned(),
                },
            )
            .await
            .expect("run");
        store
            .record_workflow_task(&authority, &run.id, "task-one")
            .await
            .expect("task reference");

        let state = AppState {
            identity: identity_client::IdentityClient::new(
                "http://127.0.0.1:1",
                "urn:b10x:workspace",
            )
            .expect("identity client"),
            connectors: connectors_client::HostedClient::new("http://127.0.0.1:1")
                .expect("connectors client"),
            agent_platform: Some(
                AgentPlatformClient::new("http://127.0.0.1:1").expect("Agent Platform client"),
            ),
            project_agent_model: Some("model:test".to_owned()),
            aep: None,
            substrate: None,
            terminal_profiles: TerminalProfiles::load(None).expect("terminal profiles"),
            terminal_brokers: TerminalBrokers::default(),
            terminal_replay: TerminalReplayHub::default(),
            materialization_workers: MaterializationWorkers::default(),
            workflow_observers: WorkflowObservers::default(),
            store: store.clone(),
        };

        resume_workflow_completions(&state, &authority, "project-workflow-recovery")
            .await
            .expect("missing authority is not a durable failure");

        let observed = store
            .workflow_runs(&authority, "project-workflow-recovery")
            .await
            .expect("workflow runs")
            .remove(0);
        assert_eq!(observed.state, WorkflowRunState::Accepted);
        assert_eq!(observed.failure_code, None);
        assert_eq!(observed.output, None);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // Exercises one complete retryable observation boundary.
    async fn transient_workflow_observation_failure_remains_recoverable() {
        let store = Store::connect_lazy("sqlite::memory:").expect("store");
        store.ready().await.expect("schema");
        let authority = Authority {
            tenant_id: "tenant-one".to_owned(),
            subject: "person:owner".to_owned(),
            connector_bearer: "not-retained".to_owned(),
            agent_platform_bearer: Some("Bearer current-session".to_owned()),
            session_authorization: "Bearer current-session".to_owned(),
            context: OwnerContext {
                tenant_id: "tenant-one".to_owned(),
                agent_id: "workspace:test".to_owned(),
                agent_revision: 1,
                authority_snapshot_id: "identity:test".to_owned(),
                authority_snapshot_sha256: "a".repeat(64),
            },
        };
        store
            .open_project(
                &authority,
                &Project {
                    id: "project-workflow-retry".to_owned(),
                    forge_instance_ref: "connection:git:one".to_owned(),
                    project_ref: "project-workflow-retry".to_owned(),
                    path_with_namespace: "group/project".to_owned(),
                    name: "project".to_owned(),
                    default_branch: Some("trunk".to_owned()),
                    selected_branch: "trunk".to_owned(),
                    pinned_commit: Some("c".repeat(40)),
                    web_url: "https://git.example.test/group/project".to_owned(),
                },
            )
            .await
            .expect("project");
        let run = store
            .start_workflow(
                &authority,
                "project-workflow-retry",
                &StartWorkflow {
                    definition_id: "review.code/v1".to_owned(),
                    branch: "trunk".to_owned(),
                    commit: "c".repeat(40),
                    idempotency_key: "transient-observation".to_owned(),
                },
            )
            .await
            .expect("run");
        store
            .record_workflow_task(&authority, &run.id, "task-one")
            .await
            .expect("task reference");

        let task = serde_json::json!({
            "id": "task-one",
            "tenant_id": "tenant-one",
            "agent_id": "agent-one",
            "agent_revision": 1,
            "capability_profile_id": null,
            "idempotency_key": format!("workspace-workflow:{}", run.id),
            "input": {},
            "status": "succeeded",
            "attempt_id": "attempt-one",
            "output": "# Recovered after a transient read failure",
            "actor": "person:owner",
            "executor": null,
            "delegation_id": null,
            "request_id": "request-one",
            "accepted_at_ms": 1,
            "completed_at_ms": 2
        });
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/v1/tasks/{task_id}",
            get({
                let attempts = attempts.clone();
                move || {
                    let attempts = attempts.clone();
                    let task = task.clone();
                    async move {
                        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                            axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
                        } else {
                            Json(task).into_response()
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });
        let client =
            AgentPlatformClient::new(&format!("http://{address}")).expect("Agent Platform client");
        spawn_workflow_completion(WorkflowObservation {
            store: store.clone(),
            client,
            observers: WorkflowObservers::default(),
            bearer: "Bearer current-session".to_owned(),
            tenant_id: authority.tenant_id.clone(),
            subject: authority.subject.clone(),
            run_id: run.id.clone(),
            task_id: TaskId::new("task-one").expect("task id"),
        })
        .await;

        let terminal = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let observed = store
                    .workflow_runs(&authority, "project-workflow-retry")
                    .await
                    .expect("workflow runs")
                    .remove(0);
                if matches!(
                    observed.state,
                    WorkflowRunState::Succeeded
                        | WorkflowRunState::Failed
                        | WorkflowRunState::Refused
                ) {
                    break observed;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("observer reached a terminal state");
        assert_eq!(terminal.state, WorkflowRunState::Succeeded);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn long_running_workflow_remains_recoverable_when_observation_window_ends() {
        let store = Store::connect_lazy("sqlite::memory:").expect("store");
        store.ready().await.expect("schema");
        let authority = workflow_test_authority();
        let run =
            admitted_workflow_test_run(&store, &authority, "project-long-workflow", "long-running")
                .await;
        store
            .record_workflow_task(&authority, &run.id, "task-long")
            .await
            .expect("task reference");

        let task = workflow_test_task(&run, "task-long", "awaiting_approval", None);
        let app = Router::new().route(
            "/v1/tasks/{task_id}",
            get(move || {
                let task = task.clone();
                async move { Json(task) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });
        let observation = WorkflowObservation {
            store: store.clone(),
            client: AgentPlatformClient::new(&format!("http://{address}"))
                .expect("Agent Platform client"),
            observers: WorkflowObservers::default(),
            bearer: "Bearer current-session".to_owned(),
            tenant_id: authority.tenant_id.clone(),
            subject: authority.subject.clone(),
            run_id: run.id,
            task_id: TaskId::new("task-long").expect("task id"),
        };
        observation
            .observe_window(1, std::time::Duration::ZERO)
            .await;

        let observed = store
            .workflow_runs(&authority, "project-long-workflow")
            .await
            .expect("workflow runs")
            .remove(0);
        assert_eq!(observed.state, WorkflowRunState::Running);
        assert_eq!(observed.failure_code, None);
        assert_eq!(observed.output, None);
        assert_eq!(
            store
                .recoverable_workflow_tasks(&authority, "project-long-workflow")
                .await
                .expect("recoverable workflow")
                .len(),
            1
        );
        server.abort();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // Covers the ambiguous submit and same-key replay boundary.
    async fn ambiguous_workflow_submit_failure_remains_idempotently_retryable() {
        let store = Store::connect_lazy("sqlite::memory:").expect("store");
        store.ready().await.expect("schema");
        let authority = workflow_test_authority();
        let input = StartWorkflow {
            definition_id: "review.code/v1".to_owned(),
            branch: "trunk".to_owned(),
            commit: "c".repeat(40),
            idempotency_key: "ambiguous-submit".to_owned(),
        };
        store
            .open_project(
                &authority,
                &workflow_test_project("project-ambiguous-submit"),
            )
            .await
            .expect("project");
        let run = store
            .start_workflow(&authority, "project-ambiguous-submit", &input)
            .await
            .expect("run");
        let task = workflow_test_task(&run, "task-submit", "accepted", None);
        let attempts = Arc::new(AtomicUsize::new(0));
        let expected_key = format!("workspace-workflow:{}", run.id);
        let app = Router::new().route(
            "/v1/tasks",
            post({
                let attempts = attempts.clone();
                move |Json(request): Json<serde_json::Value>| {
                    let attempts = attempts.clone();
                    let expected_key = expected_key.clone();
                    let task = task.clone();
                    async move {
                        assert_eq!(
                            request
                                .get("idempotency_key")
                                .and_then(serde_json::Value::as_str),
                            Some(expected_key.as_str())
                        );
                        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                            axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
                        } else {
                            Json(task).into_response()
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });
        let client =
            AgentPlatformClient::new(&format!("http://{address}")).expect("Agent Platform client");
        let request = SubmitTask {
            agent_id: AgentId::new("agent-one").expect("agent id"),
            idempotency_key: format!("workspace-workflow:{}", run.id),
            input: serde_json::json!({"kind": "project_conversation"}),
        };

        assert!(
            submit_workflow_task(&client, "Bearer current-session", &request)
                .await
                .is_err()
        );
        let accepted = store
            .workflow_runs(&authority, "project-ambiguous-submit")
            .await
            .expect("workflow runs")
            .remove(0);
        assert_eq!(accepted.state, WorkflowRunState::Accepted);
        assert_eq!(accepted.failure_code, None);
        assert_eq!(accepted.output, None);

        let replay = store
            .start_workflow(&authority, "project-ambiguous-submit", &input)
            .await
            .expect("replayed run");
        assert_eq!(replay.id, run.id);
        let task = submit_workflow_task(&client, "Bearer current-session", &request)
            .await
            .expect("idempotent retry");
        store
            .record_workflow_task(&authority, &run.id, task.id.as_str())
            .await
            .expect("task reference");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            store.workflow_task(&authority, &run.id).await.unwrap(),
            Some("task-submit".to_owned())
        );
        server.abort();
    }

    fn workflow_test_authority() -> Authority {
        Authority {
            tenant_id: "tenant-one".to_owned(),
            subject: "person:owner".to_owned(),
            connector_bearer: "not-retained".to_owned(),
            agent_platform_bearer: Some("Bearer current-session".to_owned()),
            session_authorization: "Bearer current-session".to_owned(),
            context: OwnerContext {
                tenant_id: "tenant-one".to_owned(),
                agent_id: "workspace:test".to_owned(),
                agent_revision: 1,
                authority_snapshot_id: "identity:test".to_owned(),
                authority_snapshot_sha256: "a".repeat(64),
            },
        }
    }

    fn workflow_test_project(id: &str) -> Project {
        Project {
            id: id.to_owned(),
            forge_instance_ref: "connection:git:one".to_owned(),
            project_ref: id.to_owned(),
            path_with_namespace: "group/project".to_owned(),
            name: "project".to_owned(),
            default_branch: Some("trunk".to_owned()),
            selected_branch: "trunk".to_owned(),
            pinned_commit: Some("c".repeat(40)),
            web_url: "https://git.example.test/group/project".to_owned(),
        }
    }

    async fn admitted_workflow_test_run(
        store: &Store,
        authority: &Authority,
        project_id: &str,
        idempotency_key: &str,
    ) -> workspace_core::WorkflowRun {
        store
            .open_project(authority, &workflow_test_project(project_id))
            .await
            .expect("project");
        store
            .start_workflow(
                authority,
                project_id,
                &StartWorkflow {
                    definition_id: "review.code/v1".to_owned(),
                    branch: "trunk".to_owned(),
                    commit: "c".repeat(40),
                    idempotency_key: idempotency_key.to_owned(),
                },
            )
            .await
            .expect("run")
    }

    fn workflow_test_task(
        run: &workspace_core::WorkflowRun,
        task_id: &str,
        status: &str,
        output: Option<&str>,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": task_id,
            "tenant_id": "tenant-one",
            "agent_id": "agent-one",
            "agent_revision": 1,
            "capability_profile_id": null,
            "idempotency_key": format!("workspace-workflow:{}", run.id),
            "input": {},
            "status": status,
            "attempt_id": "attempt-one",
            "output": output,
            "actor": "person:owner",
            "executor": null,
            "delegation_id": null,
            "request_id": "request-one",
            "accepted_at_ms": 1,
            "completed_at_ms": null
        })
    }

    #[test]
    fn public_listener_admits_only_https_or_internal_cluster_identity() {
        let listen = "0.0.0.0:8094".parse().unwrap();
        assert!(
            validate_identity_transport(
                listen,
                "http://devcenter-identity.devcenter.svc.cluster.local:8080"
            )
            .is_ok()
        );
        assert!(validate_identity_transport(listen, "https://identity.example.test").is_ok());
        assert!(validate_identity_transport(listen, "http://identity.example.test").is_err());
    }

    #[test]
    fn repository_candidate_keeps_provider_default_branch() {
        let candidate = repository_candidate(
            &serde_json::json!({
                "id": 42,
                "path_with_namespace": "group/project",
                "name": "project",
                "default_branch": "stable",
                "visibility": "private",
                "web_url": "https://git.example.test/group/project",
                "ignored": "provider detail"
            }),
            "connection:gitlab:one",
        )
        .expect("valid project");

        assert_eq!(candidate.forge_instance_ref, "connection:gitlab:one");
        assert_eq!(candidate.project_ref, "42");
        assert_eq!(candidate.path_with_namespace, "group/project");
        assert_eq!(candidate.default_branch.as_deref(), Some("stable"));
        assert!(candidate.opened_project_id.is_none());
    }

    #[test]
    fn substrate_authority_uses_identity_canonical_runtime_scopes() {
        assert_eq!(SUBSTRATE_SCOPE, "exec observe workspaces");
        let mut scopes = SUBSTRATE_SCOPE.split_ascii_whitespace().collect::<Vec<_>>();
        let admitted = scopes.clone();
        scopes.sort_unstable();
        assert_eq!(admitted, scopes);
        assert!(
            !SUBSTRATE_SCOPE
                .split_ascii_whitespace()
                .any(|scope| scope == "session")
        );
    }

    #[test]
    fn terminal_authority_refuses_actor_session_risk_scope_and_expiry_spoofing() {
        let authority = Authority {
            tenant_id: "tenant-one".to_owned(),
            subject: "person:owner".to_owned(),
            connector_bearer: "not-retained".to_owned(),
            agent_platform_bearer: None,
            session_authorization: "Bearer synthetic-session".to_owned(),
            context: OwnerContext {
                tenant_id: "tenant-one".to_owned(),
                agent_id: "workspace:test".to_owned(),
                agent_revision: 1,
                authority_snapshot_id: "identity:test".to_owned(),
                authority_snapshot_sha256: "a".repeat(64),
            },
        };
        let session = CodingSession {
            id: "workspace-session-one".to_owned(),
            project_id: "project-one".to_owned(),
            source_revision: "b".repeat(40),
            base_materialization_ref: Some("base-one".to_owned()),
            working_materialization_ref: Some("working-one".to_owned()),
            manifest_sha256: Some("c".repeat(64)),
            state: CodingSessionState::Ready,
            failure_code: None,
            limits: MaterializationLimits {
                max_files: 1_000,
                max_total_bytes: 256 * 1024 * 1024,
                max_file_bytes: 180 * 1024,
            },
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        let session_row = serde_json::json!({
            "session_id": "agentide-session-one",
            "workspace_root": "working-one",
            "workspace_session_id": "workspace-session-one",
            "project_id": "project-one",
            "source_revision": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "manifest_digest": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "owner": "person:owner",
            "state": "Active"
        });
        assert!(terminal_session_row_matches(
            &session_row,
            &authority,
            &session,
            "agentide-session-one"
        ));
        let mut spoofed_session = session_row;
        spoofed_session["owner"] = serde_json::json!("person:other");
        assert!(!terminal_session_row_matches(
            &spoofed_session,
            &authority,
            &session,
            "agentide-session-one"
        ));

        let now = "2026-09-03T10:00:00Z".parse().expect("time");
        let grant = serde_json::json!({
            "grant_id": "grant-one",
            "session_id": "agentide-session-one",
            "grantee": "person:owner",
            "state": "Active",
            "maximum_risk": "Medium",
            "allowed_intents": ["interactive_terminal"],
            "path_prefixes": [""],
            "expires_at": "2026-09-03T11:00:00Z"
        });
        assert!(terminal_grant_row_matches(
            &grant,
            &authority,
            "agentide-session-one",
            "grant-one",
            now
        ));
        for (field, value) in [
            ("grantee", serde_json::json!("person:other")),
            ("state", serde_json::json!("Revoked")),
            ("maximum_risk", serde_json::json!("Low")),
            ("path_prefixes", serde_json::json!(["src"])),
            ("expires_at", serde_json::json!("2026-09-03T09:00:00Z")),
        ] {
            let mut refused = grant.clone();
            refused[field] = value;
            assert!(
                !terminal_grant_row_matches(
                    &refused,
                    &authority,
                    "agentide-session-one",
                    "grant-one",
                    now
                ),
                "{field} must be receiver-verified"
            );
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn coding_session_and_agent_grant_are_derived_from_server_records() {
        let authority = Authority {
            tenant_id: "tenant-one".to_owned(),
            subject: "person:owner".to_owned(),
            connector_bearer: "not-retained".to_owned(),
            agent_platform_bearer: None,
            session_authorization: "Bearer synthetic-session".to_owned(),
            context: OwnerContext {
                tenant_id: "tenant-one".to_owned(),
                agent_id: "workspace:test".to_owned(),
                agent_revision: 1,
                authority_snapshot_id: "identity:test".to_owned(),
                authority_snapshot_sha256: "a".repeat(64),
            },
        };
        let session = CodingSession {
            id: "workspace-session-one".to_owned(),
            project_id: "project-one".to_owned(),
            source_revision: "b".repeat(40),
            base_materialization_ref: Some("base-one".to_owned()),
            working_materialization_ref: Some("working-one".to_owned()),
            manifest_sha256: Some("c".repeat(64)),
            state: CodingSessionState::Ready,
            failure_code: None,
            limits: MaterializationLimits {
                max_files: 1_000,
                max_total_bytes: 256 * 1024 * 1024,
                max_file_bytes: 180 * 1024,
            },
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        let session_record = serde_json::json!({
            "session_id": "agentide-session-one",
            "workspace_root": "working-one",
            "objective": "Fix the bounded issue",
            "workspace_session_id": "workspace-session-one",
            "project_id": "project-one",
            "source_revision": "b".repeat(40),
            "manifest_digest": "c".repeat(64),
            "owner": "person:owner",
            "scopes": {"principal": "person:owner", "team": null, "project": null},
            "state": "Active"
        });
        assert_eq!(
            agentide_session_row(
                vec![session_record.clone()],
                &authority,
                &session,
                "agentide-session-one"
            )
            .expect("bound session")
            .objective,
            "Fix the bounded issue"
        );
        let mut spoofed = session_record;
        spoofed["workspace_root"] = serde_json::json!("working-other");
        assert!(
            agentide_session_row(vec![spoofed], &authority, &session, "agentide-session-one")
                .is_err()
        );

        let grants = agentide_grants(
            vec![serde_json::json!({
                "grant_id": "grant-one",
                "session_id": "agentide-session-one",
                "grantee": "agent-one",
                "allowed_intents": ["code_edit"],
                "path_prefixes": ["src"],
                "maximum_risk": "Medium",
                "expires_at": null,
                "revision": 1,
                "owner": "person:owner",
                "scopes": {"principal": "person:owner", "team": null, "project": null},
                "state": "Active"
            })],
            &authority,
            "agentide-session-one",
        )
        .expect("grant projection");
        let actor = ActorContext::new(ActorKind::Agent, "agent-one").expect("actor");
        let (profile, implemented) = coding_intent_profile().expect("profile");
        let edit = profile.find("code_edit").expect("edit intent");
        assert!(
            authorize_intent(
                edit,
                "agentide-session-one",
                &actor,
                &implemented,
                &grants,
                true,
                chrono::Utc::now(),
                Some("src/lib.rs")
            )
            .is_ok()
        );
        assert!(
            authorize_intent(
                edit,
                "agentide-session-one",
                &actor,
                &implemented,
                &grants,
                true,
                chrono::Utc::now(),
                Some("tests/escape.rs")
            )
            .is_err()
        );
    }

    #[test]
    fn materialization_tree_parser_refuses_git_metadata_and_non_regular_entries() {
        assert!(!valid_repository_path(".git/config"));
        assert!(!valid_repository_path("src/../secret"));
        assert!(!valid_repository_path("/absolute"));
        assert!(
            strict_repository_entry(&serde_json::json!({
                "id": "abc",
                "name": "link",
                "path": "link",
                "type": "blob",
                "mode": "120000"
            }))
            .is_err()
        );
        assert!(
            strict_repository_entry(&serde_json::json!({
                "id": "abc",
                "name": "dependency",
                "path": "dependency",
                "type": "commit",
                "mode": "160000"
            }))
            .is_err()
        );
    }

    #[test]
    fn source_file_input_preserves_nested_repository_paths() {
        let commit = "a".repeat(40);
        let input = source_file_input(42, ".github/workflows/check.yml", &commit);

        assert_eq!(input["project_id"], 42);
        assert_eq!(input["file_path"], ".github/workflows/check.yml");
        assert_eq!(input["ref"], commit);
        assert_ne!(input["file_path"], ".github%2Fworkflows%2Fcheck.yml");
    }

    #[test]
    fn source_manifest_commits_to_revision_content_and_executable_mode() {
        let ordinary = MaterializedFile {
            path: "tool".to_owned(),
            bytes: b"hello".to_vec(),
            sha256: hex::encode(sha2::Sha256::digest(b"hello")),
            executable: false,
        };
        let mut executable = ordinary.clone();
        executable.executable = true;

        let first = source_manifest_sha256(&"a".repeat(40), std::slice::from_ref(&ordinary));
        let changed_mode =
            source_manifest_sha256(&"a".repeat(40), std::slice::from_ref(&executable));
        let changed_revision = source_manifest_sha256(&"b".repeat(40), &[ordinary]);

        assert_ne!(first, changed_mode);
        assert_ne!(first, changed_revision);
    }

    #[test]
    fn canonical_diff_is_deterministic_structured_and_mode_specific() {
        let file = |bytes: &[u8]| CompleteWorkspaceFile {
            bytes: bytes.to_vec(),
            sha256: hex::encode(sha2::Sha256::digest(bytes)),
            size: bytes.len() as u64,
        };
        let base = std::collections::BTreeMap::from([
            ("src/lib.rs".to_owned(), file(b"one\ntwo\n")),
            ("old.txt".to_owned(), file(b"removed\n")),
        ]);
        let working = std::collections::BTreeMap::from([
            ("src/lib.rs".to_owned(), file(b"one\nchanged\n")),
            ("new.txt".to_owned(), file(b"added\n")),
        ]);
        let selector = ChangeSelector::Workspace;
        let patch = canonical_diff(
            &selector,
            DiffMode::Patch,
            &"a".repeat(40),
            &base,
            &working,
            "person-a",
        );
        let repeated = canonical_diff(
            &selector,
            DiffMode::Patch,
            &"a".repeat(40),
            &base,
            &working,
            "person-a",
        );
        assert_eq!(patch, repeated);
        assert_eq!(patch.files.len(), 3);
        assert_eq!(patch.additions, 2);
        assert_eq!(patch.deletions, 2);
        assert!(patch.files.iter().any(|file| {
            file.new_path.as_deref() == Some("src/lib.rs")
                && file.hunks.iter().any(|hunk| {
                    hunk.lines
                        .iter()
                        .any(|line| line.kind == "addition" && line.new_line == Some(2))
                })
        }));
        let files_only = canonical_diff(
            &selector,
            DiffMode::FilesOnly,
            &"a".repeat(40),
            &base,
            &working,
            "person-a",
        );
        assert_ne!(patch.digest, files_only.digest);
        assert!(files_only.files.iter().all(|file| {
            file.additions.is_none() && file.deletions.is_none() && file.hunks.is_empty()
        }));

        let changed = patch
            .files
            .iter()
            .find(|file| file.new_path.as_deref() == Some("src/lib.rs"))
            .expect("changed file");
        let hunk = changed.hunks.first().expect("hunk");
        let reference = format!("workspace-diff/{}/src/lib.rs/{}", patch.digest, hunk.id);
        let content = diff_pin_content(&patch, &reference).expect("canonical hunk selection");
        assert_eq!(
            sha256_text(&content),
            hex::encode(sha2::Sha256::digest(content.as_bytes()))
        );
        assert!(
            diff_pin_content(
                &patch,
                &format!("workspace-diff/{}/src/lib.rs/{}", "0".repeat(64), hunk.id)
            )
            .is_none()
        );
    }

    #[test]
    fn file_operations_are_sealed_to_expected_state_and_content() {
        let input = WriteFile {
            content: "hello".to_owned(),
            expected: FileExpectedState::Absent,
            create_parents: true,
            operation_id: "save-one".to_owned(),
        };
        let first = file_operation_id("session", "src/lib.rs", &input);
        let mut changed = input.clone();
        changed.content = "goodbye".to_owned();
        assert_ne!(first, file_operation_id("session", "src/lib.rs", &changed));
        assert_eq!(language_for_path("src/lib.rs"), Some("rust"));
        assert_eq!(language_for_path("Makefile"), None);
    }
}
