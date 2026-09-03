#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::SocketAddr;
use std::time::Duration;

use aep_client::AepClient;
use aep_contract::query::{EntityQuery, QueryService};
use agent_platform_client::AgentPlatformClient;
use agent_platform_core::{
    ActivateRevision, AgentId, ConversationInput, ConversationMessage, ConversationRole,
    CreateAgent, ProjectContext, ProjectContextFile, RevisionSpec, SubmitTask, TaskStatus,
};
use anyhow::{Context, Result, bail};
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, serve};
use b10x_substrate_sdk::{
    AccessToken, Client as SubstrateClient, ExecutionPolicy, ExpectedFileState,
    RefusalClass as SubstrateRefusalClass, SdkError as SubstrateError,
    Workspace as SubstrateWorkspace,
};
use base64::Engine as _;
use clap::Parser;
use connectors_client::HostedClient;
use connectors_client::datasource::{
    BindingSearchRequest, DatasourceRead, DatasourceRequest, DatasourceResult, DescribeRequest,
    ReadRequest,
};
use connectors_client::operation::{self, OwnerContext};
use identity_client::{IdentityClient, SessionAuthority};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use similar::{ChangeTag, TextDiff};
use workspace_core::{
    Branch, ChangeSelector, CodingSession, CodingSessionState, CodingTreeEntry,
    CodingTreeProjection, CreateCodingSession, CreateMessage, CreateThread, DiffFile, DiffHunk,
    DiffLine, DiffMode, DiffProjection, DiffRange, EngineeringArtifact, EngineeringArtifactPage,
    FileConflict, FileExpectedState, FileModificationState, FileProjection, FileRevision,
    MaterializationLimits, MessageRole, OpenProject, Problem, Project, RepositoryCandidate,
    RepositoryEntry, RepositoryEntryKind, ResolveDiff, SelectBranch, StartWorkflow,
    WorkflowDefinition, WriteFile,
};

mod aep;
mod store;

use aep::{AepTransport, RequestCredential};
use store::{SessionReservation, Store, StoreError};

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
const MAX_MATERIALIZATION_KEY_BYTES: usize = 256;
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
    store: Store,
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
        .route("/v1/sessions/{session_id}/tree", get(coding_tree))
        .route("/v1/sessions/{session_id}/diff", post(resolve_diff))
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
        .route("/v1/projects/{project_id}/workflows", get(workflows))
        .route(
            "/v1/projects/{project_id}/engineering-artifacts",
            get(engineering_artifacts),
        )
        .route(
            "/v1/projects/{project_id}/workflow-runs",
            post(start_workflow),
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
    if let Err(response) = accessible_project(&state, &authority, &session.project_id).await {
        return response;
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
    let cleanup_unknown = cleanup_materializations(&client, &session).await;
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

    let files = match collect_source_files(&state, &authority, &project).await {
        Ok(files) => files,
        Err(response) => {
            let cleanup_unknown = cleanup_materializations(&client, &session).await;
            let stored = state
                .store
                .refuse_coding_session(
                    &authority,
                    &session.id,
                    if cleanup_unknown {
                        "materialization_cleanup_unknown"
                    } else {
                        "source_materialization_refused"
                    },
                    cleanup_unknown,
                )
                .await;
            return match stored {
                Ok(session) if cleanup_unknown => {
                    confidential(Json(public_session(session)).into_response())
                }
                Ok(_) => response,
                Err(error) => store_problem(&error),
            };
        }
    };
    match provision_materializations(&state, &authority, &client, session, &files).await {
        Ok(ready) => confidential(Json(public_session(ready)).into_response()),
        Err((failed, error)) => {
            let cleanup_unknown = cleanup_materializations(&client, &failed).await;
            let stored = state
                .store
                .refuse_coding_session(
                    &authority,
                    &failed.id,
                    if cleanup_unknown {
                        "materialization_cleanup_unknown"
                    } else {
                        "substrate_materialization_refused"
                    },
                    cleanup_unknown,
                )
                .await;
            match stored {
                Ok(session) if cleanup_unknown => {
                    confidential(Json(public_session(session)).into_response())
                }
                _ => substrate_problem(&error),
            }
        }
    }
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
        Ok(task) => spawn_task_completion(
            state.store.clone(),
            agent_platform.clone(),
            agent_platform_bearer.to_owned(),
            authority.tenant_id,
            authority.subject,
            thread.id,
            task.id,
        ),
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
    let project = match accessible_project(&state, &authority, &project_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    if project.selected_branch != input.branch
        || project.pinned_commit.as_deref() != Some(input.commit.as_str())
    {
        return problem(StatusCode::CONFLICT, "project_snapshot_stale");
    }
    match state
        .store
        .start_workflow(&authority, &project_id, &input)
        .await
    {
        Ok(run) => confidential(Json(run).into_response()),
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
    let mut total_bytes = 0_u64;
    let mut files = Vec::with_capacity(blobs.len());
    for entry in blobs {
        let file =
            read_source_file(state, authority, project, &file_description, &entry, commit).await?;
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
        files.push(file);
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
    let encoded_path =
        url::form_urlencoded::byte_serialize(entry.path.as_bytes()).collect::<String>();
    let output = invoke_operation(
        state,
        authority,
        operation::InvokeRequest {
            operation_ref: "gitlab-repository-file-get".to_owned(),
            connection_ref: project.forge_instance_ref.clone(),
            description_ref: description_ref.to_owned(),
            input: serde_json::json!({
                "project_id": project.project_ref.parse::<u64>().map_err(|_| problem(StatusCode::BAD_GATEWAY, "provider_project_invalid"))?,
                "file_path": encoded_path,
                "ref": commit
            }),
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

async fn provision_materializations(
    state: &AppState,
    authority: &Authority,
    client: &SubstrateClient,
    mut session: CodingSession,
    files: &[MaterializedFile],
) -> Result<CodingSession, (CodingSession, SubstrateError)> {
    let base = ensure_materialization(state, authority, client, &mut session, true).await?;
    let working = ensure_materialization(state, authority, client, &mut session, false).await?;
    upload_materialization(&base, &session.id, "base", files)
        .await
        .map_err(|error| (session.clone(), error))?;
    upload_materialization(&working, &session.id, "working", files)
        .await
        .map_err(|error| (session.clone(), error))?;
    let manifest_sha256 = source_manifest_sha256(&session.source_revision, files);
    state
        .store
        .complete_coding_session(authority, &session.id, &manifest_sha256)
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
    workspace: &SubstrateWorkspace,
    session_id: &str,
    role: &str,
    files: &[MaterializedFile],
) -> Result<(), SubstrateError> {
    for file in files {
        workspace
            .replace_file(
                &file.path,
                &file.bytes,
                ExpectedFileState::Absent,
                true,
                Some(substrate_operation_id(
                    session_id,
                    &["file", role, &file.path, &file.sha256],
                )),
            )
            .await?;
    }
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
                session_id,
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
    use sha2::Digest as _;

    use super::{
        CompleteWorkspaceFile, MaterializedFile, SUBSTRATE_SCOPE, canonical_diff,
        file_operation_id, install_crypto_provider, language_for_path, repository_candidate,
        source_manifest_sha256, strict_repository_entry, valid_repository_path,
        validate_identity_transport,
    };
    use workspace_core::{ChangeSelector, DiffMode, FileExpectedState, WriteFile};

    #[test]
    fn installs_the_process_crypto_provider_before_tls_clients_are_built() {
        install_crypto_provider().expect("AWS-LC provider should install");
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
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
