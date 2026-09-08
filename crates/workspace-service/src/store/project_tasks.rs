//! Durable coordinator bookkeeping without a dependency on the task executor.

use workspace_core::{Message, MessageRole};

use super::{Authority, Store, StoreError, as_i64, message_from_row, now_ms, thread_from_row};

pub(super) const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS workspace_message_observations (thread_id TEXT NOT NULL, user_sequence BIGINT NOT NULL, PRIMARY KEY (thread_id, user_sequence))",
    "CREATE TABLE IF NOT EXISTS workspace_message_results (thread_id TEXT NOT NULL, user_sequence BIGINT NOT NULL, task_id TEXT NOT NULL, result_sequence BIGINT NOT NULL, PRIMARY KEY (thread_id, user_sequence), UNIQUE (thread_id, result_sequence))",
];

impl Store {
    /// Only newly dispatched turns carry a completion marker; legacy finished rows are not guessed.
    pub async fn message_needs_completion(
        &self,
        authority: &Authority,
        thread_id: &str,
        sequence: u64,
    ) -> Result<bool, StoreError> {
        self.owned_thread(authority, thread_id).await?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspace_message_observations o WHERE o.thread_id = ? AND o.user_sequence = ? AND NOT EXISTS (SELECT 1 FROM workspace_message_results r WHERE r.thread_id = o.thread_id AND r.user_sequence = o.user_sequence)")
            .bind(thread_id).bind(as_i64(sequence)?).fetch_one(&self.pool).await.map_err(StoreError::Database)?;
        Ok(count == 1)
    }

    pub async fn workflow_project(
        &self,
        authority: &Authority,
        run_id: &str,
    ) -> Result<String, StoreError> {
        self.ensure_schema().await?;
        sqlx::query_scalar("SELECT project_id FROM workspace_workflow_runs WHERE tenant_id = ? AND actor_subject = ? AND run_id = ?")
            .bind(&authority.tenant_id).bind(&authority.subject).bind(run_id).fetch_optional(&self.pool)
            .await.map_err(StoreError::Database)?.ok_or(StoreError::NotFound)
    }

    /// Result retries are single-assignment and atomic with the personal thread append.
    pub async fn complete_message_task(
        &self,
        authority: &Authority,
        thread_id: &str,
        sequence: u64,
        task_id: &str,
        role: MessageRole,
        content: &str,
    ) -> Result<Message, StoreError> {
        if self.message_task(authority, thread_id, sequence).await? != task_id
            || !matches!(role, MessageRole::Assistant | MessageRole::System)
        {
            return Err(StoreError::Conflict);
        }
        let mut tx = self.pool.begin().await.map_err(StoreError::Database)?;
        let row = sqlx::query("UPDATE workspace_threads SET updated_at_ms = updated_at_ms WHERE tenant_id = ? AND owner_subject = ? AND thread_id = ? RETURNING thread_id, project_id, branch, pinned_commit, title, created_at_ms")
            .bind(&authority.tenant_id).bind(&authority.subject).bind(thread_id).fetch_optional(&mut *tx)
            .await.map_err(StoreError::Database)?.ok_or(StoreError::NotFound)?;
        let thread = thread_from_row(&row)?;
        if let Some(existing) = sqlx::query("SELECT m.sequence, m.role, m.content, m.branch, m.commit_ref, m.created_at_ms FROM workspace_message_results r JOIN workspace_messages m ON m.thread_id = r.thread_id AND m.sequence = r.result_sequence WHERE r.thread_id = ? AND r.user_sequence = ? AND r.task_id = ?")
            .bind(thread_id).bind(as_i64(sequence)?).bind(task_id).fetch_optional(&mut *tx).await.map_err(StoreError::Database)?
        {
            let message = message_from_row(&existing)?;
            if message.role != role || message.content != content { return Err(StoreError::Conflict); }
            tx.commit().await.map_err(StoreError::Database)?;
            return Ok(message);
        }
        let result_sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM workspace_messages WHERE thread_id = ?",
        )
        .bind(thread_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(StoreError::Database)?;
        let now = now_ms()?;
        let role_name = if role == MessageRole::Assistant {
            "assistant"
        } else {
            "system"
        };
        sqlx::query("INSERT INTO workspace_messages (thread_id, sequence, role, content, branch, commit_ref, created_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(thread_id).bind(result_sequence).bind(role_name).bind(content).bind(&thread.branch)
            .bind(&thread.pinned_commit).bind(as_i64(now)?).execute(&mut *tx).await.map_err(StoreError::Database)?;
        sqlx::query("INSERT INTO workspace_message_results (thread_id, user_sequence, task_id, result_sequence) VALUES (?, ?, ?, ?)")
            .bind(thread_id).bind(as_i64(sequence)?).bind(task_id).bind(result_sequence)
            .execute(&mut *tx).await.map_err(StoreError::Database)?;
        tx.commit().await.map_err(StoreError::Database)?;
        Ok(Message {
            sequence: u64::try_from(result_sequence).map_err(|_| StoreError::Corrupt)?,
            role,
            content: content.to_owned(),
            branch: thread.branch,
            commit: thread.pinned_commit,
            created_at_ms: now,
        })
    }
}
