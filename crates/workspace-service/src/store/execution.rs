//! Downward attempt registrations and atomic request replay consumption.

use sqlx::{Any, Row, Transaction};
use workspace_client::attestation::VerifiedRequest;
use workspace_core::ExecutionAttempt;

use super::{Authority, Store, StoreError, as_i64, now_ms};

pub(super) const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS workspace_execution_attempts (tenant_id TEXT NOT NULL, owner_subject TEXT NOT NULL, attempt_id TEXT NOT NULL, workspace_session_id TEXT NOT NULL, task_id TEXT NOT NULL, agentide_session_id TEXT NOT NULL, context_json TEXT NOT NULL, closed BIGINT NOT NULL, PRIMARY KEY (tenant_id, owner_subject, attempt_id))",
    "CREATE TABLE IF NOT EXISTS workspace_host_request_receipts (issuer TEXT NOT NULL, nonce TEXT NOT NULL, session_sha256 TEXT NOT NULL, request_sha256 TEXT NOT NULL, expires_at BIGINT NOT NULL, PRIMARY KEY (issuer, nonce))",
    "CREATE INDEX IF NOT EXISTS workspace_host_request_expiry ON workspace_host_request_receipts (expires_at)",
];

impl Store {
    /// Register or close one immutable attempt. A close before an open leaves a terminal tombstone.
    pub async fn record_execution_attempt(
        &self,
        authority: &Authority,
        attempt: &ExecutionAttempt,
        close: bool,
        proof: &VerifiedRequest,
    ) -> Result<(), StoreError> {
        self.ensure_schema().await?;
        let context = serde_json::to_string(attempt).map_err(|_| StoreError::Corrupt)?;
        let mut tx = self.pool.begin().await.map_err(StoreError::Database)?;
        consume(&mut tx, proof).await?;
        let inserted = sqlx::query("INSERT INTO workspace_execution_attempts (tenant_id, owner_subject, attempt_id, workspace_session_id, task_id, agentide_session_id, context_json, closed) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (tenant_id, owner_subject, attempt_id) DO NOTHING")
            .bind(&authority.tenant_id).bind(&authority.subject).bind(&attempt.attempt_id)
            .bind(&attempt.workspace_session_id).bind(&attempt.task_id).bind(&attempt.agentide_session_id)
            .bind(&context).bind(i64::from(close)).execute(&mut *tx).await.map_err(StoreError::Database)?;
        if inserted.rows_affected() == 0 {
            // A conditional write serializes with admissions and competing closes on both drivers.
            let changed = sqlx::query("UPDATE workspace_execution_attempts SET closed = ? WHERE tenant_id = ? AND owner_subject = ? AND attempt_id = ? AND context_json = ? AND (closed = 0 OR ? = 1)")
                .bind(i64::from(close)).bind(&authority.tenant_id).bind(&authority.subject)
                .bind(&attempt.attempt_id).bind(&context).bind(i64::from(close))
                .execute(&mut *tx).await.map_err(StoreError::Database)?;
            if changed.rows_affected() != 1 {
                return Err(StoreError::Conflict);
            }
        }
        tx.commit().await.map_err(StoreError::Database)
    }

    /// Consume a fresh executor request and return only its still-open, exactly owned context.
    pub async fn admit_execution_request(
        &self,
        authority: &Authority,
        session_id: &str,
        task_id: &str,
        attempt_id: &str,
        agentide_session_id: &str,
        proof: &VerifiedRequest,
    ) -> Result<ExecutionAttempt, StoreError> {
        self.ensure_schema().await?;
        let mut tx = self.pool.begin().await.map_err(StoreError::Database)?;
        let row = sqlx::query("UPDATE workspace_execution_attempts SET closed = 0 WHERE tenant_id = ? AND owner_subject = ? AND workspace_session_id = ? AND task_id = ? AND attempt_id = ? AND agentide_session_id = ? AND closed = 0 RETURNING context_json")
            .bind(&authority.tenant_id).bind(&authority.subject).bind(session_id).bind(task_id)
            .bind(attempt_id).bind(agentide_session_id).fetch_optional(&mut *tx).await
            .map_err(StoreError::Database)?.ok_or(StoreError::Conflict)?;
        consume(&mut tx, proof).await?;
        let context: String = row.try_get("context_json").map_err(StoreError::Database)?;
        let attempt = serde_json::from_str(&context).map_err(|_| StoreError::Corrupt)?;
        tx.commit().await.map_err(StoreError::Database)?;
        Ok(attempt)
    }

    /// Admit a single coordinator request after current Identity and project ownership checks.
    pub async fn consume_host_request(&self, proof: &VerifiedRequest) -> Result<(), StoreError> {
        self.ensure_schema().await?;
        let mut tx = self.pool.begin().await.map_err(StoreError::Database)?;
        consume(&mut tx, proof).await?;
        tx.commit().await.map_err(StoreError::Database)
    }
}

async fn consume(tx: &mut Transaction<'_, Any>, proof: &VerifiedRequest) -> Result<(), StoreError> {
    let current = now_ms()? / 1000;
    if proof.expires_at() <= current {
        return Err(StoreError::Conflict);
    }
    sqlx::query("DELETE FROM workspace_host_request_receipts WHERE expires_at < ?")
        .bind(as_i64(current)?)
        .execute(&mut **tx)
        .await
        .map_err(StoreError::Database)?;
    let result = sqlx::query("INSERT INTO workspace_host_request_receipts (issuer, nonce, session_sha256, request_sha256, expires_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT (issuer, nonce) DO NOTHING")
        .bind(proof.issuer()).bind(proof.nonce()).bind(proof.session_sha256()).bind(proof.request_sha256())
        .bind(as_i64(proof.expires_at())?).execute(&mut **tx).await.map_err(StoreError::Database)?;
    if result.rows_affected() != 1 {
        return Err(StoreError::Conflict);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use workspace_client::attestation::{HostRole, RequestSigner, RequestVerifier};

    fn authority() -> Authority {
        Authority {
            tenant_id: "tenant-one".into(),
            subject: "person:owner".into(),
            connector_bearer: "not-retained".to_owned().into(),
            session_authorization: "Bearer synthetic-session".to_owned().into(),
            context: connectors_client::operation::OwnerContext {
                tenant_id: "tenant-one".into(),
                agent_id: "workspace:test".into(),
                agent_revision: 1,
                authority_snapshot_id: "identity:test".into(),
                authority_snapshot_sha256: "a".repeat(64),
            },
        }
    }

    fn proof() -> VerifiedRequest {
        let der = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(der.as_ref()).unwrap();
        let private = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
            base64::engine::general_purpose::STANDARD.encode(der.as_ref())
        );
        let mut public = vec![
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0,
        ];
        public.extend_from_slice(pair.public_key().as_ref());
        let public = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
            base64::engine::general_purpose::STANDARD.encode(public)
        );
        let signer = RequestSigner::from_pem(HostRole::Executor, private.as_bytes()).unwrap();
        let verifier = RequestVerifier::from_pem(HostRole::Executor, public.as_bytes()).unwrap();
        let token = signer
            .sign("Bearer synthetic-session", "POST", "/attempts", b"{}")
            .unwrap();
        verifier
            .verify(
                &token,
                "Bearer synthetic-session",
                "POST",
                "/attempts",
                b"{}",
            )
            .unwrap()
    }

    fn attempt() -> ExecutionAttempt {
        ExecutionAttempt {
            task_id: "task-one".into(),
            attempt_id: "attempt-one".into(),
            agent_id: "agent-one".into(),
            delegation_id: None,
            workspace_session_id: "session-one".into(),
            agentide_session_id: "editor-one".into(),
            input: serde_json::json!({"kind":"coding_session_turn", "prompt":"inspect"}),
        }
    }

    async fn admit(
        store: &Store,
        owner: &Authority,
        value: &ExecutionAttempt,
        proof: &VerifiedRequest,
    ) -> Result<ExecutionAttempt, StoreError> {
        store
            .admit_execution_request(
                owner,
                &value.workspace_session_id,
                &value.task_id,
                &value.attempt_id,
                &value.agentide_session_id,
                proof,
            )
            .await
    }

    #[tokio::test]
    async fn replay_and_closed_attempts_remain_refused_after_store_reconstruction() {
        let url = "sqlite:file:execution-recovery?mode=memory&cache=shared";
        let store = Store::connect_lazy(url).unwrap();
        let owner = authority();
        let value = attempt();
        store
            .record_execution_attempt(&owner, &value, false, &proof())
            .await
            .unwrap();
        let request = proof();
        assert_eq!(
            admit(&store, &owner, &value, &request).await.unwrap(),
            value
        );
        let recovered = Store::connect_lazy(url).unwrap();
        assert!(
            matches!(
                admit(&recovered, &owner, &value, &request).await,
                Err(StoreError::Conflict)
            ),
            "a restarted service must retain the consumed nonce"
        );
        recovered
            .record_execution_attempt(&owner, &value, true, &proof())
            .await
            .unwrap();
        assert!(
            matches!(
                admit(&store, &owner, &value, &proof()).await,
                Err(StoreError::Conflict)
            ),
            "fresh signed requests must fail after acknowledged close"
        );
        assert!(
            matches!(
                store
                    .record_execution_attempt(&owner, &value, false, &proof())
                    .await,
                Err(StoreError::Conflict)
            ),
            "late open must not resurrect a closed attempt"
        );
    }

    #[tokio::test]
    async fn owner_and_all_attempt_coordinates_are_single_assignment() {
        let store = Store::connect_lazy("sqlite::memory:").unwrap();
        let owner = authority();
        let value = attempt();
        assert!(
            matches!(
                admit(&store, &owner, &value, &proof()).await,
                Err(StoreError::Conflict)
            ),
            "a signed request cannot invent an unregistered attempt"
        );
        store
            .record_execution_attempt(&owner, &value, false, &proof())
            .await
            .unwrap();
        for field in ["tenant", "subject", "session", "task", "attempt", "editor"] {
            let mut other_owner = owner.clone();
            let mut other = value.clone();
            match field {
                "tenant" => other_owner.tenant_id = "another-tenant".into(),
                "subject" => other_owner.subject = "another-person".into(),
                "session" => other.workspace_session_id = "another-session".into(),
                "task" => other.task_id = "another-task".into(),
                "attempt" => other.attempt_id = "another-attempt".into(),
                _ => other.agentide_session_id = "another-editor".into(),
            }
            assert!(
                matches!(
                    admit(&store, &other_owner, &other, &proof()).await,
                    Err(StoreError::Conflict)
                ),
                "changed {field} must refuse registration lookup"
            );
        }
        let mut changed = value.clone();
        changed.input = serde_json::json!({"prompt":"different"});
        assert!(
            matches!(
                store
                    .record_execution_attempt(&owner, &changed, false, &proof())
                    .await,
                Err(StoreError::Conflict)
            ),
            "same attempt cannot acquire different task context"
        );
        assert_eq!(
            admit(&store, &owner, &value, &proof()).await.unwrap(),
            value,
            "refused mutations must leave the original registration intact"
        );
    }

    #[tokio::test]
    async fn close_before_open_is_a_terminal_tombstone() {
        let store = Store::connect_lazy("sqlite::memory:").unwrap();
        let owner = authority();
        let value = attempt();
        store
            .record_execution_attempt(&owner, &value, true, &proof())
            .await
            .unwrap();
        store
            .record_execution_attempt(&owner, &value, true, &proof())
            .await
            .expect("fresh close retry remains idempotent");
        assert!(
            matches!(
                store
                    .record_execution_attempt(&owner, &value, false, &proof())
                    .await,
                Err(StoreError::Conflict)
            ),
            "reordered open must not override terminal authority"
        );
    }
}
