//! Credential-free durable Workspace state.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::any::{AnyPoolOptions, AnyRow};
use sqlx::{AnyPool, Row as _};
use tokio::sync::OnceCell;
use workspace_core::{
    CreateMessage, CreateThread, Message, MessageRole, Project, StartWorkflow, Thread, WorkflowRun,
    WorkflowRunState,
};

use crate::Authority;

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS workspace_projects (project_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, forge_instance_ref TEXT NOT NULL, provider_project_ref TEXT NOT NULL, path_with_namespace TEXT NOT NULL, name TEXT NOT NULL, default_branch TEXT, selected_branch TEXT NOT NULL, pinned_commit TEXT, default_branch_fallback BIGINT NOT NULL, web_url TEXT NOT NULL, created_at_ms BIGINT NOT NULL, updated_at_ms BIGINT NOT NULL, UNIQUE (tenant_id, forge_instance_ref, provider_project_ref))",
    "CREATE TABLE IF NOT EXISTS workspace_project_associations (tenant_id TEXT NOT NULL, project_id TEXT NOT NULL, subject TEXT NOT NULL, last_validated_at_ms BIGINT NOT NULL, PRIMARY KEY (tenant_id, project_id, subject))",
    "CREATE TABLE IF NOT EXISTS workspace_threads (thread_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, project_id TEXT NOT NULL, owner_subject TEXT NOT NULL, branch TEXT NOT NULL, pinned_commit TEXT NOT NULL, title TEXT NOT NULL, created_at_ms BIGINT NOT NULL, updated_at_ms BIGINT NOT NULL)",
    "CREATE INDEX IF NOT EXISTS workspace_threads_owner ON workspace_threads (tenant_id, project_id, owner_subject, created_at_ms)",
    "CREATE TABLE IF NOT EXISTS workspace_messages (thread_id TEXT NOT NULL, sequence BIGINT NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL, branch TEXT NOT NULL, commit_ref TEXT NOT NULL, created_at_ms BIGINT NOT NULL, PRIMARY KEY (thread_id, sequence))",
    "CREATE TABLE IF NOT EXISTS workspace_project_agents (tenant_id TEXT NOT NULL, project_id TEXT NOT NULL, agent_id TEXT NOT NULL, created_at_ms BIGINT NOT NULL, PRIMARY KEY (tenant_id, project_id))",
    "CREATE TABLE IF NOT EXISTS workspace_workflow_runs (run_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, project_id TEXT NOT NULL, actor_subject TEXT NOT NULL, definition_id TEXT NOT NULL, branch TEXT NOT NULL, commit_ref TEXT NOT NULL, idempotency_key TEXT NOT NULL, state TEXT NOT NULL, failure_code TEXT, created_at_ms BIGINT NOT NULL, updated_at_ms BIGINT NOT NULL, UNIQUE (tenant_id, actor_subject, project_id, idempotency_key))",
];

/// Store failures with no SQL or tenant data exposed to transports.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Database URL is not an admitted local or hosted driver.
    #[error("Workspace store configuration is invalid")]
    Configuration,
    /// Durable storage is unavailable.
    #[error("Workspace store is unavailable")]
    Database(#[source] sqlx::Error),
    /// Requested tenant-scoped record does not exist.
    #[error("Workspace record was not found")]
    NotFound,
    /// Caller intent conflicts with durable state.
    #[error("Workspace state transition was refused")]
    Conflict,
    /// Stored state violates the closed domain vocabulary.
    #[error("Workspace store contains invalid data")]
    Corrupt,
}

/// Tenant-scoped durable Workspace state.
#[derive(Clone)]
pub struct Store {
    pool: AnyPool,
    initialized: Arc<OnceCell<()>>,
}

impl Store {
    /// Construct a lazy `SQLite` or `PostgreSQL` store.
    pub fn connect_lazy(database_url: &str) -> Result<Self, StoreError> {
        if !(database_url.starts_with("sqlite:") || database_url.starts_with("postgres")) {
            return Err(StoreError::Configuration);
        }
        sqlx::any::install_default_drivers();
        let maximum = if database_url.contains(":memory:") {
            1
        } else {
            8
        };
        let pool = AnyPoolOptions::new()
            .max_connections(maximum)
            .connect_lazy(database_url)
            .map_err(StoreError::Database)?;
        Ok(Self {
            pool,
            initialized: Arc::new(OnceCell::new()),
        })
    }

    /// Establish the schema and prove the database can be reached.
    pub async fn ready(&self) -> Result<(), StoreError> {
        self.ensure_schema().await
    }

    async fn ensure_schema(&self) -> Result<(), StoreError> {
        self.initialized
            .get_or_try_init(|| async {
                for statement in SCHEMA {
                    sqlx::query(statement)
                        .execute(&self.pool)
                        .await
                        .map_err(StoreError::Database)?;
                }
                Ok::<(), StoreError>(())
            })
            .await
            .copied()
    }

    /// Find an existing canonical project for one provider repository identity.
    pub async fn project_id_for(
        &self,
        tenant_id: &str,
        forge_instance_ref: &str,
        provider_project_ref: &str,
    ) -> Result<Option<String>, StoreError> {
        self.ensure_schema().await?;
        sqlx::query_scalar::<_, String>("SELECT project_id FROM workspace_projects WHERE tenant_id = ? AND forge_instance_ref = ? AND provider_project_ref = ?")
            .bind(tenant_id)
            .bind(forge_instance_ref)
            .bind(provider_project_ref)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::Database)
    }

    /// Idempotently create or refresh one canonical project and user association.
    pub async fn open_project(
        &self,
        authority: &Authority,
        project: &Project,
    ) -> Result<Project, StoreError> {
        self.ensure_schema().await?;
        let now = now_ms()?;
        let mut transaction = self.pool.begin().await.map_err(StoreError::Database)?;
        sqlx::query("INSERT INTO workspace_projects (project_id, tenant_id, forge_instance_ref, provider_project_ref, path_with_namespace, name, default_branch, selected_branch, pinned_commit, default_branch_fallback, web_url, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (tenant_id, forge_instance_ref, provider_project_ref) DO UPDATE SET path_with_namespace = excluded.path_with_namespace, name = excluded.name, default_branch = excluded.default_branch, web_url = excluded.web_url, updated_at_ms = excluded.updated_at_ms")
            .bind(&project.id)
            .bind(&authority.tenant_id)
            .bind(&project.forge_instance_ref)
            .bind(&project.project_ref)
            .bind(&project.path_with_namespace)
            .bind(&project.name)
            .bind(&project.default_branch)
            .bind(&project.selected_branch)
            .bind(&project.pinned_commit)
            .bind(i64::from(project.default_branch_fallback))
            .bind(&project.web_url)
            .bind(as_i64(now)?)
            .bind(as_i64(now)?)
            .execute(&mut *transaction)
            .await
            .map_err(StoreError::Database)?;
        sqlx::query("INSERT INTO workspace_project_associations (tenant_id, project_id, subject, last_validated_at_ms) VALUES (?, ?, ?, ?) ON CONFLICT (tenant_id, project_id, subject) DO UPDATE SET last_validated_at_ms = excluded.last_validated_at_ms")
            .bind(&authority.tenant_id)
            .bind(&project.id)
            .bind(&authority.subject)
            .bind(as_i64(now)?)
            .execute(&mut *transaction)
            .await
            .map_err(StoreError::Database)?;
        transaction.commit().await.map_err(StoreError::Database)?;
        self.project(authority, &project.id).await
    }

    /// Read a project only through its current subject association.
    pub async fn project(
        &self,
        authority: &Authority,
        project_id: &str,
    ) -> Result<Project, StoreError> {
        self.ensure_schema().await?;
        let row = sqlx::query("SELECT p.project_id, p.forge_instance_ref, p.provider_project_ref, p.path_with_namespace, p.name, p.default_branch, p.selected_branch, p.pinned_commit, p.default_branch_fallback, p.web_url FROM workspace_projects p INNER JOIN workspace_project_associations a ON a.tenant_id = p.tenant_id AND a.project_id = p.project_id WHERE p.tenant_id = ? AND p.project_id = ? AND a.subject = ?")
            .bind(&authority.tenant_id)
            .bind(project_id)
            .bind(&authority.subject)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::Database)?
            .ok_or(StoreError::NotFound)?;
        project_from_row(&row)
    }

    /// Record the latest successful live access revalidation.
    pub async fn record_access(
        &self,
        authority: &Authority,
        project_id: &str,
    ) -> Result<(), StoreError> {
        let now = now_ms()?;
        let result = sqlx::query("UPDATE workspace_project_associations SET last_validated_at_ms = ? WHERE tenant_id = ? AND project_id = ? AND subject = ?")
            .bind(as_i64(now)?)
            .bind(&authority.tenant_id)
            .bind(project_id)
            .bind(&authority.subject)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(StoreError::NotFound)
        }
    }

    /// Atomically advance the selected branch snapshot and matching personal thread boundaries.
    pub async fn select_branch(
        &self,
        authority: &Authority,
        project: &Project,
        branch: &str,
        commit: &str,
    ) -> Result<Project, StoreError> {
        self.ensure_schema().await?;
        let now = now_ms()?;
        let mut transaction = self.pool.begin().await.map_err(StoreError::Database)?;
        let result = sqlx::query("UPDATE workspace_projects SET selected_branch = ?, pinned_commit = ?, updated_at_ms = ? WHERE tenant_id = ? AND project_id = ?")
            .bind(branch)
            .bind(commit)
            .bind(as_i64(now)?)
            .bind(&authority.tenant_id)
            .bind(&project.id)
            .execute(&mut *transaction)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() != 1 {
            return Err(StoreError::NotFound);
        }
        if project.selected_branch == branch
            && project
                .pinned_commit
                .as_deref()
                .is_some_and(|previous| previous != commit)
        {
            let thread_rows = sqlx::query("SELECT thread_id FROM workspace_threads WHERE tenant_id = ? AND project_id = ? AND owner_subject = ? AND branch = ?")
                .bind(&authority.tenant_id)
                .bind(&project.id)
                .bind(&authority.subject)
                .bind(branch)
                .fetch_all(&mut *transaction)
                .await
                .map_err(StoreError::Database)?;
            for row in thread_rows {
                let thread_id: String =
                    row.try_get("thread_id").map_err(|_| StoreError::Corrupt)?;
                let sequence: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) + 1 FROM workspace_messages WHERE thread_id = ?")
                    .bind(&thread_id)
                    .fetch_one(&mut *transaction)
                    .await
                    .map_err(StoreError::Database)?;
                sqlx::query("INSERT INTO workspace_messages (thread_id, sequence, role, content, branch, commit_ref, created_at_ms) VALUES (?, ?, 'system', ?, ?, ?, ?)")
                    .bind(&thread_id)
                    .bind(sequence)
                    .bind(format!("Repository snapshot refreshed to {commit}."))
                    .bind(branch)
                    .bind(commit)
                    .bind(as_i64(now)?)
                    .execute(&mut *transaction)
                    .await
                    .map_err(StoreError::Database)?;
            }
            sqlx::query("UPDATE workspace_threads SET pinned_commit = ?, updated_at_ms = ? WHERE tenant_id = ? AND project_id = ? AND owner_subject = ? AND branch = ?")
                .bind(commit)
                .bind(as_i64(now)?)
                .bind(&authority.tenant_id)
                .bind(&project.id)
                .bind(&authority.subject)
                .bind(branch)
                .execute(&mut *transaction)
                .await
                .map_err(StoreError::Database)?;
        }
        transaction.commit().await.map_err(StoreError::Database)?;
        self.project(authority, &project.id).await
    }

    /// List only the current subject's personal project threads.
    pub async fn threads(
        &self,
        authority: &Authority,
        project_id: &str,
    ) -> Result<Vec<Thread>, StoreError> {
        self.ensure_schema().await?;
        let rows = sqlx::query("SELECT thread_id, project_id, branch, pinned_commit, title, created_at_ms FROM workspace_threads WHERE tenant_id = ? AND project_id = ? AND owner_subject = ? ORDER BY created_at_ms DESC")
            .bind(&authority.tenant_id)
            .bind(project_id)
            .bind(&authority.subject)
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        rows.iter().map(thread_from_row).collect()
    }

    /// Create a personal branch-bound thread.
    pub async fn create_thread(
        &self,
        authority: &Authority,
        project_id: &str,
        input: &CreateThread,
    ) -> Result<Thread, StoreError> {
        self.ensure_schema().await?;
        let now = now_ms()?;
        let thread = Thread {
            id: random_id("thread")?,
            project_id: project_id.to_owned(),
            branch: input.branch.clone(),
            pinned_commit: input.pinned_commit.clone(),
            title: input.title.trim().to_owned(),
            created_at_ms: now,
        };
        sqlx::query("INSERT INTO workspace_threads (thread_id, tenant_id, project_id, owner_subject, branch, pinned_commit, title, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&thread.id)
            .bind(&authority.tenant_id)
            .bind(project_id)
            .bind(&authority.subject)
            .bind(&thread.branch)
            .bind(&thread.pinned_commit)
            .bind(&thread.title)
            .bind(as_i64(now)?)
            .bind(as_i64(now)?)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        Ok(thread)
    }

    /// List durable messages after enforcing personal thread ownership in SQL.
    pub async fn messages(
        &self,
        authority: &Authority,
        thread_id: &str,
    ) -> Result<Vec<Message>, StoreError> {
        self.ensure_schema().await?;
        self.owned_thread(authority, thread_id).await?;
        let rows = sqlx::query("SELECT sequence, role, content, branch, commit_ref, created_at_ms FROM workspace_messages WHERE thread_id = ? ORDER BY sequence")
            .bind(thread_id)
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        rows.iter().map(message_from_row).collect()
    }

    /// Append one user message at the thread's current commit boundary.
    pub async fn create_message(
        &self,
        authority: &Authority,
        thread_id: &str,
        input: &CreateMessage,
    ) -> Result<Message, StoreError> {
        self.ensure_schema().await?;
        let thread = self.owned_thread(authority, thread_id).await?;
        self.append_message(
            &authority.tenant_id,
            &authority.subject,
            &thread,
            MessageRole::User,
            input.content.trim(),
        )
        .await
    }

    /// Append agent output after a task completes without retaining request credentials.
    pub async fn append_agent_message(
        &self,
        tenant_id: &str,
        owner_subject: &str,
        thread_id: &str,
        role: MessageRole,
        content: &str,
    ) -> Result<Message, StoreError> {
        self.ensure_schema().await?;
        if !matches!(role, MessageRole::Assistant | MessageRole::System) {
            return Err(StoreError::Conflict);
        }
        let row = sqlx::query("SELECT thread_id, project_id, branch, pinned_commit, title, created_at_ms FROM workspace_threads WHERE tenant_id = ? AND thread_id = ? AND owner_subject = ?")
            .bind(tenant_id)
            .bind(thread_id)
            .bind(owner_subject)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::Database)?
            .ok_or(StoreError::NotFound)?;
        let thread = thread_from_row(&row)?;
        self.append_message(tenant_id, owner_subject, &thread, role, content)
            .await
    }

    async fn append_message(
        &self,
        tenant_id: &str,
        owner_subject: &str,
        thread: &Thread,
        role: MessageRole,
        content: &str,
    ) -> Result<Message, StoreError> {
        let now = now_ms()?;
        let mut transaction = self.pool.begin().await.map_err(StoreError::Database)?;
        let owned: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspace_threads WHERE tenant_id = ? AND thread_id = ? AND owner_subject = ?")
            .bind(tenant_id)
            .bind(&thread.id)
            .bind(owner_subject)
            .fetch_one(&mut *transaction)
            .await
            .map_err(StoreError::Database)?;
        if owned != 1 {
            return Err(StoreError::NotFound);
        }
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM workspace_messages WHERE thread_id = ?",
        )
        .bind(&thread.id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(StoreError::Database)?;
        let role_name = match role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
        };
        sqlx::query("INSERT INTO workspace_messages (thread_id, sequence, role, content, branch, commit_ref, created_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(&thread.id)
            .bind(sequence)
            .bind(role_name)
            .bind(content)
            .bind(&thread.branch)
            .bind(&thread.pinned_commit)
            .bind(as_i64(now)?)
            .execute(&mut *transaction)
            .await
            .map_err(StoreError::Database)?;
        transaction.commit().await.map_err(StoreError::Database)?;
        Ok(Message {
            sequence: u64::try_from(sequence).map_err(|_| StoreError::Corrupt)?,
            role,
            content: content.to_owned(),
            branch: thread.branch.clone(),
            commit: thread.pinned_commit.clone(),
            created_at_ms: now,
        })
    }

    /// Read the current subject's thread for project-context dispatch.
    pub async fn thread(
        &self,
        authority: &Authority,
        thread_id: &str,
    ) -> Result<Thread, StoreError> {
        self.ensure_schema().await?;
        self.owned_thread(authority, thread_id).await
    }

    /// Read the shared project agent identity when it has been provisioned.
    pub async fn project_agent(
        &self,
        tenant_id: &str,
        project_id: &str,
    ) -> Result<Option<String>, StoreError> {
        self.ensure_schema().await?;
        sqlx::query_scalar(
            "SELECT agent_id FROM workspace_project_agents WHERE tenant_id = ? AND project_id = ?",
        )
        .bind(tenant_id)
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::Database)
    }

    /// Converge the project-to-agent link after Agent Platform provisioning.
    pub async fn record_project_agent(
        &self,
        tenant_id: &str,
        project_id: &str,
        agent_id: &str,
    ) -> Result<String, StoreError> {
        self.ensure_schema().await?;
        let now = now_ms()?;
        sqlx::query("INSERT INTO workspace_project_agents (tenant_id, project_id, agent_id, created_at_ms) VALUES (?, ?, ?, ?) ON CONFLICT (tenant_id, project_id) DO NOTHING")
            .bind(tenant_id)
            .bind(project_id)
            .bind(agent_id)
            .bind(as_i64(now)?)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        self.project_agent(tenant_id, project_id)
            .await?
            .ok_or(StoreError::Corrupt)
    }

    /// Idempotently admit one exact-snapshot pre-built workflow run.
    pub async fn start_workflow(
        &self,
        authority: &Authority,
        project_id: &str,
        input: &StartWorkflow,
    ) -> Result<WorkflowRun, StoreError> {
        self.ensure_schema().await?;
        if let Some(row) = sqlx::query("SELECT run_id, definition_id, project_id, branch, commit_ref, state, failure_code, created_at_ms FROM workspace_workflow_runs WHERE tenant_id = ? AND actor_subject = ? AND project_id = ? AND idempotency_key = ?")
            .bind(&authority.tenant_id)
            .bind(&authority.subject)
            .bind(project_id)
            .bind(&input.idempotency_key)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::Database)?
        {
            let run = workflow_from_row(&row)?;
            if run.definition_id == input.definition_id && run.branch == input.branch && run.commit == input.commit {
                return Ok(run);
            }
            return Err(StoreError::Conflict);
        }
        let now = now_ms()?;
        let run = WorkflowRun {
            id: random_id("run")?,
            definition_id: input.definition_id.clone(),
            project_id: project_id.to_owned(),
            branch: input.branch.clone(),
            commit: input.commit.clone(),
            state: WorkflowRunState::Accepted,
            failure_code: None,
            created_at_ms: now,
        };
        sqlx::query("INSERT INTO workspace_workflow_runs (run_id, tenant_id, project_id, actor_subject, definition_id, branch, commit_ref, idempotency_key, state, failure_code, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'accepted', NULL, ?, ?)")
            .bind(&run.id)
            .bind(&authority.tenant_id)
            .bind(project_id)
            .bind(&authority.subject)
            .bind(&run.definition_id)
            .bind(&run.branch)
            .bind(&run.commit)
            .bind(&input.idempotency_key)
            .bind(as_i64(now)?)
            .bind(as_i64(now)?)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        Ok(run)
    }

    async fn owned_thread(
        &self,
        authority: &Authority,
        thread_id: &str,
    ) -> Result<Thread, StoreError> {
        let row = sqlx::query("SELECT thread_id, project_id, branch, pinned_commit, title, created_at_ms FROM workspace_threads WHERE tenant_id = ? AND thread_id = ? AND owner_subject = ?")
            .bind(&authority.tenant_id)
            .bind(thread_id)
            .bind(&authority.subject)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::Database)?
            .ok_or(StoreError::NotFound)?;
        thread_from_row(&row)
    }
}

fn project_from_row(row: &AnyRow) -> Result<Project, StoreError> {
    Ok(Project {
        id: row.try_get("project_id").map_err(|_| StoreError::Corrupt)?,
        forge_instance_ref: row
            .try_get("forge_instance_ref")
            .map_err(|_| StoreError::Corrupt)?,
        project_ref: row
            .try_get("provider_project_ref")
            .map_err(|_| StoreError::Corrupt)?,
        path_with_namespace: row
            .try_get("path_with_namespace")
            .map_err(|_| StoreError::Corrupt)?,
        name: row.try_get("name").map_err(|_| StoreError::Corrupt)?,
        default_branch: row
            .try_get("default_branch")
            .map_err(|_| StoreError::Corrupt)?,
        selected_branch: row
            .try_get("selected_branch")
            .map_err(|_| StoreError::Corrupt)?,
        pinned_commit: row
            .try_get("pinned_commit")
            .map_err(|_| StoreError::Corrupt)?,
        default_branch_fallback: row
            .try_get::<i64, _>("default_branch_fallback")
            .map_err(|_| StoreError::Corrupt)?
            != 0,
        web_url: row.try_get("web_url").map_err(|_| StoreError::Corrupt)?,
    })
}

fn thread_from_row(row: &AnyRow) -> Result<Thread, StoreError> {
    Ok(Thread {
        id: row.try_get("thread_id").map_err(|_| StoreError::Corrupt)?,
        project_id: row.try_get("project_id").map_err(|_| StoreError::Corrupt)?,
        branch: row.try_get("branch").map_err(|_| StoreError::Corrupt)?,
        pinned_commit: row
            .try_get("pinned_commit")
            .map_err(|_| StoreError::Corrupt)?,
        title: row.try_get("title").map_err(|_| StoreError::Corrupt)?,
        created_at_ms: from_i64(
            row.try_get("created_at_ms")
                .map_err(|_| StoreError::Corrupt)?,
        )?,
    })
}

fn message_from_row(row: &AnyRow) -> Result<Message, StoreError> {
    let role: String = row.try_get("role").map_err(|_| StoreError::Corrupt)?;
    Ok(Message {
        sequence: from_i64(row.try_get("sequence").map_err(|_| StoreError::Corrupt)?)?,
        role: match role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => MessageRole::System,
            _ => return Err(StoreError::Corrupt),
        },
        content: row.try_get("content").map_err(|_| StoreError::Corrupt)?,
        branch: row.try_get("branch").map_err(|_| StoreError::Corrupt)?,
        commit: row.try_get("commit_ref").map_err(|_| StoreError::Corrupt)?,
        created_at_ms: from_i64(
            row.try_get("created_at_ms")
                .map_err(|_| StoreError::Corrupt)?,
        )?,
    })
}

fn workflow_from_row(row: &AnyRow) -> Result<WorkflowRun, StoreError> {
    let state: String = row.try_get("state").map_err(|_| StoreError::Corrupt)?;
    Ok(WorkflowRun {
        id: row.try_get("run_id").map_err(|_| StoreError::Corrupt)?,
        definition_id: row
            .try_get("definition_id")
            .map_err(|_| StoreError::Corrupt)?,
        project_id: row.try_get("project_id").map_err(|_| StoreError::Corrupt)?,
        branch: row.try_get("branch").map_err(|_| StoreError::Corrupt)?,
        commit: row.try_get("commit_ref").map_err(|_| StoreError::Corrupt)?,
        state: match state.as_str() {
            "accepted" => WorkflowRunState::Accepted,
            "running" => WorkflowRunState::Running,
            "succeeded" => WorkflowRunState::Succeeded,
            "failed" => WorkflowRunState::Failed,
            "refused" => WorkflowRunState::Refused,
            _ => return Err(StoreError::Corrupt),
        },
        failure_code: row
            .try_get("failure_code")
            .map_err(|_| StoreError::Corrupt)?,
        created_at_ms: from_i64(
            row.try_get("created_at_ms")
                .map_err(|_| StoreError::Corrupt)?,
        )?,
    })
}

fn random_id(prefix: &str) -> Result<String, StoreError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| StoreError::Configuration)?;
    Ok(format!("{prefix}-{}", hex::encode(bytes)))
}

fn now_ms() -> Result<u64, StoreError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StoreError::Configuration)
        .and_then(|duration| {
            u64::try_from(duration.as_millis()).map_err(|_| StoreError::Configuration)
        })
}

fn as_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Corrupt)
}

fn from_i64(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Corrupt)
}

#[cfg(test)]
mod tests {
    use connectors_client::operation::OwnerContext;

    use super::*;

    fn authority(subject: &str) -> Authority {
        Authority {
            tenant_id: "tenant-one".to_owned(),
            subject: subject.to_owned(),
            connector_bearer: "not-retained".to_owned(),
            agent_platform_bearer: None,
            context: OwnerContext {
                tenant_id: "tenant-one".to_owned(),
                agent_id: "workspace:test".to_owned(),
                agent_revision: 1,
                authority_snapshot_id: "identity:test".to_owned(),
                authority_snapshot_sha256: "a".repeat(64),
            },
        }
    }

    fn project() -> Project {
        Project {
            id: "project-one".to_owned(),
            forge_instance_ref: "connection:gitlab:one".to_owned(),
            project_ref: "42".to_owned(),
            path_with_namespace: "group/project".to_owned(),
            name: "project".to_owned(),
            default_branch: Some("main".to_owned()),
            selected_branch: "main".to_owned(),
            pinned_commit: None,
            default_branch_fallback: false,
            web_url: "https://gitlab.example.test/group/project".to_owned(),
        }
    }

    async fn store() -> Store {
        let store = Store::connect_lazy("sqlite::memory:").expect("store");
        store.ready().await.expect("schema");
        store
    }

    #[tokio::test]
    async fn canonical_project_converges_but_requires_each_subject_association() {
        let store = store().await;
        let first = authority("person:first");
        let second = authority("person:second");
        let opened = store.open_project(&first, &project()).await.unwrap();
        assert_eq!(opened.id, "project-one");
        assert!(matches!(
            store.project(&second, "project-one").await,
            Err(StoreError::NotFound)
        ));

        let second_open = store.open_project(&second, &project()).await.unwrap();
        assert_eq!(second_open.id, opened.id);
        assert_eq!(
            store
                .project_id_for("tenant-one", "connection:gitlab:one", "42")
                .await
                .unwrap()
                .as_deref(),
            Some("project-one")
        );
    }

    #[tokio::test]
    async fn refresh_records_a_commit_boundary_without_exposing_personal_threads() {
        let store = store().await;
        let owner = authority("person:owner");
        let other = authority("person:other");
        let opened = store.open_project(&owner, &project()).await.unwrap();
        let pinned = store
            .select_branch(&owner, &opened, "main", &"a".repeat(40))
            .await
            .unwrap();
        let thread = store
            .create_thread(
                &owner,
                &pinned.id,
                &CreateThread {
                    branch: "main".to_owned(),
                    pinned_commit: "a".repeat(40),
                    title: "Understand the service".to_owned(),
                },
            )
            .await
            .unwrap();
        let refreshed = store
            .select_branch(&owner, &pinned, "main", &"b".repeat(40))
            .await
            .unwrap();
        assert_eq!(
            refreshed.pinned_commit.as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        let messages = store.messages(&owner, &thread.id).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, MessageRole::System);
        assert_eq!(messages[0].commit, "b".repeat(40));
        assert!(matches!(
            store.messages(&other, &thread.id).await,
            Err(StoreError::NotFound)
        ));
    }

    #[tokio::test]
    async fn workflow_idempotency_covers_the_exact_snapshot_intent() {
        let store = store().await;
        let actor = authority("person:actor");
        store.open_project(&actor, &project()).await.unwrap();
        let input = StartWorkflow {
            definition_id: "review.code/v1".to_owned(),
            branch: "main".to_owned(),
            commit: "c".repeat(40),
            idempotency_key: "request-one".to_owned(),
        };
        let first = store
            .start_workflow(&actor, "project-one", &input)
            .await
            .unwrap();
        let replay = store
            .start_workflow(&actor, "project-one", &input)
            .await
            .unwrap();
        assert_eq!(first.id, replay.id);

        let changed = StartWorkflow {
            commit: "d".repeat(40),
            ..input
        };
        assert!(matches!(
            store.start_workflow(&actor, "project-one", &changed).await,
            Err(StoreError::Conflict)
        ));
    }
}
