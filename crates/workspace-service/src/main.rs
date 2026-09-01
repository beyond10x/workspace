#![forbid(unsafe_code)]

use std::net::SocketAddr;

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, serve};
use clap::Parser;
use connectors_client::HostedClient;
use connectors_client::datasource::{
    BindingSearchRequest, DatasourceRead, DatasourceRequest, DatasourceResult, DescribeRequest,
    ReadRequest,
};
use connectors_client::operation::OwnerContext;
use identity_client::{IdentityClient, SessionAuthority};
use serde_json::Value;
use sha2::{Digest, Sha256};
use workspace_core::{
    Branch, CreateMessage, CreateThread, OpenProject, Problem, Project, RepositoryCandidate,
    SelectBranch, StartWorkflow, WorkflowDefinition,
};

mod store;

use store::{Store, StoreError};

const CONNECTORS_AUDIENCE: &str = "urn:b10x:connectors";
const CONNECTORS_SCOPE: &str = "connectors.catalog.read";

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
    store: Store,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.listen.ip().is_unspecified() && args.identity_origin.starts_with("http://") {
        bail!("an HTTP Identity origin is allowed only with a non-public listener");
    }
    let identity = IdentityClient::new(&args.identity_origin, &args.identity_audience)
        .context("invalid Identity configuration")?;
    let connectors =
        HostedClient::new(&args.connectors_api_base).context("invalid Connectors configuration")?;
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
            store,
        }),
    )
    .with_graceful_shutdown(shutdown())
    .await
    .context("Workspace HTTP server failed")?;
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

async fn repositories(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let authority = match authenticate(&state, &headers).await {
        Ok(authority) => authority,
        Err(response) => return response,
    };
    match discover_projects(&state, &authority).await {
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
    let candidate = match discover_projects(&state, &authority).await {
        Ok(projects) => projects.into_iter().find(|project| {
            project.forge_instance_ref == input.forge_instance_ref
                && project.project_ref == input.project_ref
        }),
        Err(response) => return response,
    };
    let Some(candidate) = candidate else {
        return problem(StatusCode::FORBIDDEN, "project_access_refused");
    };
    let default_branch_fallback = candidate.default_branch.as_deref() != Some("main");
    let selected_branch = if default_branch_fallback {
        candidate
            .default_branch
            .clone()
            .unwrap_or_else(|| "main".to_owned())
    } else {
        "main".to_owned()
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
        default_branch_fallback,
        web_url: candidate.web_url,
    };
    match state.store.open_project(&authority, &project).await {
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
    match state
        .store
        .create_message(&authority, &thread_id, &input)
        .await
    {
        Ok(message) => confidential(Json(message).into_response()),
        Err(error) => store_problem(&error),
    }
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
    let visible = discover_projects(state, authority)
        .await?
        .into_iter()
        .any(|candidate| {
            candidate.forge_instance_ref == project.forge_instance_ref
                && candidate.project_ref == project.project_ref
        });
    if !visible {
        return Err(problem(StatusCode::FORBIDDEN, "project_access_refused"));
    }
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
        context: OwnerContext {
            tenant_id: session.tenant_id,
            agent_id: format!("workspace:{}", session.subject),
            agent_revision: 1,
            authority_snapshot_id: format!("identity:{snapshot}"),
            authority_snapshot_sha256: snapshot,
        },
    })
}

async fn discover_projects(
    state: &AppState,
    authority: &Authority,
) -> Result<Vec<RepositoryCandidate>, Response> {
    let description = describe(state, authority, "gitlab.projects").await?;
    let bindings = bindings(state, authority, "gitlab.projects", "").await?;
    let mut projects = Vec::new();
    for binding in bindings {
        let mut cursor = None;
        loop {
            let result = datasource(
                state,
                authority,
                DatasourceRequest::Read(ReadRequest {
                    datasource_ref: "gitlab.projects".to_owned(),
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
                let Some(project_ref) = value
                    .get("id")
                    .and_then(Value::as_u64)
                    .map(|id| id.to_string())
                else {
                    continue;
                };
                let Some(path) = value.get("path_with_namespace").and_then(Value::as_str) else {
                    continue;
                };
                projects.push(RepositoryCandidate {
                    forge_instance_ref: binding.connection_ref.clone(),
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
                });
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
    }
    projects.sort_by(|left, right| left.path_with_namespace.cmp(&right.path_with_namespace));
    projects.dedup_by(|left, right| {
        left.forge_instance_ref == right.forge_instance_ref && left.project_ref == right.project_ref
    });
    Ok(projects)
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
