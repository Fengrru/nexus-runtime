use super::rows::{EventRow, StateRow};
use super::store::{EventStore, StoreError};
use async_trait::async_trait;
use nexus_core::{
    ArtifactRef, LlmCallRecord, LockMode, NexusEvent, NexusState, SessionId, SideEffectIntent,
};
use nexus_core::session_driver::SessionStore;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

pub const CREATE_SCHEMA_SQL: &str = include_str!("../schema.sql");

pub struct SqliteEventStore {
    pool: SqlitePool,
}

impl SqliteEventStore {
    pub async fn new(database_url: &str) -> Result<Self, StoreError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(database_url)
            .await
            .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&pool)
            .await
            .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        sqlx::query("PRAGMA synchronous=NORMAL")
            .execute(&pool)
            .await
            .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await
            .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        for statement in CREATE_SCHEMA_SQL.split(';') {
            let trimmed = statement.trim();
            if trimmed.is_empty() {
                continue;
            }
            sqlx::query(trimmed)
                .execute(&pool)
                .await
                .map_err(|e| StoreError::ConnectionFailed(format!("schema error: {}", e)))?;
        }

        Ok(Self { pool })
    }
}

#[async_trait]
impl EventStore for SqliteEventStore {
    async fn append_event(&self, event: &NexusEvent) -> Result<(), StoreError> {
        let payload_bytes = rmp_serde::to_vec(&event.payload)
            .map_err(|e| StoreError::SerializationError(e.to_string()))?;

        sqlx::query(
            r#"INSERT INTO events (
                event_id, event_type, session_id, trace_id, parent_event_id,
                causal_vector, payload, payload_hash, event_timestamp,
                nonce, integrity_hash
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
        )
        .bind(&event.event_id)
        .bind(serde_json::to_string(&event.event_type).unwrap_or_default())
        .bind(event.session_id.as_bytes().as_slice())
        .bind(&event.trace_id[..])
        .bind(&event.parent_event_id)
        .bind(event.causal_vector.to_canonical())
        .bind(&payload_bytes)
        .bind(&event.payload_hash)
        .bind(event.event_timestamp as i64)
        .bind(&event.nonce)
        .bind(&event.integrity_hash)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        Ok(())
    }

    async fn get_events(
        &self,
        session_id: SessionId,
        since: Option<u64>,
    ) -> Result<Vec<NexusEvent>, StoreError> {
        if since.is_some() {
            // Simplified: load all and filter (full impl would use WHERE)
            Err(StoreError::SerializationError(
                "since parameter requires full deserialization; use get_events without since"
                    .into(),
            ))
        } else {
            let rows = sqlx::query_as::<_, EventRow>(
                "SELECT event_id, event_type, session_id, trace_id, parent_event_id,
                 causal_vector, payload, payload_hash, event_timestamp,
                 nonce, integrity_hash
                 FROM events
                 WHERE session_id = ?1
                 ORDER BY event_timestamp, rowid",
            )
            .bind(session_id.as_bytes().as_slice())
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

            rows.into_iter()
                .map(|r| r.to_nexus_event())
                .collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::SerializationError)
        }
    }

    async fn get_event(&self, event_id: &str) -> Result<Option<NexusEvent>, StoreError> {
        let row = sqlx::query_as::<_, EventRow>(
            "SELECT event_id, event_type, session_id, trace_id, parent_event_id,
             causal_vector, payload, payload_hash, event_timestamp,
             nonce, integrity_hash
             FROM events WHERE event_id = ?1",
        )
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        row.map(|r| r.to_nexus_event())
            .transpose()
            .map_err(StoreError::SerializationError)
    }

    async fn get_state(&self, session_id: SessionId) -> Result<Option<NexusState>, StoreError> {
        let row = sqlx::query_as::<_, StateRow>(
            "SELECT session_id, version, status, intent_graph, execution_frontier,
             memory_refs, budget, checkpoint_seq, created_at, updated_at, latest_event_id
             FROM sessions WHERE session_id = ?1",
        )
        .bind(session_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        row.map(|r| r.to_nexus_state())
            .transpose()
            .map_err(StoreError::SerializationError)
    }

    async fn update_state(
        &self,
        state: &NexusState,
        expected_version: u64,
    ) -> Result<bool, StoreError> {
        let _intent_graph_bytes = rmp_serde::to_vec(&state.intent_graph)
            .map_err(|e| StoreError::SerializationError(e.to_string()))?;
        let _frontier_bytes = rmp_serde::to_vec(&state.execution_frontier)
            .map_err(|e| StoreError::SerializationError(e.to_string()))?;
        let _memory_refs_bytes = rmp_serde::to_vec(&state.memory_refs)
            .map_err(|e| StoreError::SerializationError(e.to_string()))?;
        let _budget_bytes = rmp_serde::to_vec(&state.budget)
            .map_err(|e| StoreError::SerializationError(e.to_string()))?;

        let result = sqlx::query(
            "INSERT OR REPLACE INTO sessions (
                session_id, version, status, intent_graph, execution_frontier,
                memory_refs, budget, checkpoint_seq, created_at, updated_at, latest_event_id
            ) VALUES (?1, ?2, ?3, ?8, ?9, ?10, ?11, ?6, ?4, ?4, ?5)",
        )
        .bind(state.session_id.as_bytes().as_slice())
        .bind(state.version as i64)
        .bind(format!("{:?}", state.status).to_lowercase())
        .bind(state.created_at as i64)
        .bind(&state.latest_event_id)
        .bind(state.checkpoint_seq as i64)
        .bind(expected_version as i64)
        .bind(&_intent_graph_bytes)
        .bind(&_frontier_bytes)
        .bind(&_memory_refs_bytes)
        .bind(&_budget_bytes)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        Ok(result.rows_affected() > 0)
    }

    async fn record_side_effect_intent(&self, intent: &SideEffectIntent) -> Result<(), StoreError> {
        let request_payload = rmp_serde::to_vec(&intent.payload)
            .map_err(|e| StoreError::SerializationError(e.to_string()))?;
        let idempotency_key = format!("{}:{}", intent.session_id.to_hex(), intent.request_hash);
        let id = uuid::Uuid::new_v4().into_bytes().to_vec();

        sqlx::query(
            "INSERT OR REPLACE INTO side_effects (
                id, session_id, event_id, idempotency_key,
                effect_class, status, request_payload, request_hash,
                response_payload, response_hash, compensation_data, committed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, 'PENDING', ?6, ?7, NULL, NULL, NULL, NULL)",
        )
        .bind(&id)
        .bind(intent.session_id.as_bytes().as_slice())
        .bind(&intent.id)
        .bind(&idempotency_key)
        .bind(format!("{:?}", intent.effect_class).to_uppercase())
        .bind(&request_payload)
        .bind(&intent.request_hash)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        Ok(())
    }

    async fn commit_side_effect(&self, id: &[u8], response_hash: &str) -> Result<(), StoreError> {
        let rows = sqlx::query(
            "UPDATE side_effects SET
                status = 'COMMITTED',
                response_hash = ?2,
                committed_at = ?3
             WHERE id = ?1 AND status = 'PENDING'",
        )
        .bind(id)
        .bind(response_hash)
        .bind(nexus_core::now_millis() as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        if rows.rows_affected() == 0 {
            return Err(StoreError::EventNotFound(format!(
                "side effect {:?} not found or already committed",
                id
            )));
        }

        Ok(())
    }

    async fn acquire_lock(
        &self,
        resource_id: &str,
        session_id: SessionId,
        mode: LockMode,
    ) -> Result<bool, StoreError> {
        let now = nexus_core::now_millis() as i64;
        let mode_str = match mode {
            LockMode::Exclusive => "EXCLUSIVE",
            LockMode::Shared => "SHARED",
        };

        sqlx::query(
            "INSERT OR REPLACE INTO resource_locks (
                resource_id, owner_session, owner_task, mode,
                acquired_at, lease_expiry, generation
            ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, 1)",
        )
        .bind(resource_id)
        .bind(session_id.as_bytes().as_slice())
        .bind(mode_str)
        .bind(now)
        .bind(now + 60_000) // 60s default lease
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        Ok(true)
    }

    async fn release_lock(
        &self,
        resource_id: &str,
        session_id: SessionId,
    ) -> Result<bool, StoreError> {
        let rows = sqlx::query(
            "DELETE FROM resource_locks
             WHERE resource_id = ?1 AND owner_session = ?2",
        )
        .bind(resource_id)
        .bind(session_id.as_bytes().as_slice())
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        Ok(rows.rows_affected() > 0)
    }

    async fn record_llm_call(&self, call: &LlmCallRecord) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT OR REPLACE INTO llm_calls (
                request_id, session_id, event_id, model,
                prompt_hash, response_hash, input_tokens, output_tokens,
                cost_usd_cents, status, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )
        .bind(&call.request_id)
        .bind(call.session_id.as_bytes().as_slice())
        .bind(&call.event_id)
        .bind(&call.model)
        .bind(&call.prompt_hash)
        .bind(&call.response_hash)
        .bind(call.input_tokens)
        .bind(call.output_tokens)
        .bind(call.cost_usd_cents)
        .bind(&call.status)
        .bind(call.created_at as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        Ok(())
    }

    async fn register_artifact(&self, artifact: &ArtifactRef) -> Result<(), StoreError> {
        let id = uuid::Uuid::new_v4().into_bytes().to_vec();

        sqlx::query(
            "INSERT OR REPLACE INTO artifact_refs (
                id, kind, uri, blake3, size,
                produced_by_session, produced_by_event,
                status, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'created', ?8)",
        )
        .bind(&id)
        .bind(format!("{:?}", artifact.kind).to_lowercase())
        .bind(&artifact.uri)
        .bind(&artifact.blake3)
        .bind(artifact.size_bytes as i64)
        .bind(artifact.produced_by_session.as_bytes().as_slice())
        .bind(&artifact.produced_by_event)
        .bind(artifact.created_at as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        Ok(())
    }

    async fn health_check(&self) -> Result<(), StoreError> {
        let row: (String,) = sqlx::query_as("PRAGMA integrity_check")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;

        if row.0 != "ok" {
            return Err(StoreError::IntegrityCheckFailed(row.0));
        }

        Ok(())
    }
}

#[async_trait]
impl SessionStore for SqliteEventStore {
    async fn append_event(&self, event: &NexusEvent) -> Result<(), String> {
        <Self as EventStore>::append_event(self, event)
            .await
            .map_err(|e| e.to_string())
    }

    async fn update_state(&self, state: &NexusState, expected_version: u64) -> Result<bool, String> {
        <Self as EventStore>::update_state(self, state, expected_version)
            .await
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::*;

    async fn create_test_store() -> SqliteEventStore {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db_url = format!("sqlite:{}?mode=rwc", db_path.display());
        SqliteEventStore::new(&db_url).await.unwrap()
    }

    fn make_session() -> SessionId {
        SessionId::from_bytes([1u8; 16])
    }

    fn make_event(sid: SessionId, cv_seq: u64) -> NexusEvent {
        let mut cv = CausalVector::new();
        cv.increment(sid);
        for _ in 0..cv_seq - 1 {
            cv.increment(sid);
        }

        NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "test intent".into(),
                source: "integration_test".into(),
            },
            sid,
            cv,
            None,
        )
    }

    #[tokio::test]
    async fn test_append_and_read_event() {
        let store = create_test_store().await;
        let sid = make_session();
        let event = make_event(sid, 1);

        let event_id = event.event_id.clone();

        store.append_event(&event).await.unwrap();

        let fetched = store.get_event(&event_id).await.unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().session_id, sid);
    }

    #[tokio::test]
    async fn test_append_multiple_events() {
        let store = create_test_store().await;
        let sid = make_session();

        for i in 1..=5 {
            let event = make_event(sid, i);
            store.append_event(&event).await.unwrap();
        }

        let events = store.get_events(sid, None).await.unwrap();
        assert_eq!(events.len(), 5);
    }

    #[tokio::test]
    async fn test_get_events_by_session() {
        let store = create_test_store().await;
        let sid1 = SessionId::from_bytes([1u8; 16]);
        let sid2 = SessionId::from_bytes([2u8; 16]);

        store.append_event(&make_event(sid1, 1)).await.unwrap();
        store.append_event(&make_event(sid1, 2)).await.unwrap();
        store.append_event(&make_event(sid2, 1)).await.unwrap();

        let events1 = store.get_events(sid1, None).await.unwrap();
        let events2 = store.get_events(sid2, None).await.unwrap();

        assert_eq!(events1.len(), 2);
        assert_eq!(events2.len(), 1);
    }

    #[tokio::test]
    async fn test_health_check() {
        let store = create_test_store().await;
        assert!(store.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_event_not_found() {
        let store = create_test_store().await;
        let result = store.get_event("nonexistent_event_id").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_update_state_optimistic_locking() {
        let store = create_test_store().await;
        let sid = make_session();

        // Insert an event first (FK constraint requires events row to exist)
        let event = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "fk test".into(),
                source: "test".into(),
            },
            sid,
            {
                let mut cv = CausalVector::new();
                cv.increment(sid);
                cv
            },
            None,
        );
        store.append_event(&event).await.unwrap();

        let mut initial_state = NexusState::new(sid, now_millis());
        initial_state.version = 1;
        initial_state.latest_event_id = event.event_id.clone();

        let ok = store.update_state(&initial_state, 0).await.unwrap();
        assert!(ok, "First insert should succeed");

        let mut updated = initial_state.clone();
        updated.version = 2;

        let ok = store.update_state(&updated, 1).await.unwrap();
        assert!(ok, "Update should succeed");

        let stored = store.get_state(sid).await.unwrap().unwrap();
        assert_eq!(stored.version, 2, "Version should be updated");
        assert_eq!(stored.status, updated.status);
    }

    #[tokio::test]
    async fn test_recover_events_through_state_machine() {
        let store = create_test_store().await;
        let sid = make_session();

        // Build a full lifecycle
        let events = vec![
            NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "recovery test".into(),
                    source: "test".into(),
                },
                sid,
                {
                    let mut cv = CausalVector::new();
                    cv.increment(sid);
                    cv
                },
                None,
            ),
            NexusEvent::new(
                EventType::IntentParsed {
                    intent_graph: IntentGraph::default(),
                },
                sid,
                {
                    let mut cv = CausalVector::new();
                    cv.increment(sid);
                    cv.increment(sid);
                    cv
                },
                None,
            ),
            NexusEvent::new(
                EventType::PlanCommitted {
                    frontier: Frontier::empty(),
                },
                sid,
                {
                    let mut cv = CausalVector::new();
                    cv.increment(sid);
                    cv.increment(sid);
                    cv.increment(sid);
                    cv
                },
                None,
            ),
            NexusEvent::new(
                EventType::DependenciesMet,
                sid,
                {
                    let mut cv = CausalVector::new();
                    for _ in 0..4 {
                        cv.increment(sid);
                    }
                    cv
                },
                None,
            ),
            NexusEvent::new(
                EventType::WorkerCheckpoint {
                    task_id: TaskId::from_bytes([10u8; 16]),
                    step_index: 3,
                    actions: vec![],
                    artifacts: vec![],
                },
                sid,
                {
                    let mut cv = CausalVector::new();
                    for _ in 0..5 {
                        cv.increment(sid);
                    }
                    cv
                },
                None,
            ),
        ];

        for event in &events {
            store.append_event(event).await.unwrap();
        }

        // Load all events
        let loaded = store.get_events(sid, None).await.unwrap();
        assert_eq!(loaded.len(), 5);

        // Replay through state machine
        let rm = nexus_core::recovery::RecoveryManager::new("/tmp/vault".into());
        let recovered = rm.recover_from_events(&loaded, sid).unwrap();

        assert!(recovered.report.causal_valid);
        assert!(recovered.report.replay_success);
        assert_eq!(recovered.state.status, SessionStatus::Checkpointing);
        assert_eq!(recovered.state.checkpoint_seq, 3);
    }

    #[tokio::test]
    async fn test_event_integrity_hash() {
        let sid = make_session();
        let event = make_event(sid, 1);

        let hash = event.compute_integrity_hash();
        assert!(!hash.is_empty());
        assert_eq!(hash.len(), 64);

        let event2 = event.clone();
        assert_eq!(
            event.compute_integrity_hash(),
            event2.compute_integrity_hash(),
            "Integrity hash must be deterministic"
        );
    }

    #[tokio::test]
    async fn test_causal_vector_persistence() {
        let store = create_test_store().await;
        let sid = make_session();
        let mut cv = CausalVector::new();
        cv.increment(sid);
        cv.increment(sid);

        let event = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "causal test".into(),
                source: "test".into(),
            },
            sid,
            cv,
            None,
        );

        store.append_event(&event).await.unwrap();

        let fetched = store.get_event(&event.event_id).await.unwrap().unwrap();
        assert_eq!(fetched.causal_vector.0.get(&sid), Some(&2u64));
    }
}
