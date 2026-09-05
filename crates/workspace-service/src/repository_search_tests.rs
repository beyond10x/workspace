use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use connectors_client::operation::{self, OperationErrorCode, OperationRequest, OperationResult};
use serde_json::{Value, json};
use tower::ServiceExt as _;

use super::{AppState, Store, router};

fn confidential(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static header"),
    );
    response
        .headers_mut()
        .insert(header::PRAGMA, "no-cache".parse().expect("static header"));
    response
}

#[derive(Clone)]
enum Reply {
    Success(OperationResult),
    Failure(OperationErrorCode),
    HttpFailure(StatusCode),
    MalformedNotFound(&'static str),
}

impl Reply {
    fn respond(&self, request_id: &str) -> Response {
        let envelope = match self {
            Self::Success(result) => {
                operation::ResponseEnvelope::success(request_id, result.clone())
            }
            Self::Failure(code) => operation::ResponseEnvelope::failure(
                request_id,
                operation::OperationError::new(*code, "operation refused", false),
            ),
            Self::HttpFailure(status) => return (*status, "upstream unavailable").into_response(),
            Self::MalformedNotFound(field) => {
                let mut envelope = serde_json::to_value(operation::ResponseEnvelope::failure(
                    request_id,
                    operation::OperationError::new(
                        OperationErrorCode::NotFound,
                        "operation not visible",
                        false,
                    ),
                ))
                .expect("error envelope");
                envelope[field] = match *field {
                    "protocol" | "request_id" => json!("mismatched"),
                    "status" => json!("ok"),
                    "response" => json!({ "result": "search", "value": { "operations": [] } }),
                    _ => Value::Null,
                };
                return confidential(Json(envelope).into_response());
            }
        };
        confidential(Json(envelope).into_response())
    }
}

struct Fixture {
    app: Router,
    state: AppState,
    requests: Arc<Mutex<Vec<OperationRequest>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(describe: Reply, invoke: Reply) -> Self {
        Self::scripted("gitlab-project-list", vec![describe, invoke]).await
    }

    async fn scripted(operation_ref: &'static str, replies: Vec<Reply>) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = requests.clone();
        let replies = Arc::new(Mutex::new(VecDeque::from(replies)));
        let services = Router::new()
            .route("/v1/session-authority", get(session_authority))
            .route("/v1/access-token", post(access_token))
            .route(
                "/api/connectors/v1/operations",
                post(
                    move |headers: HeaderMap, Json(request): Json<operation::RequestEnvelope>| {
                        let observed = observed.clone();
                        let replies = replies.clone();
                        async move {
                            assert_eq!(
                                headers[header::AUTHORIZATION],
                                "Bearer connector-authority"
                            );
                            request.validate().expect("valid operation request");
                            assert_eq!(request.context.tenant_id, "tenant-one");
                            assert_eq!(request.context.agent_id, "workspace:person:owner");
                            observed
                                .lock()
                                .expect("request log")
                                .push(request.request.clone());
                            match request.request {
                                OperationRequest::Describe(params) => {
                                    assert_eq!(params.operation_ref, operation_ref);
                                }
                                OperationRequest::Invoke(params) => {
                                    assert_eq!(params.operation_ref, operation_ref);
                                }
                                other => panic!("unexpected operation request: {other:?}"),
                            }
                            replies
                                .lock()
                                .expect("reply queue")
                                .pop_front()
                                .expect("no unexpected extra Connector calls")
                                .respond(&request.request_id)
                        }
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture listener");
        let origin = format!("http://{}", listener.local_addr().expect("fixture address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, services)
                .await
                .expect("fixture server");
        });
        let store = Store::connect_lazy("sqlite::memory:").expect("store");
        store.ready().await.expect("schema");
        let state = AppState {
            identity: identity_client::IdentityClient::new(&origin, "urn:b10x:workspace")
                .expect("Identity client"),
            connectors: connectors_client::HostedClient::new(&format!(
                "{origin}/api/connectors/v1"
            ))
            .expect("Connector client"),
            agent_platform: None,
            project_agent_model: None,
            aep: None,
            substrate: None,
            terminal_profiles: super::TerminalProfiles::load(None).expect("terminal profiles"),
            terminal_brokers: super::TerminalBrokers::default(),
            terminal_replay: super::TerminalReplayHub::default(),
            materialization_workers: super::MaterializationWorkers::default(),
            workflow_observers: super::WorkflowObservers::default(),
            store,
        };
        let app = router(state.clone());
        Self {
            app,
            state,
            requests,
            server,
        }
    }

    async fn repositories(&self, uri: &str) -> (StatusCode, Value) {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::AUTHORIZATION, "Bearer identity-session")
                    .body(Body::empty())
                    .expect("repository request"),
            )
            .await
            .expect("Workspace response");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body");
        (
            status,
            serde_json::from_slice(&body).expect("JSON response"),
        )
    }

    fn requests(&self) -> Vec<OperationRequest> {
        self.requests.lock().expect("request log").clone()
    }
}

mod branch_discovery {
    use super::*;
    use crate::{Authority, discover_branches};
    use workspace_core::Project;

    fn project() -> Project {
        Project {
            id: "project-one".to_owned(),
            forge_instance_ref: "connection:gitlab:admitted".to_owned(),
            project_ref: "42".to_owned(),
            path_with_namespace: "group/project".to_owned(),
            name: "Project".to_owned(),
            default_branch: Some("trunk".to_owned()),
            selected_branch: "trunk".to_owned(),
            pinned_commit: Some("a".repeat(40)),
            web_url: "https://git.example.test/group/project".to_owned(),
        }
    }

    async fn authority(fixture: &Fixture) -> Authority {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer identity-session".parse().unwrap(),
        );
        crate::authenticate(&fixture.state, &headers)
            .await
            .expect("current Identity authority")
    }

    fn description(witness: &str, connection_ref: &str) -> Reply {
        let Reply::Success(OperationResult::Describe(mut description)) = admitted_description()
        else {
            unreachable!();
        };
        description.operation_ref = "gitlab-branch-list".to_owned();
        description.description_ref = witness.to_owned();
        description.connections[0].connection_ref = connection_ref.to_owned();
        Reply::Success(OperationResult::Describe(description))
    }

    fn page(output: Value) -> Reply {
        let Reply::Success(OperationResult::Invoke(mut result)) = invocation(output) else {
            unreachable!();
        };
        result.operation_ref = "gitlab-branch-list".to_owned();
        Reply::Success(OperationResult::Invoke(result))
    }

    fn rows(count: usize) -> Value {
        json!(
            (0..count)
                .map(|index| json!({
                    "name": format!("branch-{index}"),
                    "commit": { "id": format!("{index:040x}") },
                    "default": index == 0,
                    "protected": index == 1
                }))
                .collect::<Vec<_>>()
        )
    }

    async fn assert_error(response: Response, expected: StatusCode, code: &str) {
        assert_eq!(response.status(), expected);
        let body = to_bytes(response.into_body(), 1024)
            .await
            .expect("problem body");
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["code"],
            code
        );
    }

    #[tokio::test]
    async fn pages_bind_exact_project_connection_and_fresh_descriptions() {
        let project = project();
        let fixture = Fixture::scripted(
            "gitlab-branch-list",
            vec![
                description("description:first", &project.forge_instance_ref),
                page(rows(100)),
                page(json!([{ "name": "topic", "commit": { "id": "b".repeat(40) } }])),
                description("description:second", &project.forge_instance_ref),
                page(json!([])),
            ],
        )
        .await;
        let authority = authority(&fixture).await;
        let branches = discover_branches(&fixture.state, &authority, &project)
            .await
            .expect("all pages");
        assert_eq!(branches.len(), 101);
        assert!(branches[0].provider_default);
        assert!(!branches[0].protected);
        assert!(branches[1].protected);
        assert!(!branches[1].provider_default);
        assert_eq!(branches[100].name, "topic");
        assert_eq!(branches[100].commit, "b".repeat(40));
        assert!(!branches[100].provider_default);
        assert!(!branches[100].protected);
        let refreshed = discover_branches(&fixture.state, &authority, &project)
            .await
            .expect("fresh read");
        assert!(refreshed.is_empty());
        let requests = fixture.requests();
        let [
            OperationRequest::Describe(_),
            OperationRequest::Invoke(first),
            OperationRequest::Invoke(second),
            OperationRequest::Describe(_),
            OperationRequest::Invoke(refreshed),
        ] = requests.as_slice()
        else {
            panic!(
                "only branch operations, with fresh descriptions and short-page termination: {requests:?}"
            );
        };
        for (invoke, page, witness) in [
            (first, 1, "description:first"),
            (second, 2, "description:first"),
            (refreshed, 1, "description:second"),
        ] {
            assert_eq!(invoke.connection_ref, project.forge_instance_ref);
            assert_eq!(invoke.description_ref, witness);
            assert_eq!(
                invoke.input,
                json!({"project_id": 42, "page": page, "per_page": 100})
            );
            assert_eq!(invoke.approval_evidence_ref, None);
        }
    }

    #[tokio::test]
    async fn an_exact_full_page_requires_a_final_empty_page() {
        let project = project();
        let fixture = Fixture::scripted(
            "gitlab-branch-list",
            vec![
                description("description:current", &project.forge_instance_ref),
                page(rows(100)),
                page(json!([])),
            ],
        )
        .await;
        let authority = authority(&fixture).await;
        let branches = discover_branches(&fixture.state, &authority, &project)
            .await
            .expect("complete list");
        assert_eq!(branches.len(), 100);
        assert_eq!(fixture.requests().len(), 3);
    }

    #[tokio::test]
    async fn revoked_connection_discards_pages_and_is_not_reused_on_the_next_read() {
        let project = project();
        let fixture = Fixture::scripted(
            "gitlab-branch-list",
            vec![
                description("description:before-revocation", &project.forge_instance_ref),
                page(rows(100)),
                Reply::Failure(OperationErrorCode::NotGranted),
                description("description:after-revocation", "connection:gitlab:other"),
            ],
        )
        .await;
        let authority = authority(&fixture).await;

        let error = discover_branches(&fixture.state, &authority, &project)
            .await
            .expect_err("revocation cannot produce a partial branch list");
        assert_error(error, StatusCode::FORBIDDEN, "connector_access_refused").await;

        let error = discover_branches(&fixture.state, &authority, &project)
            .await
            .expect_err("the next read must recheck the connection");
        assert_error(error, StatusCode::FORBIDDEN, "project_access_refused").await;
        let requests = fixture.requests();
        assert!(matches!(
            requests.as_slice(),
            [
                OperationRequest::Describe(_),
                OperationRequest::Invoke(_),
                OperationRequest::Invoke(_),
                OperationRequest::Describe(_),
            ]
        ));
    }

    #[tokio::test]
    async fn the_project_connection_must_be_currently_admitted() {
        let fixture = Fixture::scripted(
            "gitlab-branch-list",
            vec![description(
                "description:current",
                "connection:gitlab:other",
            )],
        )
        .await;
        let authority = authority(&fixture).await;
        let error = discover_branches(&fixture.state, &authority, &project())
            .await
            .expect_err("unadmitted connection");
        assert_error(error, StatusCode::FORBIDDEN, "project_access_refused").await;
        assert_eq!(fixture.requests().len(), 1);
    }

    #[tokio::test]
    async fn missing_or_stale_branch_authority_remains_an_error() {
        for (code, status, problem) in [
            (
                OperationErrorCode::NotFound,
                StatusCode::BAD_GATEWAY,
                "connector_read_refused",
            ),
            (
                OperationErrorCode::NotGranted,
                StatusCode::FORBIDDEN,
                "connector_access_refused",
            ),
            (
                OperationErrorCode::StaleAuthority,
                StatusCode::CONFLICT,
                "connector_authority_stale",
            ),
        ] {
            let fixture = Fixture::scripted("gitlab-branch-list", vec![Reply::Failure(code)]).await;
            let authority = authority(&fixture).await;
            let error = discover_branches(&fixture.state, &authority, &project())
                .await
                .expect_err("authority failure");
            assert_error(error, status, problem).await;
            assert_eq!(fixture.requests().len(), 1);
        }
    }

    #[tokio::test]
    async fn later_page_failures_never_return_a_partial_list() {
        for (code, status, problem) in [
            (
                OperationErrorCode::Unavailable,
                StatusCode::BAD_GATEWAY,
                "connector_read_refused",
            ),
            (
                OperationErrorCode::NotFound,
                StatusCode::BAD_GATEWAY,
                "connector_read_refused",
            ),
            (
                OperationErrorCode::StaleAuthority,
                StatusCode::CONFLICT,
                "connector_authority_stale",
            ),
        ] {
            let project = project();
            let fixture = Fixture::scripted(
                "gitlab-branch-list",
                vec![
                    description("description:current", &project.forge_instance_ref),
                    page(rows(100)),
                    Reply::Failure(code),
                ],
            )
            .await;
            let authority = authority(&fixture).await;
            let error = discover_branches(&fixture.state, &authority, &project)
                .await
                .expect_err("no partial result");
            assert_error(error, status, problem).await;
            assert_eq!(fixture.requests().len(), 3);
        }
    }

    #[tokio::test]
    async fn malformed_later_pages_never_return_a_partial_list() {
        for malformed in [
            json!({ "branches": [] }),
            json!([{"name": "topic"}]),
            json!([{"name": "topic", "commit": {}}]),
            json!([{"name": 42, "commit": {"id": "a".repeat(40)}}]),
            json!([{"name": "topic", "commit": {"id": 42}}]),
            json!([{"name": "topic", "commit": {"id": "a".repeat(40)}, "default": "true"}]),
            json!([{"name": "topic", "commit": {"id": "a".repeat(40)}, "protected": null}]),
            rows(101),
        ] {
            let project = project();
            let fixture = Fixture::scripted(
                "gitlab-branch-list",
                vec![
                    description("description:current", &project.forge_instance_ref),
                    page(rows(100)),
                    page(malformed),
                ],
            )
            .await;
            let authority = authority(&fixture).await;
            let error = discover_branches(&fixture.state, &authority, &project)
                .await
                .expect_err("invalid page");
            assert_error(error, StatusCode::BAD_GATEWAY, "connector_protocol_invalid").await;
            assert_eq!(fixture.requests().len(), 3);
        }
    }

    #[tokio::test]
    async fn project_ids_are_positive_integers_before_connector_calls() {
        for reference in ["group/project", "0", "-1", "18446744073709551616"] {
            let fixture = Fixture::scripted("gitlab-branch-list", Vec::new()).await;
            let authority = authority(&fixture).await;
            let project = Project {
                project_ref: reference.to_owned(),
                ..project()
            };
            let error = discover_branches(&fixture.state, &authority, &project)
                .await
                .expect_err("invalid numeric project");
            assert_error(
                error,
                StatusCode::UNPROCESSABLE_ENTITY,
                "project_reference_invalid",
            )
            .await;
            assert!(fixture.requests().is_empty());
        }
    }
}

async fn session_authority(headers: HeaderMap) -> Response {
    assert_eq!(headers[header::AUTHORIZATION], "Bearer identity-session");
    confidential(
        Json(json!({
            "iss": "https://identity.example.test",
            "sub": "person:owner",
            "aud": "urn:b10x:workspace",
            "exp": 4_102_444_800_i64,
            "email": null,
            "tenant_id": "tenant-one",
            "groups": []
        }))
        .into_response(),
    )
}

async fn access_token(headers: HeaderMap, Json(request): Json<Value>) -> Response {
    assert_eq!(headers[header::AUTHORIZATION], "Bearer identity-session");
    assert_eq!(
        request,
        json!({
            "audience": super::CONNECTORS_AUDIENCE,
            "scope": super::CONNECTORS_SCOPE
        })
    );
    confidential(
        Json(json!({
            "access_token": "connector-authority",
            "token_type": "Bearer",
            "expires_in": 300,
            "audience": super::CONNECTORS_AUDIENCE,
            "scope": super::CONNECTORS_SCOPE
        }))
        .into_response(),
    )
}

fn admitted_description() -> Reply {
    Reply::Success(OperationResult::Describe(operation::OperationDescription {
        operation_ref: "gitlab-project-list".to_owned(),
        title: "List GitLab projects".to_owned(),
        description: "List repositories visible to the connected person.".to_owned(),
        input_schema: json!({}),
        output_schema: json!({}),
        effect: operation::EffectClass::ReadOnly,
        approval: operation::ApprovalPosture::NotRequired,
        connections: vec![operation::ConnectionSummary {
            connection_ref: "connection:gitlab:admitted".to_owned(),
            label: "GitLab".to_owned(),
            provider: "gitlab".to_owned(),
            audiences: Vec::new(),
            purpose: None,
        }],
        description_ref: "description:current".to_owned(),
    }))
}

fn invocation(output: Value) -> Reply {
    Reply::Success(OperationResult::Invoke(operation::InvocationResult {
        operation_ref: "gitlab-project-list".to_owned(),
        output,
        connector_audit_ref: "audit:listing".to_owned(),
        execution_ref: None,
    }))
}

#[tokio::test]
async fn absent_gitlab_operation_returns_empty_repositories_without_invoking() {
    let fixture = Fixture::new(
        Reply::Failure(OperationErrorCode::NotFound),
        invocation(json!([])),
    )
    .await;
    let (status, body) = fixture.repositories("/v1/repositories").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "missing connection should be recoverable: {body}"
    );
    assert_eq!(body, json!([]));
    assert!(matches!(
        fixture.requests().as_slice(),
        [OperationRequest::Describe(_)]
    ));
}

#[tokio::test]
async fn a_newly_connected_operation_is_discovered_after_an_empty_recovery_result() {
    let fixture = Fixture::scripted(
        "gitlab-project-list",
        vec![
            Reply::Failure(OperationErrorCode::NotFound),
            admitted_description(),
            invocation(json!([{
                "id": 42,
                "path_with_namespace": "group/project",
                "name": "Project",
                "default_branch": "trunk"
            }])),
        ],
    )
    .await;
    let (status, body) = fixture.repositories("/v1/repositories").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));

    let (status, body) = fixture.repositories("/v1/repositories").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the connected retry must be usable: {body}"
    );
    let repositories = body.as_array().expect("connected repository listing");
    assert_eq!(repositories.len(), 1);
    assert_eq!(repositories[0]["project_ref"], "42");
    assert_eq!(
        repositories[0]["forge_instance_ref"],
        "connection:gitlab:admitted"
    );
    assert!(matches!(
        fixture.requests().as_slice(),
        [
            OperationRequest::Describe(_),
            OperationRequest::Describe(_),
            OperationRequest::Invoke(_),
        ]
    ));
}

#[tokio::test]
async fn describe_authority_and_provider_refusals_remain_errors() {
    for (code, status, problem) in [
        (
            OperationErrorCode::NotGranted,
            StatusCode::FORBIDDEN,
            "connector_access_refused",
        ),
        (
            OperationErrorCode::StaleAuthority,
            StatusCode::CONFLICT,
            "connector_authority_stale",
        ),
        (
            OperationErrorCode::Unavailable,
            StatusCode::BAD_GATEWAY,
            "connector_read_refused",
        ),
        (
            OperationErrorCode::Protocol,
            StatusCode::BAD_GATEWAY,
            "connector_read_refused",
        ),
    ] {
        let fixture = Fixture::new(Reply::Failure(code), invocation(json!([]))).await;
        let (observed, body) = fixture.repositories("/v1/repositories").await;
        assert_eq!(observed, status, "{code:?}: {body}");
        assert_eq!(body["code"], problem, "{code:?}");
        assert_eq!(fixture.requests().len(), 1);
    }
}

#[tokio::test]
async fn connector_http_failures_remain_errors() {
    for status in [StatusCode::NOT_FOUND, StatusCode::SERVICE_UNAVAILABLE] {
        let fixture = Fixture::new(Reply::HttpFailure(status), invocation(json!([]))).await;
        let (observed, body) = fixture.repositories("/v1/repositories").await;
        assert_eq!(observed, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "connectors_unavailable");
        assert_eq!(fixture.requests().len(), 1);
    }
}

#[tokio::test]
async fn malformed_describe_not_found_envelopes_remain_errors() {
    for field in ["protocol", "request_id", "status", "response", "error"] {
        let fixture = Fixture::new(Reply::MalformedNotFound(field), invocation(json!([]))).await;
        let (status, body) = fixture.repositories("/v1/repositories").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "malformed {field}");
        assert_eq!(body["code"], "connectors_unavailable", "malformed {field}");
        assert_eq!(fixture.requests().len(), 1);
    }
}

#[tokio::test]
async fn unexpected_describe_result_remains_a_protocol_error() {
    let fixture = Fixture::new(invocation(json!([])), invocation(json!([]))).await;
    let (status, body) = fixture.repositories("/v1/repositories").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["code"], "connector_protocol_invalid");
    assert_eq!(fixture.requests().len(), 1);
}

#[tokio::test]
async fn invocation_not_found_and_provider_failures_never_become_empty_success() {
    for code in [
        OperationErrorCode::NotFound,
        OperationErrorCode::Unavailable,
    ] {
        let fixture = Fixture::new(admitted_description(), Reply::Failure(code)).await;
        let (status, body) = fixture.repositories("/v1/repositories").await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{code:?}");
        assert_eq!(body["code"], "connector_read_refused");
        assert_eq!(fixture.requests().len(), 2);
    }
}

#[tokio::test]
async fn malformed_provider_output_remains_a_protocol_error() {
    let fixture = Fixture::new(
        admitted_description(),
        invocation(json!({ "projects": [] })),
    )
    .await;
    let (status, body) = fixture.repositories("/v1/repositories").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["code"], "connector_protocol_invalid");
    assert_eq!(fixture.requests().len(), 2);
}

#[tokio::test]
async fn connected_repository_search_keeps_current_authority_and_bounded_candidates() {
    let candidates: Vec<_> = (1..=30)
        .map(|id| {
            json!({
                "id": id,
                "path_with_namespace": format!("group/project-{id}"),
                "name": format!("Project {id}"),
                "default_branch": "trunk",
                "visibility": "private",
                "web_url": format!("https://git.example.test/group/project-{id}")
            })
        })
        .collect();
    let fixture = Fixture::new(admitted_description(), invocation(json!(candidates))).await;
    let (status, body) = fixture
        .repositories("/v1/repositories?query=%20project%20")
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let repositories = body.as_array().expect("repository list");
    assert_eq!(repositories.len(), 25);
    assert_eq!(repositories[0]["project_ref"], "1");
    assert_eq!(repositories[24]["project_ref"], "25");
    assert!(repositories.iter().all(|candidate| {
        candidate["forge_instance_ref"] == "connection:gitlab:admitted"
            && candidate["default_branch"] == "trunk"
    }));
    let requests = fixture.requests();
    let [
        OperationRequest::Describe(_),
        OperationRequest::Invoke(invoke),
    ] = requests.as_slice()
    else {
        panic!("connected search must describe then invoke once: {requests:?}");
    };
    assert_eq!(invoke.operation_ref, "gitlab-project-list");
    assert_eq!(invoke.connection_ref, "connection:gitlab:admitted");
    assert_eq!(invoke.description_ref, "description:current");
    assert_eq!(
        invoke.input,
        json!({"membership": true, "page": 1, "per_page": 25, "search": "project"})
    );
    assert_eq!(invoke.approval_evidence_ref, None);
}
