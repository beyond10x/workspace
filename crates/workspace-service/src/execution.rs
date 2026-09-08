//! Authenticated downward service calls. No task-authority callback lives here.

use axum::extract::OriginalUri;
use bytes::Bytes;
use workspace_client::attestation::{HostRole, PROOF_HEADER, RequestVerifier, VerifiedRequest};
use workspace_core::{ExecutionAttempt, ExecutionAttemptState};

use super::{
    AppState, Authority, HeaderMap, IntoResponse, Json, Path, Response, State, StatusCode,
    authenticate, problem, ready_session_materializations, valid_ref,
};

pub(super) fn load_verifier(
    path: Option<&std::path::Path>,
    role: HostRole,
) -> anyhow::Result<Option<RequestVerifier>> {
    path.map(|path| {
        let key = std::fs::read(path)
            .map_err(|_| anyhow::anyhow!("Workspace host public key is unavailable"))?;
        RequestVerifier::from_pem(role, &key).map_err(anyhow::Error::from)
    })
    .transpose()
}

pub(super) async fn authenticate_host(
    state: &AppState,
    headers: &HeaderMap,
    role: HostRole,
    path: &str,
    body: &[u8],
) -> Result<(Authority, VerifiedRequest), Response> {
    // Resolve the current session before accepting any executor-supplied coordinates.
    let authority = authenticate(state, headers).await?;
    let verifier = match role {
        HostRole::Executor => state.execution_verifier.as_ref(),
        HostRole::Coordinator => state.coordination_verifier.as_ref(),
    }
    .ok_or_else(|| {
        problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "workspace_host_authority_unconfigured",
        )
    })?;
    let proof = headers
        .get(PROOF_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| problem(StatusCode::FORBIDDEN, "workspace_host_proof_required"))?;
    let receipt = verifier
        .verify(proof, &authority.session_authorization, "POST", path, body)
        .map_err(|_| problem(StatusCode::FORBIDDEN, "workspace_host_proof_refused"))?;
    Ok((authority, receipt))
}

pub(super) async fn open_attempt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    record_attempt(&state, &headers, &session_id, uri.path(), &body, false).await
}

pub(super) async fn close_attempt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    record_attempt(&state, &headers, &session_id, uri.path(), &body, true).await
}

async fn record_attempt(
    state: &AppState,
    headers: &HeaderMap,
    session_id: &str,
    path: &str,
    body: &[u8],
    close: bool,
) -> Response {
    let (authority, receipt) =
        match authenticate_host(state, headers, HostRole::Executor, path, body).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    let attempt: ExecutionAttempt = match serde_json::from_slice(body) {
        Ok(attempt) => attempt,
        Err(_) => return problem(StatusCode::UNPROCESSABLE_ENTITY, "coding_attempt_invalid"),
    };
    if !valid_attempt(&attempt, session_id) {
        return problem(StatusCode::UNPROCESSABLE_ENTITY, "coding_attempt_invalid");
    }
    // Closing remains possible after the materialization is gone. Its owner binding survives.
    if close {
        if state
            .store
            .coding_session(&authority, session_id)
            .await
            .is_err()
        {
            return problem(StatusCode::FORBIDDEN, "coding_session_binding_refused");
        }
    } else if let Err(response) =
        ready_session_materializations(state, &authority, session_id).await
    {
        return response;
    }
    match state
        .store
        .record_execution_attempt(&authority, &attempt, close, &receipt)
        .await
    {
        Ok(()) => Json(ExecutionAttemptState {
            attempt_id: attempt.attempt_id,
            active: !close,
        })
        .into_response(),
        Err(_) => problem(StatusCode::CONFLICT, "coding_attempt_or_replay_refused"),
    }
}

fn valid_attempt(attempt: &ExecutionAttempt, session_id: &str) -> bool {
    [
        &attempt.task_id,
        &attempt.attempt_id,
        &attempt.agent_id,
        &attempt.workspace_session_id,
        &attempt.agentide_session_id,
    ]
    .into_iter()
    .all(|value| valid_ref(value))
        && attempt
            .delegation_id
            .as_ref()
            .is_none_or(|value| valid_ref(value))
        && attempt.workspace_session_id == session_id
        && attempt
            .input
            .get("kind")
            .and_then(serde_json::Value::as_str)
            == Some("coding_session_turn")
        && attempt
            .input
            .get("workspace_session_id")
            .and_then(serde_json::Value::as_str)
            == Some(session_id)
        && attempt
            .input
            .get("agentide_session_id")
            .and_then(serde_json::Value::as_str)
            == Some(attempt.agentide_session_id.as_str())
}
