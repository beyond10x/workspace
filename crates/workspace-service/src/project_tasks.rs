//! Product coordinator bookkeeping over Workspace-owned rows.

use axum::extract::OriginalUri;
use bytes::Bytes;
use workspace_client::attestation::HostRole;
use workspace_core::{ProjectTaskReply as Reply, ProjectTaskRequest as Request};

use super::{
    AppState, Authority, HeaderMap, Json, Response, State, StatusCode, accessible_project,
    confidential, execution, problem, project_context, store_problem, valid_ref,
};
use axum::response::IntoResponse;

#[allow(clippy::too_many_lines)] // One exhaustive dispatch inventories the closed bookkeeping protocol.
pub(super) async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    let (authority, receipt) = match execution::authenticate_host(
        &state,
        &headers,
        HostRole::Coordinator,
        uri.path(),
        &body,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let request: Request = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return problem(
                StatusCode::UNPROCESSABLE_ENTITY,
                "project_task_request_invalid",
            );
        }
    };
    let project_id = match project_for(&state, &authority, &request).await {
        Ok(project_id) => project_id,
        Err(response) => return response,
    };
    let project = match accessible_project(&state, &authority, &project_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    if let Err(error) = state.store.consume_host_request(&receipt).await {
        return store_problem(&error);
    }
    let store = &state.store;
    let reply = async {
        Ok::<Reply, Response>(match request {
            Request::Context { .. } => Reply::Context {
                context: project_context(&state, &authority, &project).await?,
                project: Box::new(project),
            },
            Request::Thread { thread_id } => Reply::Thread {
                thread: store
                    .thread(&authority, &thread_id)
                    .await
                    .map_err(|error| store_problem(&error))?,
            },
            Request::ProjectAgent { .. } => Reply::Agent {
                agent_id: store
                    .project_agent(&authority.tenant_id, &project_id)
                    .await
                    .map_err(|error| store_problem(&error))?,
            },
            Request::RecordProjectAgent { agent_id, .. } => {
                require_reference(&agent_id)?;
                Reply::Agent {
                    agent_id: Some(
                        store
                            .record_project_agent(&authority.tenant_id, &project_id, &agent_id)
                            .await
                            .map_err(|error| store_problem(&error))?,
                    ),
                }
            }
            Request::MessageTask {
                thread_id,
                sequence,
            } => Reply::MessageTask {
                task_id: store
                    .message_task(&authority, &thread_id, sequence)
                    .await
                    .map_err(|error| store_problem(&error))?,
                needs_completion: store
                    .message_needs_completion(&authority, &thread_id, sequence)
                    .await
                    .map_err(|error| store_problem(&error))?,
            },
            Request::RecordMessageTask {
                thread_id,
                sequence,
                task_id,
            } => {
                require_reference(&task_id)?;
                store
                    .record_message_task(&authority, &thread_id, sequence, &task_id)
                    .await
                    .map_err(|error| store_problem(&error))?;
                Reply::Task {
                    task_id: Some(task_id),
                }
            }
            Request::CompleteMessageTask {
                thread_id,
                sequence,
                task_id,
                role,
                content,
            } => {
                require_content(&content)?;
                Reply::Message {
                    message: store
                        .complete_message_task(
                            &authority, &thread_id, sequence, &task_id, role, &content,
                        )
                        .await
                        .map_err(|error| store_problem(&error))?,
                }
            }
            Request::AppendMessage {
                thread_id,
                role,
                content,
            } => {
                require_content(&content)?;
                Reply::Message {
                    message: store
                        .append_agent_message(
                            &authority.tenant_id,
                            &authority.subject,
                            &thread_id,
                            role,
                            &content,
                        )
                        .await
                        .map_err(|error| store_problem(&error))?,
                }
            }
            Request::WorkflowTask { run_id } => Reply::Task {
                task_id: store
                    .workflow_task(&authority, &run_id)
                    .await
                    .map_err(|error| store_problem(&error))?,
            },
            Request::RecordWorkflowTask { run_id, task_id } => {
                require_reference(&task_id)?;
                store
                    .record_workflow_task(&authority, &run_id, &task_id)
                    .await
                    .map_err(|error| store_problem(&error))?;
                Reply::Task {
                    task_id: Some(task_id),
                }
            }
            Request::RecoverableWorkflowTasks { .. } => Reply::WorkflowTasks {
                tasks: store
                    .recoverable_workflow_tasks(&authority, &project_id)
                    .await
                    .map_err(|error| store_problem(&error))?,
            },
            Request::UpdateWorkflow {
                run_id,
                state,
                failure_code,
                output,
            } => {
                if let Some(output) = &output {
                    require_content(output)?;
                }
                Reply::Updated {
                    changed: store
                        .update_workflow_run(
                            &authority.tenant_id,
                            &authority.subject,
                            &run_id,
                            state,
                            failure_code.as_deref(),
                            output.as_deref(),
                        )
                        .await
                        .map_err(|error| store_problem(&error))?,
                }
            }
        })
    }
    .await;
    match reply {
        Ok(reply) => confidential(Json(reply).into_response()),
        Err(response) => response,
    }
}

async fn project_for(
    state: &AppState,
    authority: &Authority,
    request: &Request,
) -> Result<String, Response> {
    let result = match request {
        Request::Context { project_id }
        | Request::ProjectAgent { project_id }
        | Request::RecordProjectAgent { project_id, .. }
        | Request::RecoverableWorkflowTasks { project_id } => Ok(project_id.clone()),
        Request::Thread { thread_id }
        | Request::MessageTask { thread_id, .. }
        | Request::RecordMessageTask { thread_id, .. }
        | Request::CompleteMessageTask { thread_id, .. }
        | Request::AppendMessage { thread_id, .. } => state
            .store
            .thread(authority, thread_id)
            .await
            .map(|thread| thread.project_id),
        Request::WorkflowTask { run_id }
        | Request::RecordWorkflowTask { run_id, .. }
        | Request::UpdateWorkflow { run_id, .. } => {
            state.store.workflow_project(authority, run_id).await
        }
    };
    result.map_err(|error| store_problem(&error))
}

fn require_reference(value: &str) -> Result<(), Response> {
    if valid_ref(value) {
        Ok(())
    } else {
        Err(problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "project_task_reference_invalid",
        ))
    }
}

fn require_content(value: &str) -> Result<(), Response> {
    if value.len() <= 1024 * 1024 {
        Ok(())
    } else {
        Err(problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "project_task_output_too_large",
        ))
    }
}
