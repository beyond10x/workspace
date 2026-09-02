#![forbid(unsafe_code)]

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
use b10x_substrate_sdk::{AccessToken, Client as SubstrateClient, SdkError as SubstrateError};
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
use workspace_core::{
    Branch, CreateMessage, CreateThread, EngineeringArtifact, EngineeringArtifactPage, MessageRole,
    OpenProject, Problem, Project, RepositoryCandidate, RepositoryEntry, RepositoryEntryKind,
    SelectBranch, StartWorkflow, WorkflowDefinition,
};

mod aep;
mod store;

use aep::{AepTransport, RequestCredential};
use store::{Store, StoreError};

const CONNECTORS_AUDIENCE: &str = "urn:b10x:connectors";
const CONNECTORS_SCOPE: &str = "connectors.catalog.read connectors.invoke";
const SUBSTRATE_AUDIENCE: &str = "urn:b10x:substrate";
const SUBSTRATE_SCOPE: &str = "observe workspaces";
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
    use super::{install_crypto_provider, repository_candidate, validate_identity_transport};

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
}
