#![forbid(unsafe_code)]

//! Bounded official HTTP client for Workspace's user-facing contract.

use reqwest::{Method, StatusCode};
use serde::{Serialize, de::DeserializeOwned};
use url::Url;
use workspace_core::{
    Branch, CreateMessage, CreateThread, Message, OpenProject, Project, RepositoryCandidate,
    SelectBranch, StartWorkflow, Thread, WorkflowDefinition, WorkflowRun,
};

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
        self.exchange(
            Method::GET,
            "v1/repositories",
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
        let response = request.send().await.map_err(|_| ClientError::Transport)?;
        if !response.status().is_success() {
            return Err(ClientError::Refused(response.status().as_u16()));
        }
        if response.status() == StatusCode::NO_CONTENT {
            return Err(ClientError::Transport);
        }
        response.json().await.map_err(|_| ClientError::Transport)
    }
}
