#![forbid(unsafe_code)]

//! Bounded official HTTP client for Workspace's user-facing contract.

use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde::{Serialize, de::DeserializeOwned};
use url::Url;
use workspace_core::{
    Branch, CodingActorViewRequest, CodingIntentInvocation, CodingIntentResult, CodingSession,
    CodingTreeProjection, CreateCodingSession, CreateMessage, CreateTerminal, CreateThread,
    DiffProjection, EngineeringArtifactPage, FileConflict, FileProjection, Message, OpenProject,
    Project, RepositoryCandidate, RepositoryEntry, ResolveDiff, SelectBranch, StartWorkflow,
    TerminalProfile, TerminalSession, Thread, WorkflowDefinition, WorkflowRun, WriteFile,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Workspace transport failure without response or credential bodies.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Configured service base is not admitted.
    #[error("Workspace service base is invalid")]
    Configuration,
    /// Service could not be reached or returned malformed bytes.
    #[error("Workspace service is unavailable")]
    Transport,
    /// Workspace refused the operation with the returned HTTP status.
    #[error("Workspace refused the operation with status {0}")]
    Refused(u16),
    /// An exact file write raced newer Workspace content.
    #[error("Workspace file content changed after it was loaded")]
    FileConflict(Box<FileConflict>),
}

/// Exact-origin Workspace HTTP client.
#[derive(Clone)]
pub struct WorkspaceClient {
    base: Url,
    http: reqwest::Client,
}

impl WorkspaceClient {
    /// Construct a client for an HTTPS or internal-cluster service origin.
    pub fn new(origin: &str) -> Result<Self, ClientError> {
        let base = Url::parse(origin).map_err(|_| ClientError::Configuration)?;
        let internal_http = base.scheme() == "http"
            && base.host_str().is_some_and(|host| {
                host == "127.0.0.1" || host == "localhost" || host.ends_with(".svc.cluster.local")
            });
        if !(base.scheme() == "https" || internal_http)
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(ClientError::Configuration);
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ClientError::Configuration)?;
        Ok(Self { base, http })
    }

    /// List repositories visible under current authority.
    pub async fn repositories(
        &self,
        authorization: &str,
    ) -> Result<Vec<RepositoryCandidate>, ClientError> {
        self.search_repositories(authorization, "").await
    }

    /// List a bounded set of repositories matching a provider-side search term.
    pub async fn search_repositories(
        &self,
        authorization: &str,
        query: &str,
    ) -> Result<Vec<RepositoryCandidate>, ClientError> {
        let suffix = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("query", query)
            .finish();
        self.exchange(
            Method::GET,
            &format!("v1/repositories?{suffix}"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// Open or join one canonical project after live access revalidation.
    pub async fn open_project(
        &self,
        authorization: &str,
        input: &OpenProject,
    ) -> Result<Project, ClientError> {
        self.exchange(Method::POST, "v1/projects", authorization, Some(input))
            .await
    }

    /// Read one accessible canonical project.
    pub async fn project(
        &self,
        authorization: &str,
        project_id: &str,
    ) -> Result<Project, ClientError> {
        self.exchange(
            Method::GET,
            &format!("v1/projects/{project_id}"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// List all currently visible branches for one project.
    pub async fn branches(
        &self,
        authorization: &str,
        project_id: &str,
    ) -> Result<Vec<Branch>, ClientError> {
        self.exchange(
            Method::GET,
            &format!("v1/projects/{project_id}/branches"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// List the root entries at the project's exact pinned commit.
    pub async fn repository_tree(
        &self,
        authorization: &str,
        project_id: &str,
    ) -> Result<Vec<RepositoryEntry>, ClientError> {
        self.exchange(
            Method::GET,
            &format!("v1/projects/{project_id}/tree"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// Explicitly select and refresh one branch head.
    pub async fn select_branch(
        &self,
        authorization: &str,
        project_id: &str,
        input: &SelectBranch,
    ) -> Result<Project, ClientError> {
        self.exchange(
            Method::POST,
            &format!("v1/projects/{project_id}/branch"),
            authorization,
            Some(input),
        )
        .await
    }

    /// List the authenticated subject's confined coding sessions for one project.
    pub async fn coding_sessions(
        &self,
        authorization: &str,
        project_id: &str,
    ) -> Result<Vec<CodingSession>, ClientError> {
        self.exchange(
            Method::GET,
            &format!("v1/projects/{project_id}/sessions"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// Materialize one exact project revision as immutable-base and writable-working references.
    pub async fn create_coding_session(
        &self,
        authorization: &str,
        project_id: &str,
        input: &CreateCodingSession,
    ) -> Result<CodingSession, ClientError> {
        self.exchange(
            Method::POST,
            &format!("v1/projects/{project_id}/sessions"),
            authorization,
            Some(input),
        )
        .await
    }

    /// Resume one owned coding-session representation after live project revalidation.
    pub async fn coding_session(
        &self,
        authorization: &str,
        session_id: &str,
    ) -> Result<CodingSession, ClientError> {
        self.exchange(
            Method::GET,
            &format!("v1/sessions/{session_id}"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// Refresh the exact `AgentIDE` actor context, attachments and tool inventory for one model turn.
    pub async fn coding_actor_view(
        &self,
        authorization: &str,
        session_id: &str,
        input: &CodingActorViewRequest,
    ) -> Result<agentide_contracts::ActorView, ClientError> {
        self.exchange(
            Method::POST,
            &format!("v1/sessions/{session_id}/actor-view"),
            authorization,
            Some(input),
        )
        .await
    }

    /// Invoke one currently authorized semantic coding intent through Workspace.
    pub async fn invoke_coding_intent(
        &self,
        authorization: &str,
        session_id: &str,
        input: &CodingIntentInvocation,
    ) -> Result<CodingIntentResult, ClientError> {
        self.exchange(
            Method::POST,
            &format!("v1/sessions/{session_id}/intents"),
            authorization,
            Some(input),
        )
        .await
    }

    /// Explicitly close one owned coding session and clean up both Substrate materializations.
    pub async fn close_coding_session(
        &self,
        authorization: &str,
        session_id: &str,
    ) -> Result<CodingSession, ClientError> {
        self.exchange(
            Method::DELETE,
            &format!("v1/sessions/{session_id}"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// Search and list a bounded working-materialization tree using the predecessor wire shape.
    pub async fn coding_tree(
        &self,
        authorization: &str,
        session_id: &str,
        query: &str,
        limit: u32,
    ) -> Result<CodingTreeProjection, ClientError> {
        let suffix = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("query", query)
            .append_pair("limit", &limit.to_string())
            .finish();
        self.exchange(
            Method::GET,
            &format!("v1/sessions/{session_id}/tree?{suffix}"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// Read one bounded page of a materialized directory without walking the repository.
    pub async fn coding_tree_page(
        &self,
        authorization: &str,
        session_id: &str,
        path: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<CodingTreeProjection, ClientError> {
        // `form_urlencoded::Serializer` contains a non-`Sync` callback slot. Keep it out of the
        // future state that crosses the network await so callers may use this client from Axum
        // handlers, whose futures must be `Send`.
        let suffix = {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query
                .append_pair("path", path)
                .append_pair("limit", &limit.to_string());
            if let Some(cursor) = cursor {
                query.append_pair("cursor", cursor);
            }
            query.finish()
        };
        self.exchange(
            Method::GET,
            &format!("v1/sessions/{session_id}/tree?{suffix}"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// Read one complete working file with its base-relative modification state.
    pub async fn coding_file(
        &self,
        authorization: &str,
        session_id: &str,
        path: &str,
    ) -> Result<FileProjection, ClientError> {
        self.exchange(
            Method::GET,
            &format!(
                "v1/sessions/{session_id}/files/{}",
                encode_workspace_path(path)
            ),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// Save or create one UTF-8 file using exact expected state.
    pub async fn write_coding_file(
        &self,
        authorization: &str,
        session_id: &str,
        path: &str,
        input: &WriteFile,
    ) -> Result<FileProjection, ClientError> {
        let endpoint = self
            .base
            .join(&format!(
                "v1/sessions/{session_id}/files/{}",
                encode_workspace_path(path)
            ))
            .map_err(|_| ClientError::Configuration)?;
        let response = self
            .http
            .put(endpoint)
            .header("authorization", authorization)
            .json(input)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|_| ClientError::Transport)?;
        if response.status() == StatusCode::CONFLICT {
            let conflict = response
                .json::<FileConflict>()
                .await
                .map_err(|_| ClientError::Transport)?;
            return Err(ClientError::FileConflict(Box::new(conflict)));
        }
        if !response.status().is_success() {
            return Err(ClientError::Refused(response.status().as_u16()));
        }
        response.json().await.map_err(|_| ClientError::Transport)
    }

    /// Resolve one canonical server-side diff projection.
    pub async fn resolve_diff(
        &self,
        authorization: &str,
        session_id: &str,
        input: &ResolveDiff,
    ) -> Result<DiffProjection, ClientError> {
        self.exchange(
            Method::POST,
            &format!("v1/sessions/{session_id}/diff"),
            authorization,
            Some(input),
        )
        .await
    }

    /// List deployment-declared terminal profiles for one ready coding session.
    pub async fn terminal_profiles(
        &self,
        authorization: &str,
        session_id: &str,
    ) -> Result<Vec<TerminalProfile>, ClientError> {
        self.exchange(
            Method::GET,
            &format!("v1/sessions/{session_id}/terminal-profiles"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// List durable terminal metadata without raw scrollback.
    pub async fn terminals(
        &self,
        authorization: &str,
        session_id: &str,
    ) -> Result<Vec<TerminalSession>, ClientError> {
        self.exchange(
            Method::GET,
            &format!("v1/sessions/{session_id}/terminals"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// Create a terminal through an exact deployment profile and current `AgentIDE` grant.
    pub async fn create_terminal(
        &self,
        authorization: &str,
        session_id: &str,
        input: &CreateTerminal,
    ) -> Result<TerminalSession, ClientError> {
        self.exchange(
            Method::POST,
            &format!("v1/sessions/{session_id}/terminals"),
            authorization,
            Some(input),
        )
        .await
    }

    /// Read one owned terminal's durable metadata.
    pub async fn terminal(
        &self,
        authorization: &str,
        terminal_id: &str,
    ) -> Result<TerminalSession, ClientError> {
        self.exchange(
            Method::GET,
            &format!("v1/terminals/{terminal_id}"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// Explicitly kill and retire one Substrate PTY session.
    pub async fn terminate_terminal(
        &self,
        authorization: &str,
        terminal_id: &str,
    ) -> Result<TerminalSession, ClientError> {
        self.exchange(
            Method::DELETE,
            &format!("v1/terminals/{terminal_id}"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// Build the credential-free WebSocket endpoint for a Devcenter same-origin proxy.
    ///
    /// The caller supplies its transient authorization only in the WebSocket handshake header;
    /// Workspace never places it in this URL.
    pub fn terminal_attachment_url(
        &self,
        terminal_id: &str,
        from_sequence: Option<u64>,
    ) -> Result<Url, ClientError> {
        if terminal_id.is_empty() || terminal_id.len() > 256 {
            return Err(ClientError::Configuration);
        }
        let encoded =
            url::form_urlencoded::byte_serialize(terminal_id.as_bytes()).collect::<String>();
        let mut endpoint = self
            .base
            .join(&format!("v1/terminals/{encoded}/attach"))
            .map_err(|_| ClientError::Configuration)?;
        let websocket_scheme = match endpoint.scheme() {
            "https" => "wss",
            "http" => "ws",
            _ => return Err(ClientError::Configuration),
        };
        endpoint
            .set_scheme(websocket_scheme)
            .map_err(|()| ClientError::Configuration)?;
        if let Some(sequence) = from_sequence {
            endpoint
                .query_pairs_mut()
                .append_pair("from_sequence", &sequence.to_string());
        }
        Ok(endpoint)
    }

    /// List the authenticated subject's personal threads.
    pub async fn threads(
        &self,
        authorization: &str,
        project_id: &str,
    ) -> Result<Vec<Thread>, ClientError> {
        self.exchange(
            Method::GET,
            &format!("v1/projects/{project_id}/threads"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// Create one personal branch-bound thread.
    pub async fn create_thread(
        &self,
        authorization: &str,
        project_id: &str,
        input: &CreateThread,
    ) -> Result<Thread, ClientError> {
        self.exchange(
            Method::POST,
            &format!("v1/projects/{project_id}/threads"),
            authorization,
            Some(input),
        )
        .await
    }

    /// List one personal thread's durable messages.
    pub async fn messages(
        &self,
        authorization: &str,
        thread_id: &str,
    ) -> Result<Vec<Message>, ClientError> {
        self.exchange(
            Method::GET,
            &format!("v1/threads/{thread_id}/messages"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// Append one user message to a personal thread.
    pub async fn create_message(
        &self,
        authorization: &str,
        thread_id: &str,
        input: &CreateMessage,
    ) -> Result<Message, ClientError> {
        self.exchange(
            Method::POST,
            &format!("v1/threads/{thread_id}/messages"),
            authorization,
            Some(input),
        )
        .await
    }

    /// Stream ordered Agent Platform events for one owned user turn without buffering them.
    pub async fn message_events(
        &self,
        authorization: &str,
        thread_id: &str,
        message_sequence: u64,
    ) -> Result<reqwest::Response, ClientError> {
        let endpoint = self
            .base
            .join(&format!(
                "v1/threads/{thread_id}/messages/{message_sequence}/events"
            ))
            .map_err(|_| ClientError::Configuration)?;
        let response = self
            .http
            .get(endpoint)
            .header("authorization", authorization)
            .send()
            .await
            .map_err(|_| ClientError::Transport)?;
        if !response.status().is_success() {
            return Err(ClientError::Refused(response.status().as_u16()));
        }
        Ok(response)
    }

    /// List the pre-built workflow definitions.
    pub async fn workflows(
        &self,
        authorization: &str,
        project_id: &str,
    ) -> Result<Vec<WorkflowDefinition>, ClientError> {
        self.exchange(
            Method::GET,
            &format!("v1/projects/{project_id}/workflows"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// Start one exact-commit workflow run.
    pub async fn start_workflow(
        &self,
        authorization: &str,
        project_id: &str,
        input: &StartWorkflow,
    ) -> Result<WorkflowRun, ClientError> {
        self.exchange(
            Method::POST,
            &format!("v1/projects/{project_id}/workflow-runs"),
            authorization,
            Some(input),
        )
        .await
    }

    /// List the current subject's recent runs for one project.
    pub async fn workflow_runs(
        &self,
        authorization: &str,
        project_id: &str,
    ) -> Result<Vec<WorkflowRun>, ClientError> {
        self.exchange(
            Method::GET,
            &format!("v1/projects/{project_id}/workflow-runs"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    /// List central AEP entities indexed to one accessible project.
    pub async fn engineering_artifacts(
        &self,
        authorization: &str,
        project_id: &str,
    ) -> Result<EngineeringArtifactPage, ClientError> {
        self.exchange(
            Method::GET,
            &format!("v1/projects/{project_id}/engineering-artifacts"),
            authorization,
            Option::<&()>::None,
        )
        .await
    }

    async fn exchange<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        authorization: &str,
        body: Option<&B>,
    ) -> Result<T, ClientError> {
        let endpoint = self
            .base
            .join(path)
            .map_err(|_| ClientError::Configuration)?;
        let mut request = self
            .http
            .request(method, endpoint)
            .header("authorization", authorization);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|_| ClientError::Transport)?;
        if !response.status().is_success() {
            return Err(ClientError::Refused(response.status().as_u16()));
        }
        if response.status() == StatusCode::NO_CONTENT {
            return Err(ClientError::Transport);
        }
        response.json().await.map_err(|_| ClientError::Transport)
    }
}

fn encode_workspace_path(path: &str) -> String {
    path.split('/')
        .map(|component| {
            url::form_urlencoded::byte_serialize(component.as_bytes()).collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_attachment_url_is_websocket_and_never_carries_authority() {
        let client = WorkspaceClient::new("https://workspace.example.test/base/").expect("client");
        let endpoint = client
            .terminal_attachment_url("terminal/one", Some(41))
            .expect("endpoint");
        assert_eq!(endpoint.scheme(), "wss");
        assert_eq!(
            endpoint.as_str(),
            "wss://workspace.example.test/base/v1/terminals/terminal%2Fone/attach?from_sequence=41"
        );
        assert!(!endpoint.as_str().contains("Bearer"));
    }
}
