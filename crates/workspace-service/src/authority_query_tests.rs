use super::*;
use crate::{Authority, agentide_service_rows, verify_terminal_grant};
use workspace_core::{CodingSession, CodingSessionState, CreateTerminal};

const SESSION: &str = "agentide.get_session";
const GRANTS: &str = "agentide.list_grants";

fn page(items: Value, cursor: Option<&str>) -> Value {
    let through_version = (!items.as_array().unwrap().is_empty()).then_some(7);
    let mut result = json!({"partial": cursor.is_some(), "next_cursor": cursor, "through_version": through_version});
    result["items"] = items;
    result
}

fn query(operation_ref: &'static str, output: Value) -> Vec<(&'static str, Reply)> {
    let Reply::Success(OperationResult::Describe(mut description)) = admitted_description() else {
        unreachable!();
    };
    description.operation_ref = operation_ref.to_owned();
    vec![
        (
            operation_ref,
            Reply::Success(OperationResult::Describe(description)),
        ),
        (
            operation_ref,
            Reply::Success(OperationResult::Invoke(operation::InvocationResult {
                operation_ref: operation_ref.to_owned(),
                output,
                connector_audit_ref: "audit:authority-query".to_owned(),
                execution_ref: None,
            })),
        ),
    ]
}

async fn authority(fixture: &Fixture) -> Authority {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        "Bearer identity-session".parse().unwrap(),
    );
    crate::authenticate(&fixture.state, &headers).await.unwrap()
}

fn binding() -> (CodingSession, CreateTerminal, Value, Value) {
    let session = CodingSession {
        id: "workspace-session-one".to_owned(),
        project_id: "project-one".to_owned(),
        source_revision: "a".repeat(40),
        materialization_ref: Some("working-one".to_owned()),
        manifest_sha256: Some("b".repeat(64)),
        state: CodingSessionState::Ready,
        failure_code: None,
        limits: crate::SOURCE_MATERIALIZATION_LIMITS,
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    let input = CreateTerminal {
        agentide_session_id: "agentide-session-one".to_owned(),
        authority_grant_id: "grant-one".to_owned(),
        profile_id: "shell".to_owned(),
        columns: 80,
        rows: 24,
        idempotency_key: "terminal-one".to_owned(),
    };
    let session_row = json!({
        "session_id": input.agentide_session_id,
        "workspace_root": session.materialization_ref,
        "workspace_session_id": session.id,
        "project_id": session.project_id,
        "source_revision": session.source_revision,
        "manifest_digest": session.manifest_sha256,
        "owner": "person:owner", "state": "Active"
    });
    let grant = json!({
        "grant_id": "grant-one", "session_id": "agentide-session-one",
        "grantee": "person:owner", "state": "Active", "maximum_risk": "Medium",
        "allowed_intents": ["interactive_terminal"], "path_prefixes": [""]
    });
    (session, input, session_row, grant)
}

#[tokio::test]
async fn terminal_authority_finds_session_and_grant_after_empty_filtered_pages() {
    let (session, input, session_row, grant) = binding();
    let replies = [
        query(SESSION, page(json!([]), Some("session-page-2"))),
        query(SESSION, page(json!([]), Some("session-page-3"))),
        query(SESSION, page(json!([session_row]), None)),
        query(GRANTS, page(json!([]), Some("grant-page-2"))),
        query(GRANTS, page(json!([grant]), None)),
    ]
    .concat();
    let fixture = Fixture::scripted_operations(replies).await;
    verify_terminal_grant(&fixture.state, &authority(&fixture).await, &session, &input)
        .await
        .unwrap();
    let calls = fixture
        .requests()
        .into_iter()
        .filter_map(|request| match request {
            OperationRequest::Invoke(params) => Some(params),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 5);
    for (call, cursor) in calls.iter().zip([
        Value::Null,
        json!("session-page-2"),
        json!("session-page-3"),
        Value::Null,
        json!("grant-page-2"),
    ]) {
        assert_eq!(call.input["session_id"], input.agentide_session_id);
        assert_eq!(call.input["$page"]["cursor"], cursor);
        assert_eq!(call.input["$page"]["limit"], 1000);
        assert_eq!(call.description_ref, "description:current");
        assert!(call.approval_evidence_ref.is_none());
    }
}

#[tokio::test]
async fn terminal_authority_later_rows_preserve_binding_and_grant_refusals() {
    let (session, input, session_row, grant) = binding();
    for (is_grant, field, value) in [
        (false, "owner", json!("person:other")),
        (false, "source_revision", json!("c".repeat(40))),
        (true, "grantee", json!("person:other")),
        (true, "state", json!("Revoked")),
        (true, "allowed_intents", json!(["code_read"])),
        (true, "expires_at", json!("2000-01-01T00:00:00Z")),
    ] {
        let mut row = if is_grant {
            grant.clone()
        } else {
            session_row.clone()
        };
        row[field] = value;
        let mut replies = if is_grant {
            query(SESSION, page(json!([session_row]), None))
        } else {
            Vec::new()
        };
        let operation_ref = if is_grant { GRANTS } else { SESSION };
        replies.extend(query(operation_ref, page(json!([]), Some("later"))));
        replies.extend(query(operation_ref, page(json!([row]), None)));
        let fixture = Fixture::scripted_operations(replies).await;
        let response =
            verify_terminal_grant(&fixture.state, &authority(&fixture).await, &session, &input)
                .await
                .unwrap_err();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{field}");
        assert_eq!(fixture.requests().len(), if is_grant { 6 } else { 4 });
    }
}

#[tokio::test]
async fn authority_queries_refuse_malformed_continuations_without_partial_success() {
    for malformed in [
        json!({"items": [], "partial": true}),
        json!({"items": [], "partial": true, "next_cursor": ""}),
        json!({"items": [], "partial": false, "next_cursor": "extra"}),
        json!({"items": [], "partial": false, "next_cursor": 12}),
        json!({"items": [], "next_cursor": null}),
        json!({"items": null, "partial": false}),
        json!([]),
    ] {
        let replies = [
            query(
                SESSION,
                page(json!([{"session_id": "early-match"}]), Some("later")),
            ),
            query(SESSION, malformed.clone()),
        ]
        .concat();
        let fixture = Fixture::scripted_operations(replies).await;
        let response =
            agentide_service_rows(&fixture.state, &authority(&fixture).await, SESSION, "one")
                .await
                .unwrap_err();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY, "{malformed}");
        assert_eq!(fixture.requests().len(), 4);
    }
}

#[tokio::test]
async fn authority_queries_bound_cursor_cycles_empty_pages_and_legacy_rows() {
    let cycles = ["a", "b", "a"]
        .map(|cursor| query(SESSION, page(json!([]), Some(cursor))))
        .concat();
    let fixture = Fixture::scripted_operations(cycles).await;
    assert_eq!(
        agentide_service_rows(&fixture.state, &authority(&fixture).await, SESSION, "one")
            .await
            .unwrap_err()
            .status(),
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(fixture.requests().len(), 6);

    let pages = (0..10)
        .flat_map(|index| query(SESSION, page(json!([]), Some(&format!("page-{index}")))))
        .collect();
    let fixture = Fixture::scripted_operations(pages).await;
    assert_eq!(
        agentide_service_rows(&fixture.state, &authority(&fixture).await, SESSION, "one")
            .await
            .unwrap_err()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(fixture.requests().len(), 20);

    for output in [json!([{"one": 1}]), json!(vec![Value::Null; 10_001])] {
        let fixture = Fixture::scripted_operations(query(SESSION, output.clone())).await;
        let result =
            agentide_service_rows(&fixture.state, &authority(&fixture).await, SESSION, "one").await;
        if output.as_array().unwrap().len() == 1 {
            assert_eq!(result.unwrap(), output.as_array().unwrap().clone());
        } else {
            assert_eq!(result.unwrap_err().status(), StatusCode::PAYLOAD_TOO_LARGE);
        }
    }
}

#[tokio::test]
async fn authority_queries_preserve_snapshot_across_empty_pages_and_refuse_changes() {
    for version in [json!(7), json!(8), Value::Null, json!("7")] {
        let mut last = page(json!([{"session_id": "last"}]), None);
        last["through_version"] = version.clone();
        let fixture = Fixture::scripted_operations(
            [
                query(
                    SESSION,
                    page(json!([{"session_id": "first"}]), Some("empty")),
                ),
                query(SESSION, page(json!([]), Some("last"))),
                query(SESSION, last),
            ]
            .concat(),
        )
        .await;
        let result =
            agentide_service_rows(&fixture.state, &authority(&fixture).await, SESSION, "one").await;
        if version == json!(7) {
            assert_eq!(result.unwrap().len(), 2);
        } else {
            assert_eq!(
                result.unwrap_err().status(),
                if version == json!(8) {
                    StatusCode::CONFLICT
                } else {
                    StatusCode::BAD_GATEWAY
                }
            );
        }
        assert_eq!(fixture.requests().len(), 6);
    }
}

#[tokio::test]
async fn terminal_authority_requires_complete_query_even_after_a_matching_row() {
    let (session, input, session_row, _) = binding();
    let mut replies = query(SESSION, page(json!([session_row]), Some("later")));
    replies.extend(query(SESSION, page(json!([]), None)));
    replies[3].1 = Reply::Failure(OperationErrorCode::NotGranted);
    let fixture = Fixture::scripted_operations(replies).await;
    let response =
        verify_terminal_grant(&fixture.state, &authority(&fixture).await, &session, &input)
            .await
            .unwrap_err();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(fixture.requests().len(), 4);
}
