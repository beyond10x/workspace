//! Credential-free durable Workspace state.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::any::{AnyPoolOptions, AnyRow};
use sqlx::{AnyPool, Row as _};
use tokio::sync::OnceCell;
use workspace_core::{
    CodingSession, CodingSessionState, CreateCodingSession, CreateMessage, CreateTerminal,
    CreateThread, MaterializationLimits, Message, MessageRole, Project, StartWorkflow,
    TerminalExit, TerminalProfile, TerminalSession, TerminalState, Thread, WorkflowRun,
    WorkflowRunState,
};

use crate::Authority;

const MAX_ACTIVE_TERMINALS_PER_SESSION: i64 = 8;

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS workspace_projects (project_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, forge_instance_ref TEXT NOT NULL, provider_project_ref TEXT NOT NULL, path_with_namespace TEXT NOT NULL, name TEXT NOT NULL, default_branch TEXT, web_url TEXT NOT NULL, created_at_ms BIGINT NOT NULL, updated_at_ms BIGINT NOT NULL, UNIQUE (tenant_id, forge_instance_ref, provider_project_ref))",
    "CREATE TABLE IF NOT EXISTS workspace_project_associations (tenant_id TEXT NOT NULL, project_id TEXT NOT NULL, subject TEXT NOT NULL, selected_branch TEXT NOT NULL, pinned_commit TEXT, last_validated_at_ms BIGINT NOT NULL, PRIMARY KEY (tenant_id, project_id, subject))",
    "CREATE TABLE IF NOT EXISTS workspace_threads (thread_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, project_id TEXT NOT NULL, owner_subject TEXT NOT NULL, branch TEXT NOT NULL, pinned_commit TEXT NOT NULL, title TEXT NOT NULL, created_at_ms BIGINT NOT NULL, updated_at_ms BIGINT NOT NULL)",
    "CREATE INDEX IF NOT EXISTS workspace_threads_owner ON workspace_threads (tenant_id, project_id, owner_subject, created_at_ms)",
    "CREATE TABLE IF NOT EXISTS workspace_messages (thread_id TEXT NOT NULL, sequence BIGINT NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL, branch TEXT NOT NULL, commit_ref TEXT NOT NULL, created_at_ms BIGINT NOT NULL, PRIMARY KEY (thread_id, sequence))",
    "CREATE TABLE IF NOT EXISTS workspace_project_agents (tenant_id TEXT NOT NULL, project_id TEXT NOT NULL, agent_id TEXT NOT NULL, created_at_ms BIGINT NOT NULL, PRIMARY KEY (tenant_id, project_id))",
    "CREATE TABLE IF NOT EXISTS workspace_coding_sessions (session_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, project_id TEXT NOT NULL, owner_subject TEXT NOT NULL, source_revision TEXT NOT NULL, idempotency_key TEXT NOT NULL, base_materialization_ref TEXT, working_materialization_ref TEXT, manifest_sha256 TEXT, state TEXT NOT NULL, failure_code TEXT, max_files BIGINT NOT NULL, max_total_bytes BIGINT NOT NULL, max_file_bytes BIGINT NOT NULL, created_at_ms BIGINT NOT NULL, updated_at_ms BIGINT NOT NULL, UNIQUE (tenant_id, owner_subject, project_id, idempotency_key))",
    "CREATE INDEX IF NOT EXISTS workspace_coding_sessions_owner ON workspace_coding_sessions (tenant_id, project_id, owner_subject, created_at_ms)",
    "CREATE TABLE IF NOT EXISTS workspace_terminals (terminal_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, coding_session_id TEXT NOT NULL, owner_subject TEXT NOT NULL, agentide_session_id TEXT NOT NULL, authority_grant_id TEXT NOT NULL, profile_json TEXT NOT NULL, initial_columns BIGINT NOT NULL, initial_rows BIGINT NOT NULL, idempotency_key TEXT NOT NULL, substrate_session_ref TEXT, process_id TEXT, state TEXT NOT NULL, exit_code BIGINT, exit_signal TEXT, failure_code TEXT, created_at_ms BIGINT NOT NULL, updated_at_ms BIGINT NOT NULL, UNIQUE (tenant_id, owner_subject, coding_session_id, idempotency_key))",
    "CREATE INDEX IF NOT EXISTS workspace_terminals_owner ON workspace_terminals (tenant_id, coding_session_id, owner_subject, created_at_ms)",
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

/// Whether an idempotent coding-session reservation was newly admitted.
pub enum SessionReservation {
    New(CodingSession),
    Existing(CodingSession),
}

/// Whether an idempotent terminal reservation was newly admitted.
pub enum TerminalReservation {
    New(StoredTerminal),
    Existing(StoredTerminal),
}

/// Durable terminal row including the opaque Substrate reference used only by Workspace.
#[derive(Clone)]
pub struct StoredTerminal {
    pub public: TerminalSession,
    pub substrate_session_ref: Option<String>,
    pub initial_columns: u16,
    pub initial_rows: u16,
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
        sqlx::query("INSERT INTO workspace_projects (project_id, tenant_id, forge_instance_ref, provider_project_ref, path_with_namespace, name, default_branch, web_url, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (tenant_id, forge_instance_ref, provider_project_ref) DO UPDATE SET path_with_namespace = excluded.path_with_namespace, name = excluded.name, default_branch = excluded.default_branch, web_url = excluded.web_url, updated_at_ms = excluded.updated_at_ms")
            .bind(&project.id)
            .bind(&authority.tenant_id)
            .bind(&project.forge_instance_ref)
            .bind(&project.project_ref)
            .bind(&project.path_with_namespace)
            .bind(&project.name)
            .bind(&project.default_branch)
            .bind(&project.web_url)
            .bind(as_i64(now)?)
            .bind(as_i64(now)?)
            .execute(&mut *transaction)
            .await
            .map_err(StoreError::Database)?;
        sqlx::query("INSERT INTO workspace_project_associations (tenant_id, project_id, subject, selected_branch, pinned_commit, last_validated_at_ms) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT (tenant_id, project_id, subject) DO UPDATE SET last_validated_at_ms = excluded.last_validated_at_ms")
            .bind(&authority.tenant_id)
            .bind(&project.id)
            .bind(&authority.subject)
            .bind(&project.selected_branch)
            .bind(&project.pinned_commit)
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
        let row = sqlx::query("SELECT p.project_id, p.forge_instance_ref, p.provider_project_ref, p.path_with_namespace, p.name, p.default_branch, a.selected_branch, a.pinned_commit, p.web_url FROM workspace_projects p INNER JOIN workspace_project_associations a ON a.tenant_id = p.tenant_id AND a.project_id = p.project_id WHERE p.tenant_id = ? AND p.project_id = ? AND a.subject = ?")
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
        let result = sqlx::query("UPDATE workspace_project_associations SET selected_branch = ?, pinned_commit = ? WHERE tenant_id = ? AND project_id = ? AND subject = ?")
            .bind(branch)
            .bind(commit)
            .bind(&authority.tenant_id)
            .bind(&project.id)
            .bind(&authority.subject)
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

    /// Durably reserve one exact-revision coding session before any Substrate mutation.
    pub async fn reserve_coding_session(
        &self,
        authority: &Authority,
        project_id: &str,
        input: &CreateCodingSession,
        limits: MaterializationLimits,
    ) -> Result<SessionReservation, StoreError> {
        self.ensure_schema().await?;
        let now = now_ms()?;
        let session_id = random_id("session")?;
        let result = sqlx::query("INSERT INTO workspace_coding_sessions (session_id, tenant_id, project_id, owner_subject, source_revision, idempotency_key, base_materialization_ref, working_materialization_ref, manifest_sha256, state, failure_code, max_files, max_total_bytes, max_file_bytes, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, NULL, NULL, NULL, 'preparing', NULL, ?, ?, ?, ?, ?) ON CONFLICT (tenant_id, owner_subject, project_id, idempotency_key) DO NOTHING")
            .bind(&session_id)
            .bind(&authority.tenant_id)
            .bind(project_id)
            .bind(&authority.subject)
            .bind(&input.source_revision)
            .bind(&input.idempotency_key)
            .bind(i64::from(limits.max_files))
            .bind(as_i64(limits.max_total_bytes)?)
            .bind(as_i64(limits.max_file_bytes)?)
            .bind(as_i64(now)?)
            .bind(as_i64(now)?)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        let row = sqlx::query("SELECT session_id, project_id, source_revision, base_materialization_ref, working_materialization_ref, manifest_sha256, state, failure_code, max_files, max_total_bytes, max_file_bytes, created_at_ms, updated_at_ms FROM workspace_coding_sessions WHERE tenant_id = ? AND owner_subject = ? AND project_id = ? AND idempotency_key = ?")
            .bind(&authority.tenant_id)
            .bind(&authority.subject)
            .bind(project_id)
            .bind(&input.idempotency_key)
            .fetch_one(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        let session = coding_session_from_row(&row)?;
        if session.source_revision != input.source_revision {
            return Err(StoreError::Conflict);
        }
        if result.rows_affected() == 1 {
            Ok(SessionReservation::New(session))
        } else {
            Ok(SessionReservation::Existing(session))
        }
    }

    /// List only the current subject's sessions for one canonical project.
    pub async fn coding_sessions(
        &self,
        authority: &Authority,
        project_id: &str,
    ) -> Result<Vec<CodingSession>, StoreError> {
        self.ensure_schema().await?;
        let rows = sqlx::query("SELECT session_id, project_id, source_revision, base_materialization_ref, working_materialization_ref, manifest_sha256, state, failure_code, max_files, max_total_bytes, max_file_bytes, created_at_ms, updated_at_ms FROM workspace_coding_sessions WHERE tenant_id = ? AND project_id = ? AND owner_subject = ? ORDER BY created_at_ms DESC")
            .bind(&authority.tenant_id)
            .bind(project_id)
            .bind(&authority.subject)
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        rows.iter().map(coding_session_from_row).collect()
    }

    /// Read one session only through its tenant, project and owner coordinates.
    pub async fn coding_session(
        &self,
        authority: &Authority,
        session_id: &str,
    ) -> Result<CodingSession, StoreError> {
        self.ensure_schema().await?;
        let row = sqlx::query("SELECT session_id, project_id, source_revision, base_materialization_ref, working_materialization_ref, manifest_sha256, state, failure_code, max_files, max_total_bytes, max_file_bytes, created_at_ms, updated_at_ms FROM workspace_coding_sessions WHERE tenant_id = ? AND session_id = ? AND owner_subject = ?")
            .bind(&authority.tenant_id)
            .bind(session_id)
            .bind(&authority.subject)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::Database)?
            .ok_or(StoreError::NotFound)?;
        coding_session_from_row(&row)
    }

    /// Record a created Substrate reference while the session is still preparing.
    pub async fn record_materialization_ref(
        &self,
        authority: &Authority,
        session_id: &str,
        base: bool,
        materialization_ref: &str,
    ) -> Result<CodingSession, StoreError> {
        self.ensure_schema().await?;
        let now = now_ms()?;
        let statement = if base {
            "UPDATE workspace_coding_sessions SET base_materialization_ref = ?, updated_at_ms = ? WHERE tenant_id = ? AND session_id = ? AND owner_subject = ? AND state = 'preparing' AND (base_materialization_ref IS NULL OR base_materialization_ref = ?)"
        } else {
            "UPDATE workspace_coding_sessions SET working_materialization_ref = ?, updated_at_ms = ? WHERE tenant_id = ? AND session_id = ? AND owner_subject = ? AND state = 'preparing' AND (working_materialization_ref IS NULL OR working_materialization_ref = ?)"
        };
        let result = sqlx::query(statement)
            .bind(materialization_ref)
            .bind(as_i64(now)?)
            .bind(&authority.tenant_id)
            .bind(session_id)
            .bind(&authority.subject)
            .bind(materialization_ref)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() != 1 {
            return Err(StoreError::Conflict);
        }
        self.coding_session(authority, session_id).await
    }

    /// Publish both verified Substrate references and their exact source manifest atomically.
    pub async fn complete_coding_session(
        &self,
        authority: &Authority,
        session_id: &str,
        manifest_sha256: &str,
    ) -> Result<CodingSession, StoreError> {
        let now = now_ms()?;
        let result = sqlx::query("UPDATE workspace_coding_sessions SET state = 'ready', failure_code = NULL, manifest_sha256 = ?, updated_at_ms = ? WHERE tenant_id = ? AND session_id = ? AND owner_subject = ? AND state = 'preparing' AND base_materialization_ref IS NOT NULL AND working_materialization_ref IS NOT NULL")
            .bind(manifest_sha256)
            .bind(as_i64(now)?)
            .bind(&authority.tenant_id)
            .bind(session_id)
            .bind(&authority.subject)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() != 1 {
            return Err(StoreError::Conflict);
        }
        self.coding_session(authority, session_id).await
    }

    /// Record a whole-materialization refusal without publishing partial references as ready.
    pub async fn refuse_coding_session(
        &self,
        authority: &Authority,
        session_id: &str,
        failure_code: &str,
        unknown: bool,
    ) -> Result<CodingSession, StoreError> {
        if !unknown {
            let now = now_ms()?;
            let result = sqlx::query("UPDATE workspace_coding_sessions SET state = 'refused', failure_code = ?, base_materialization_ref = NULL, working_materialization_ref = NULL, manifest_sha256 = NULL, updated_at_ms = ? WHERE tenant_id = ? AND session_id = ? AND owner_subject = ? AND state = 'preparing'")
                .bind(failure_code)
                .bind(as_i64(now)?)
                .bind(&authority.tenant_id)
                .bind(session_id)
                .bind(&authority.subject)
                .execute(&self.pool)
                .await
                .map_err(StoreError::Database)?;
            if result.rows_affected() != 1 {
                return Err(StoreError::Conflict);
            }
            return self.coding_session(authority, session_id).await;
        }
        self.transition_coding_session(
            authority,
            session_id,
            "unknown",
            Some(failure_code),
            None,
            "preparing",
        )
        .await
    }

    /// Move an owned session into cleanup, replaying an already closing or closed request.
    pub async fn begin_close_coding_session(
        &self,
        authority: &Authority,
        session_id: &str,
    ) -> Result<CodingSession, StoreError> {
        let session = self.coding_session(authority, session_id).await?;
        if matches!(
            session.state,
            CodingSessionState::Closing | CodingSessionState::Closed
        ) {
            return Ok(session);
        }
        let now = now_ms()?;
        let (next, expected) = if session.state == CodingSessionState::Refused {
            ("closed", "refused")
        } else {
            ("closing", session_state_name(session.state))
        };
        let result = sqlx::query("UPDATE workspace_coding_sessions SET state = ?, failure_code = NULL, updated_at_ms = ? WHERE tenant_id = ? AND session_id = ? AND owner_subject = ? AND state = ?")
            .bind(next)
            .bind(as_i64(now)?)
            .bind(&authority.tenant_id)
            .bind(session_id)
            .bind(&authority.subject)
            .bind(expected)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() != 1 {
            let raced = self.coding_session(authority, session_id).await?;
            return if matches!(
                raced.state,
                CodingSessionState::Closing | CodingSessionState::Closed
            ) {
                Ok(raced)
            } else {
                Err(StoreError::Conflict)
            };
        }
        self.coding_session(authority, session_id).await
    }

    /// Finish cleanup and remove all live Substrate references from durable state.
    pub async fn complete_close_coding_session(
        &self,
        authority: &Authority,
        session_id: &str,
    ) -> Result<CodingSession, StoreError> {
        let now = now_ms()?;
        let result = sqlx::query("UPDATE workspace_coding_sessions SET state = 'closed', failure_code = NULL, base_materialization_ref = NULL, working_materialization_ref = NULL, updated_at_ms = ? WHERE tenant_id = ? AND session_id = ? AND owner_subject = ? AND state = 'closing'")
            .bind(as_i64(now)?)
            .bind(&authority.tenant_id)
            .bind(session_id)
            .bind(&authority.subject)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() != 1 {
            return Err(StoreError::Conflict);
        }
        self.coding_session(authority, session_id).await
    }

    /// Make an uncertain cleanup explicit so a later close can safely retry observation.
    pub async fn mark_close_unknown(
        &self,
        authority: &Authority,
        session_id: &str,
    ) -> Result<CodingSession, StoreError> {
        self.transition_coding_session(
            authority,
            session_id,
            "unknown",
            Some("materialization_cleanup_unknown"),
            None,
            "closing",
        )
        .await
    }

    async fn transition_coding_session(
        &self,
        authority: &Authority,
        session_id: &str,
        state: &str,
        failure_code: Option<&str>,
        manifest_sha256: Option<&str>,
        expected_state: &str,
    ) -> Result<CodingSession, StoreError> {
        let now = now_ms()?;
        let result = sqlx::query("UPDATE workspace_coding_sessions SET state = ?, failure_code = ?, manifest_sha256 = COALESCE(?, manifest_sha256), updated_at_ms = ? WHERE tenant_id = ? AND session_id = ? AND owner_subject = ? AND state = ?")
            .bind(state)
            .bind(failure_code)
            .bind(manifest_sha256)
            .bind(as_i64(now)?)
            .bind(&authority.tenant_id)
            .bind(session_id)
            .bind(&authority.subject)
            .bind(expected_state)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() != 1 {
            return Err(StoreError::Conflict);
        }
        self.coding_session(authority, session_id).await
    }

    /// Durably reserve one terminal before asking Substrate to create a PTY session.
    pub async fn reserve_terminal(
        &self,
        authority: &Authority,
        coding_session_id: &str,
        input: &CreateTerminal,
        profile: &TerminalProfile,
    ) -> Result<TerminalReservation, StoreError> {
        self.ensure_schema().await?;
        if let Some(row) = sqlx::query("SELECT terminal_id, coding_session_id, owner_subject, agentide_session_id, authority_grant_id, profile_json, initial_columns, initial_rows, substrate_session_ref, process_id, state, exit_code, exit_signal, failure_code, created_at_ms, updated_at_ms FROM workspace_terminals WHERE tenant_id = ? AND owner_subject = ? AND coding_session_id = ? AND idempotency_key = ?")
            .bind(&authority.tenant_id)
            .bind(&authority.subject)
            .bind(coding_session_id)
            .bind(&input.idempotency_key)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::Database)?
        {
            let terminal = terminal_from_row(&row)?;
            if terminal.public.agentide_session_id != input.agentide_session_id
                || terminal.public.authority_grant_id != input.authority_grant_id
                || terminal.public.profile != *profile
                || terminal.initial_columns != input.columns
                || terminal.initial_rows != input.rows
            {
                return Err(StoreError::Conflict);
            }
            return Ok(TerminalReservation::Existing(terminal));
        }
        let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspace_terminals WHERE tenant_id = ? AND owner_subject = ? AND coding_session_id = ? AND state IN ('preparing', 'running', 'unknown')")
            .bind(&authority.tenant_id)
            .bind(&authority.subject)
            .bind(coding_session_id)
            .fetch_one(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if active >= MAX_ACTIVE_TERMINALS_PER_SESSION {
            return Err(StoreError::Conflict);
        }
        let now = now_ms()?;
        let terminal_id = random_id("terminal")?;
        let profile_json = serde_json::to_string(profile).map_err(|_| StoreError::Corrupt)?;
        let result = sqlx::query("INSERT INTO workspace_terminals (terminal_id, tenant_id, coding_session_id, owner_subject, agentide_session_id, authority_grant_id, profile_json, initial_columns, initial_rows, idempotency_key, substrate_session_ref, process_id, state, exit_code, exit_signal, failure_code, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, 'preparing', NULL, NULL, NULL, ?, ?) ON CONFLICT (tenant_id, owner_subject, coding_session_id, idempotency_key) DO NOTHING")
            .bind(&terminal_id)
            .bind(&authority.tenant_id)
            .bind(coding_session_id)
            .bind(&authority.subject)
            .bind(&input.agentide_session_id)
            .bind(&input.authority_grant_id)
            .bind(profile_json)
            .bind(i64::from(input.columns))
            .bind(i64::from(input.rows))
            .bind(&input.idempotency_key)
            .bind(as_i64(now)?)
            .bind(as_i64(now)?)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() == 1 {
            return self
                .terminal(authority, &terminal_id)
                .await
                .map(TerminalReservation::New);
        }
        let row = sqlx::query("SELECT terminal_id, coding_session_id, owner_subject, agentide_session_id, authority_grant_id, profile_json, initial_columns, initial_rows, substrate_session_ref, process_id, state, exit_code, exit_signal, failure_code, created_at_ms, updated_at_ms FROM workspace_terminals WHERE tenant_id = ? AND owner_subject = ? AND coding_session_id = ? AND idempotency_key = ?")
            .bind(&authority.tenant_id)
            .bind(&authority.subject)
            .bind(coding_session_id)
            .bind(&input.idempotency_key)
            .fetch_one(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        let terminal = terminal_from_row(&row)?;
        if terminal.public.agentide_session_id != input.agentide_session_id
            || terminal.public.authority_grant_id != input.authority_grant_id
            || terminal.public.profile != *profile
            || terminal.initial_columns != input.columns
            || terminal.initial_rows != input.rows
        {
            return Err(StoreError::Conflict);
        }
        Ok(TerminalReservation::Existing(terminal))
    }

    /// List durable terminal metadata for one owned coding session.
    pub async fn terminals(
        &self,
        authority: &Authority,
        coding_session_id: &str,
    ) -> Result<Vec<StoredTerminal>, StoreError> {
        self.ensure_schema().await?;
        let rows = sqlx::query("SELECT terminal_id, coding_session_id, owner_subject, agentide_session_id, authority_grant_id, profile_json, initial_columns, initial_rows, substrate_session_ref, process_id, state, exit_code, exit_signal, failure_code, created_at_ms, updated_at_ms FROM workspace_terminals WHERE tenant_id = ? AND owner_subject = ? AND coding_session_id = ? ORDER BY created_at_ms")
            .bind(&authority.tenant_id)
            .bind(&authority.subject)
            .bind(coding_session_id)
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        rows.iter().map(terminal_from_row).collect()
    }

    /// Read one terminal only through its tenant and owner coordinates.
    pub async fn terminal(
        &self,
        authority: &Authority,
        terminal_id: &str,
    ) -> Result<StoredTerminal, StoreError> {
        self.ensure_schema().await?;
        let row = sqlx::query("SELECT terminal_id, coding_session_id, owner_subject, agentide_session_id, authority_grant_id, profile_json, initial_columns, initial_rows, substrate_session_ref, process_id, state, exit_code, exit_signal, failure_code, created_at_ms, updated_at_ms FROM workspace_terminals WHERE tenant_id = ? AND owner_subject = ? AND terminal_id = ?")
            .bind(&authority.tenant_id)
            .bind(&authority.subject)
            .bind(terminal_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::Database)?
            .ok_or(StoreError::NotFound)?;
        terminal_from_row(&row)
    }

    /// Publish the exact Substrate PTY and process identities after successful creation.
    pub async fn complete_terminal(
        &self,
        authority: &Authority,
        terminal_id: &str,
        substrate_session_ref: &str,
        process_id: &str,
    ) -> Result<StoredTerminal, StoreError> {
        let now = now_ms()?;
        let result = sqlx::query("UPDATE workspace_terminals SET substrate_session_ref = ?, process_id = ?, state = 'running', failure_code = NULL, updated_at_ms = ? WHERE tenant_id = ? AND owner_subject = ? AND terminal_id = ? AND state = 'preparing' AND (substrate_session_ref IS NULL OR substrate_session_ref = ?)")
            .bind(substrate_session_ref)
            .bind(process_id)
            .bind(as_i64(now)?)
            .bind(&authority.tenant_id)
            .bind(&authority.subject)
            .bind(terminal_id)
            .bind(substrate_session_ref)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() != 1 {
            return Err(StoreError::Conflict);
        }
        self.terminal(authority, terminal_id).await
    }

    /// Record a named creation refusal without publishing a partial live terminal.
    pub async fn refuse_terminal(
        &self,
        authority: &Authority,
        terminal_id: &str,
        failure_code: &str,
        unknown: bool,
    ) -> Result<StoredTerminal, StoreError> {
        let state = if unknown { "unknown" } else { "refused" };
        let now = now_ms()?;
        let result = sqlx::query("UPDATE workspace_terminals SET state = ?, failure_code = ?, substrate_session_ref = NULL, process_id = NULL, updated_at_ms = ? WHERE tenant_id = ? AND owner_subject = ? AND terminal_id = ? AND state = 'preparing'")
            .bind(state)
            .bind(failure_code)
            .bind(as_i64(now)?)
            .bind(&authority.tenant_id)
            .bind(&authority.subject)
            .bind(terminal_id)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() != 1 {
            return Err(StoreError::Conflict);
        }
        self.terminal(authority, terminal_id).await
    }

    /// Record an observed process state without retaining the request credential in a background task.
    pub async fn observe_terminal(
        &self,
        tenant_id: &str,
        owner_subject: &str,
        terminal_id: &str,
        state: TerminalState,
        exit: Option<&TerminalExit>,
    ) -> Result<(), StoreError> {
        self.ensure_schema().await?;
        let now = now_ms()?;
        let exit_code = exit.and_then(|exit| exit.code).map(i64::from);
        let exit_signal = exit.and_then(|exit| exit.signal.as_deref());
        let result = sqlx::query("UPDATE workspace_terminals SET state = ?, exit_code = ?, exit_signal = ?, updated_at_ms = ? WHERE tenant_id = ? AND owner_subject = ? AND terminal_id = ? AND state IN ('running', 'unknown')")
            .bind(terminal_state_name(state))
            .bind(exit_code)
            .bind(exit_signal)
            .bind(as_i64(now)?)
            .bind(tenant_id)
            .bind(owner_subject)
            .bind(terminal_id)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(StoreError::NotFound)
        }
    }

    /// Mark an explicitly killed or session-cleaned terminal and forget its Substrate reference.
    pub async fn complete_terminal_termination(
        &self,
        authority: &Authority,
        terminal_id: &str,
        exit: Option<&TerminalExit>,
    ) -> Result<StoredTerminal, StoreError> {
        let now = now_ms()?;
        let exit_code = exit.and_then(|exit| exit.code).map(i64::from);
        let exit_signal = exit.and_then(|exit| exit.signal.as_deref());
        let result = sqlx::query("UPDATE workspace_terminals SET state = 'terminated', substrate_session_ref = NULL, exit_code = ?, exit_signal = ?, failure_code = NULL, updated_at_ms = ? WHERE tenant_id = ? AND owner_subject = ? AND terminal_id = ?")
            .bind(exit_code)
            .bind(exit_signal)
            .bind(as_i64(now)?)
            .bind(&authority.tenant_id)
            .bind(&authority.subject)
            .bind(terminal_id)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() != 1 {
            return Err(StoreError::NotFound);
        }
        self.terminal(authority, terminal_id).await
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
        web_url: row.try_get("web_url").map_err(|_| StoreError::Corrupt)?,
    })
}

fn coding_session_from_row(row: &AnyRow) -> Result<CodingSession, StoreError> {
    let state: String = row.try_get("state").map_err(|_| StoreError::Corrupt)?;
    Ok(CodingSession {
        id: row.try_get("session_id").map_err(|_| StoreError::Corrupt)?,
        project_id: row.try_get("project_id").map_err(|_| StoreError::Corrupt)?,
        source_revision: row
            .try_get("source_revision")
            .map_err(|_| StoreError::Corrupt)?,
        base_materialization_ref: row
            .try_get("base_materialization_ref")
            .map_err(|_| StoreError::Corrupt)?,
        working_materialization_ref: row
            .try_get("working_materialization_ref")
            .map_err(|_| StoreError::Corrupt)?,
        manifest_sha256: row
            .try_get("manifest_sha256")
            .map_err(|_| StoreError::Corrupt)?,
        state: match state.as_str() {
            "preparing" => CodingSessionState::Preparing,
            "ready" => CodingSessionState::Ready,
            "refused" => CodingSessionState::Refused,
            "unknown" => CodingSessionState::Unknown,
            "closing" => CodingSessionState::Closing,
            "closed" => CodingSessionState::Closed,
            _ => return Err(StoreError::Corrupt),
        },
        failure_code: row
            .try_get("failure_code")
            .map_err(|_| StoreError::Corrupt)?,
        limits: MaterializationLimits {
            max_files: u32::try_from(
                row.try_get::<i64, _>("max_files")
                    .map_err(|_| StoreError::Corrupt)?,
            )
            .map_err(|_| StoreError::Corrupt)?,
            max_total_bytes: from_i64(
                row.try_get("max_total_bytes")
                    .map_err(|_| StoreError::Corrupt)?,
            )?,
            max_file_bytes: from_i64(
                row.try_get("max_file_bytes")
                    .map_err(|_| StoreError::Corrupt)?,
            )?,
        },
        created_at_ms: from_i64(
            row.try_get("created_at_ms")
                .map_err(|_| StoreError::Corrupt)?,
        )?,
        updated_at_ms: from_i64(
            row.try_get("updated_at_ms")
                .map_err(|_| StoreError::Corrupt)?,
        )?,
    })
}

fn terminal_from_row(row: &AnyRow) -> Result<StoredTerminal, StoreError> {
    let profile_json: String = row
        .try_get("profile_json")
        .map_err(|_| StoreError::Corrupt)?;
    let profile =
        serde_json::from_str::<TerminalProfile>(&profile_json).map_err(|_| StoreError::Corrupt)?;
    let state: String = row.try_get("state").map_err(|_| StoreError::Corrupt)?;
    let exit_code = row
        .try_get::<Option<i64>, _>("exit_code")
        .map_err(|_| StoreError::Corrupt)?
        .map(|code| i32::try_from(code).map_err(|_| StoreError::Corrupt))
        .transpose()?;
    let exit_signal = row
        .try_get::<Option<String>, _>("exit_signal")
        .map_err(|_| StoreError::Corrupt)?;
    let exit = (exit_code.is_some() || exit_signal.is_some()).then_some(TerminalExit {
        code: exit_code,
        signal: exit_signal,
    });
    let initial_columns = u16::try_from(
        row.try_get::<i64, _>("initial_columns")
            .map_err(|_| StoreError::Corrupt)?,
    )
    .map_err(|_| StoreError::Corrupt)?;
    let initial_rows = u16::try_from(
        row.try_get::<i64, _>("initial_rows")
            .map_err(|_| StoreError::Corrupt)?,
    )
    .map_err(|_| StoreError::Corrupt)?;
    Ok(StoredTerminal {
        public: TerminalSession {
            id: row
                .try_get("terminal_id")
                .map_err(|_| StoreError::Corrupt)?,
            coding_session_id: row
                .try_get("coding_session_id")
                .map_err(|_| StoreError::Corrupt)?,
            agentide_session_id: row
                .try_get("agentide_session_id")
                .map_err(|_| StoreError::Corrupt)?,
            authority_grant_id: row
                .try_get("authority_grant_id")
                .map_err(|_| StoreError::Corrupt)?,
            profile,
            actor: row
                .try_get("owner_subject")
                .map_err(|_| StoreError::Corrupt)?,
            process_id: row.try_get("process_id").map_err(|_| StoreError::Corrupt)?,
            state: match state.as_str() {
                "preparing" => TerminalState::Preparing,
                "running" => TerminalState::Running,
                "exited" => TerminalState::Exited,
                "terminated" => TerminalState::Terminated,
                "refused" => TerminalState::Refused,
                "unknown" => TerminalState::Unknown,
                _ => return Err(StoreError::Corrupt),
            },
            exit,
            failure_code: row
                .try_get("failure_code")
                .map_err(|_| StoreError::Corrupt)?,
            created_at_ms: from_i64(
                row.try_get("created_at_ms")
                    .map_err(|_| StoreError::Corrupt)?,
            )?,
            updated_at_ms: from_i64(
                row.try_get("updated_at_ms")
                    .map_err(|_| StoreError::Corrupt)?,
            )?,
        },
        substrate_session_ref: row
            .try_get("substrate_session_ref")
            .map_err(|_| StoreError::Corrupt)?,
        initial_columns,
        initial_rows,
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

fn session_state_name(state: CodingSessionState) -> &'static str {
    match state {
        CodingSessionState::Preparing => "preparing",
        CodingSessionState::Ready => "ready",
        CodingSessionState::Refused => "refused",
        CodingSessionState::Unknown => "unknown",
        CodingSessionState::Closing => "closing",
        CodingSessionState::Closed => "closed",
    }
}

fn terminal_state_name(state: TerminalState) -> &'static str {
    match state {
        TerminalState::Preparing => "preparing",
        TerminalState::Running => "running",
        TerminalState::Exited => "exited",
        TerminalState::Terminated => "terminated",
        TerminalState::Refused => "refused",
        TerminalState::Unknown => "unknown",
    }
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
            session_authorization: "Bearer synthetic-session".to_owned(),
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
            default_branch: Some("trunk".to_owned()),
            selected_branch: "trunk".to_owned(),
            pinned_commit: None,
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
        let first_selected = store
            .select_branch(&first, &opened, "feature", &"f".repeat(40))
            .await
            .unwrap();
        assert_eq!(first_selected.selected_branch, "feature");
        let second_still_default = store.project(&second, "project-one").await.unwrap();
        assert_eq!(second_still_default.selected_branch, "trunk");
        assert!(second_still_default.pinned_commit.is_none());
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
    async fn coding_session_reservation_is_exact_owned_and_publishes_both_refs_atomically() {
        let store = store().await;
        let owner = authority("person:owner");
        let other = authority("person:other");
        store.open_project(&owner, &project()).await.unwrap();
        let input = CreateCodingSession {
            source_revision: "a".repeat(40),
            idempotency_key: "open-editor-on-trunk".to_owned(),
        };
        let limits = MaterializationLimits {
            max_files: 4_096,
            max_total_bytes: 256 * 1024 * 1024,
            max_file_bytes: 180 * 1024,
        };
        let SessionReservation::New(reserved) = store
            .reserve_coding_session(&owner, "project-one", &input, limits)
            .await
            .unwrap()
        else {
            panic!("first reservation must be new");
        };
        let SessionReservation::Existing(replayed) = store
            .reserve_coding_session(&owner, "project-one", &input, limits)
            .await
            .unwrap()
        else {
            panic!("same reservation must replay");
        };
        assert_eq!(replayed.id, reserved.id);
        assert!(matches!(
            store.coding_session(&other, &reserved.id).await,
            Err(StoreError::NotFound)
        ));
        store
            .record_materialization_ref(&owner, &reserved.id, true, "ws_base")
            .await
            .unwrap();
        let preparing = store
            .record_materialization_ref(&owner, &reserved.id, false, "ws_working")
            .await
            .unwrap();
        assert_eq!(preparing.state, CodingSessionState::Preparing);
        let ready = store
            .complete_coding_session(&owner, &reserved.id, &"f".repeat(64))
            .await
            .unwrap();
        assert_eq!(ready.state, CodingSessionState::Ready);
        assert_eq!(ready.base_materialization_ref.as_deref(), Some("ws_base"));
        assert_eq!(
            ready.working_materialization_ref.as_deref(),
            Some("ws_working")
        );
        assert_eq!(
            ready.manifest_sha256.as_deref(),
            Some("f".repeat(64).as_str())
        );
    }

    #[tokio::test]
    async fn coding_session_cannot_publish_partial_materialization_and_refusal_clears_references() {
        let store = store().await;
        let owner = authority("person:owner");
        store.open_project(&owner, &project()).await.unwrap();
        let input = CreateCodingSession {
            source_revision: "a".repeat(40),
            idempotency_key: "partial-materialization".to_owned(),
        };
        let SessionReservation::New(reserved) = store
            .reserve_coding_session(
                &owner,
                "project-one",
                &input,
                MaterializationLimits {
                    max_files: 4_096,
                    max_total_bytes: 256 * 1024 * 1024,
                    max_file_bytes: 180 * 1024,
                },
            )
            .await
            .unwrap()
        else {
            panic!("first reservation must be new");
        };
        store
            .record_materialization_ref(&owner, &reserved.id, true, "ws_base")
            .await
            .unwrap();
        assert!(matches!(
            store
                .complete_coding_session(&owner, &reserved.id, &"f".repeat(64))
                .await,
            Err(StoreError::Conflict)
        ));

        let refused = store
            .refuse_coding_session(&owner, &reserved.id, "source_refused", false)
            .await
            .unwrap();
        assert_eq!(refused.state, CodingSessionState::Refused);
        assert!(refused.base_materialization_ref.is_none());
        assert!(refused.working_materialization_ref.is_none());
        assert!(refused.manifest_sha256.is_none());
        let closed = store
            .begin_close_coding_session(&owner, &reserved.id)
            .await
            .unwrap();
        assert_eq!(closed.state, CodingSessionState::Closed);
    }

    fn terminal_profile(id: &str) -> TerminalProfile {
        TerminalProfile {
            id: id.to_owned(),
            label: "Hosted Rust toolchain".to_owned(),
            runtime_ref: "substrate:workspace-default@sha256:test".to_owned(),
            shell: "/bin/sh".to_owned(),
            arguments: vec!["-l".to_owned()],
            working_directory: "/workspace".to_owned(),
            environment: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            workspace_access: workspace_core::TerminalWorkspaceAccess::ReadWrite,
            network: workspace_core::TerminalNetworkPosture::None,
            limits: workspace_core::TerminalLimits {
                timeout_ms: 60_000,
                cpu_millis: 60_000,
                memory_bytes: 128 * 1024 * 1024,
                processes: 64,
                output_bytes: 1024 * 1024,
                input_bytes: 1024 * 1024,
                frame_bytes: 64 * 1024,
                queued_frames: 16,
                lease_ttl_ms: 60_000,
            },
        }
    }

    #[tokio::test]
    async fn terminal_reservation_is_exact_idempotent_and_owner_scoped() {
        let store = store().await;
        let owner = authority("person:owner");
        let other = authority("person:other");
        store.open_project(&owner, &project()).await.unwrap();
        let SessionReservation::New(session) = store
            .reserve_coding_session(
                &owner,
                "project-one",
                &CreateCodingSession {
                    source_revision: "a".repeat(40),
                    idempotency_key: "terminal-session".to_owned(),
                },
                MaterializationLimits {
                    max_files: 4_096,
                    max_total_bytes: 256 * 1024 * 1024,
                    max_file_bytes: 180 * 1024,
                },
            )
            .await
            .unwrap()
        else {
            panic!("first coding-session reservation must be new");
        };
        let input = CreateTerminal {
            agentide_session_id: "agentide-session-one".to_owned(),
            authority_grant_id: "grant-one".to_owned(),
            profile_id: "rust".to_owned(),
            columns: 120,
            rows: 36,
            idempotency_key: "terminal-one".to_owned(),
        };
        let profile = terminal_profile("rust");
        let TerminalReservation::New(reserved) = store
            .reserve_terminal(&owner, &session.id, &input, &profile)
            .await
            .unwrap()
        else {
            panic!("first terminal reservation must be new");
        };
        let TerminalReservation::Existing(replayed) = store
            .reserve_terminal(&owner, &session.id, &input, &profile)
            .await
            .unwrap()
        else {
            panic!("same terminal reservation must replay");
        };
        assert_eq!(replayed.public.id, reserved.public.id);
        assert!(matches!(
            store.terminal(&other, &reserved.public.id).await,
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store
                .reserve_terminal(&owner, &session.id, &input, &terminal_profile("other"))
                .await,
            Err(StoreError::Conflict)
        ));

        let running = store
            .complete_terminal(&owner, &reserved.public.id, "pty-session", "process-one")
            .await
            .unwrap();
        assert_eq!(running.public.state, TerminalState::Running);
        assert_eq!(running.public.process_id.as_deref(), Some("process-one"));
        assert_eq!(
            running.substrate_session_ref.as_deref(),
            Some("pty-session")
        );

        store
            .observe_terminal(
                &owner.tenant_id,
                &owner.subject,
                &reserved.public.id,
                TerminalState::Exited,
                Some(&TerminalExit {
                    code: Some(0),
                    signal: None,
                }),
            )
            .await
            .unwrap();
        let exited = store.terminal(&owner, &reserved.public.id).await.unwrap();
        assert_eq!(exited.public.state, TerminalState::Exited);
        assert_eq!(exited.public.exit.and_then(|exit| exit.code), Some(0));
    }

    #[tokio::test]
    async fn closing_a_ready_session_retains_provenance_but_clears_live_references() {
        let store = store().await;
        let owner = authority("person:owner");
        store.open_project(&owner, &project()).await.unwrap();
        let input = CreateCodingSession {
            source_revision: "a".repeat(40),
            idempotency_key: "close-ready".to_owned(),
        };
        let SessionReservation::New(reserved) = store
            .reserve_coding_session(
                &owner,
                "project-one",
                &input,
                MaterializationLimits {
                    max_files: 4_096,
                    max_total_bytes: 256 * 1024 * 1024,
                    max_file_bytes: 180 * 1024,
                },
            )
            .await
            .unwrap()
        else {
            panic!("first reservation must be new");
        };
        store
            .record_materialization_ref(&owner, &reserved.id, true, "ws_base")
            .await
            .unwrap();
        store
            .record_materialization_ref(&owner, &reserved.id, false, "ws_working")
            .await
            .unwrap();
        let ready = store
            .complete_coding_session(&owner, &reserved.id, &"f".repeat(64))
            .await
            .unwrap();
        let closing = store
            .begin_close_coding_session(&owner, &ready.id)
            .await
            .unwrap();
        assert_eq!(closing.state, CodingSessionState::Closing);
        assert_eq!(closing.base_materialization_ref.as_deref(), Some("ws_base"));

        let closed = store
            .complete_close_coding_session(&owner, &ready.id)
            .await
            .unwrap();
        assert_eq!(closed.state, CodingSessionState::Closed);
        assert!(closed.base_materialization_ref.is_none());
        assert!(closed.working_materialization_ref.is_none());
        assert_eq!(
            closed.manifest_sha256.as_deref(),
            Some("f".repeat(64).as_str())
        );
        let replay = store
            .begin_close_coding_session(&owner, &ready.id)
            .await
            .unwrap();
        assert_eq!(replay.state, CodingSessionState::Closed);
    }

    #[tokio::test]
    async fn refresh_records_a_commit_boundary_without_exposing_personal_threads() {
        let store = store().await;
        let owner = authority("person:owner");
        let other = authority("person:other");
        let opened = store.open_project(&owner, &project()).await.unwrap();
        let pinned = store
            .select_branch(&owner, &opened, "trunk", &"a".repeat(40))
            .await
            .unwrap();
        let thread = store
            .create_thread(
                &owner,
                &pinned.id,
                &CreateThread {
                    branch: "trunk".to_owned(),
                    pinned_commit: "a".repeat(40),
                    title: "Understand the service".to_owned(),
                },
            )
            .await
            .unwrap();
        let refreshed = store
            .select_branch(&owner, &pinned, "trunk", &"b".repeat(40))
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
            branch: "trunk".to_owned(),
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
