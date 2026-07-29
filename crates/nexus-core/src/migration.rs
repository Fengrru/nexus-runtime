use crate::event::{EventType, NexusEvent};
use crate::state_machine::transition;
use crate::types::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossNodeSession {
    pub session_id: String,
    pub source_node: String,
    pub target_node: String,
    pub state: NexusState,
    pub causal_vector: CausalVector,
    pub events: Vec<ExportEventStub>,
    pub memory_graph: MemoryGraph,
    pub migration_id: String,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub status: MigrationStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MigrationStatus {
    Preparing,
    InFlight,
    Received,
    Validated,
    Committed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportEventStub {
    pub event_id: String,
    pub causal_vector_canonical: String,
    pub event_type_json: String,
    pub timestamp: u64,
}

impl CrossNodeSession {
    pub fn new(
        session_id: SessionId,
        source_node: String,
        target_node: String,
        state: NexusState,
        events: &[NexusEvent],
    ) -> Self {
        let event_stubs: Vec<ExportEventStub> = events
            .iter()
            .map(|e| ExportEventStub {
                event_id: e.event_id.clone(),
                causal_vector_canonical: e.causal_vector.to_canonical(),
                event_type_json: serde_json::to_string(&e.event_type).unwrap_or_default(),
                timestamp: e.event_timestamp,
            })
            .collect();

        let memory_graph = state.memory_graph.clone();
        let mut causal_vector = state.causal_vector.clone();
        for event in events {
            causal_vector.merge(&event.causal_vector);
        }

        Self {
            session_id: session_id.to_hex(),
            source_node: source_node.clone(),
            target_node: target_node.clone(),
            causal_vector,
            state,
            events: event_stubs,
            memory_graph,
            migration_id: uuid::Uuid::new_v4().to_string(),
            started_at: now_millis(),
            completed_at: None,
            status: MigrationStatus::Preparing,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.session_id.is_empty() {
            return Err("empty session ID".into());
        }
        if self.source_node == self.target_node {
            return Err(format!(
                "source and target node are the same: {}",
                self.source_node
            ));
        }
        if self.events.is_empty() {
            return Err("no events in migration".into());
        }
        if !self.causal_vector.is_consistent() {
            return Err("causal vector inconsistent".into());
        }
        Ok(())
    }

    pub fn mark_in_flight(&mut self) {
        self.status = MigrationStatus::InFlight;
    }

    pub fn mark_received(&mut self) {
        self.status = MigrationStatus::Received;
    }

    pub fn mark_validated(&mut self) {
        self.status = MigrationStatus::Validated;
    }

    pub fn commit(&mut self) {
        self.status = MigrationStatus::Committed;
        self.completed_at = Some(now_millis());
    }

    pub fn fail(&mut self, reason: String) {
        self.status = MigrationStatus::Failed;
        tracing::error!(
            target = "nexus.migration",
            migration_id = %self.migration_id,
            reason = %reason,
            "Migration failed"
        );
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.completed_at.map(|c| c.saturating_sub(self.started_at))
    }

    pub fn to_binary(&self) -> Result<Vec<u8>, String> {
        rmp_serde::to_vec(self).map_err(|e| format!("binary serialization error: {}", e))
    }

    pub fn from_binary(data: &[u8]) -> Result<Self, String> {
        rmp_serde::from_slice(data).map_err(|e| format!("binary deserialization error: {}", e))
    }
}

pub struct SessionMigrationManager {
    node_id: String,
    peer_nodes: Vec<String>,
}

impl SessionMigrationManager {
    pub fn new(node_id: String, peer_nodes: Vec<String>) -> Self {
        Self {
            node_id,
            peer_nodes,
        }
    }

    pub fn prepare_migration(
        &self,
        session_id: SessionId,
        target_node: String,
        state: NexusState,
        events: &[NexusEvent],
    ) -> Result<CrossNodeSession, String> {
        if !self.peer_nodes.contains(&target_node) {
            return Err(format!("target node {} is not a known peer", target_node));
        }

        let target_for_log = target_node.clone();

        let session =
            CrossNodeSession::new(session_id, self.node_id.clone(), target_node, state, events);

        session.validate()?;

        tracing::info!(
            target = "nexus.migration",
            migration_id = %session.migration_id,
            session_id = %session.session_id,
            source = %self.node_id,
            target = %target_for_log,
            events = %session.events.len(),
            "Migration prepared"
        );

        Ok(session)
    }

    pub async fn receive_migration(
        &self,
        mut session: CrossNodeSession,
    ) -> Result<CrossNodeSession, String> {
        session.mark_received();

        if session.target_node != self.node_id {
            return Err(format!(
                "migration target {} does not match this node {}",
                session.target_node, self.node_id
            ));
        }

        session.validate()?;

        let session_id = SessionId::from_hex(&session.session_id)
            .map_err(|e| format!("invalid session ID: {}", e))?;

        let mut replayed_state = NexusState::new(session_id, session.started_at);
        let dag = BTreeMap::new();

        for stub in &session.events {
            let event_type: EventType = serde_json::from_str(&stub.event_type_json)
                .map_err(|e| format!("event_type parse: {}", e))?;
            let cv = CausalVector::from_canonical(&stub.causal_vector_canonical)
                .map_err(|e| format!("causal_vector parse: {}", e))?;

            let event = NexusEvent {
                event_id: stub.event_id.clone(),
                event_type,
                session_id,
                trace_id: generate_trace_id(),
                parent_event_id: None,
                causal_vector: cv,
                payload: vec![],
                payload_hash: String::new(),
                event_timestamp: stub.timestamp,
                nonce: generate_nonce(),
                integrity_hash: String::new(),
            };

            replayed_state = transition(&replayed_state, &event, &dag)
                .map_err(|e| format!("replay error at {}: {:?}", stub.event_id, e))?;
        }

        if replayed_state.version != session.state.version {
            return Err(format!(
                "state divergence: replayed v{} != expected v{}",
                replayed_state.version, session.state.version
            ));
        }

        session.mark_validated();
        session.state = replayed_state;
        session.state.memory_graph = session.memory_graph.clone();

        Ok(session)
    }

    pub async fn commit_migration(
        &self,
        mut session: CrossNodeSession,
    ) -> Result<CrossNodeSession, String> {
        if session.status != MigrationStatus::Validated {
            return Err(format!(
                "cannot commit migration in state {:?}",
                session.status
            ));
        }

        let session_id = SessionId::from_hex(&session.session_id)
            .map_err(|e| format!("invalid session ID: {}", e))?;

        let mut cv = session.state.causal_vector.clone();
        cv.increment(session_id);

        let _event = NexusEvent::new(
            EventType::SessionMigrated {
                from: SessionId::from_hex(&session.source_node).unwrap_or_default(),
                to: SessionId::from_hex(&session.target_node).unwrap_or_default(),
                export_hash: session.migration_id.clone(),
            },
            session_id,
            cv,
            None,
        );

        session.commit();

        tracing::info!(
            target = "nexus.migration",
            migration_id = %session.migration_id,
            duration_ms = %session.duration_ms().unwrap_or(0),
            status = ?session.status,
            "Migration committed"
        );

        Ok(session)
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    pub fn peer_count(&self) -> usize {
        self.peer_nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migration_prepare_and_validate() {
        let manager =
            SessionMigrationManager::new("node-1".into(), vec!["node-2".into(), "node-3".into()]);

        let sid = SessionId::from_bytes([1u8; 16]);
        let state = NexusState::new(sid, now_millis());

        let _cv = CausalVector::new();
        let event = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "migration test".into(),
                source: "cli".into(),
            },
            sid,
            {
                let mut c = CausalVector::new();
                c.increment(sid);
                c
            },
            None,
        );

        let session = manager
            .prepare_migration(sid, "node-2".into(), state, &[event])
            .unwrap();

        assert_eq!(session.source_node, "node-1");
        assert_eq!(session.target_node, "node-2");
        assert_eq!(session.status, MigrationStatus::Preparing);

        let binary = session.to_binary().unwrap();
        let restored = CrossNodeSession::from_binary(&binary).unwrap();
        assert_eq!(restored.session_id, session.session_id);
    }

    #[test]
    fn test_migration_rejects_same_node() {
        let manager = SessionMigrationManager::new("node-1".into(), vec!["node-1".into()]);

        let sid = SessionId::from_bytes([1u8; 16]);
        let state = NexusState::new(sid, now_millis());

        let result = manager.prepare_migration(sid, "node-1".into(), state, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_migration_rejects_unknown_target() {
        let manager = SessionMigrationManager::new("node-1".into(), vec!["node-2".into()]);

        let sid = SessionId::from_bytes([1u8; 16]);
        let state = NexusState::new(sid, now_millis());

        let result = manager.prepare_migration(sid, "node-3".into(), state, &[]);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_receive_and_commit_migration() {
        let sid = SessionId::from_bytes([1u8; 16]);
        let mut state = NexusState::new(sid, now_millis());

        let mut cv1 = CausalVector::new();
        cv1.increment(sid);

        let event = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "receive test".into(),
                source: "test".into(),
            },
            sid,
            cv1,
            None,
        );

        let dag = BTreeMap::new();
        state = transition(&state, &event, &dag).unwrap();

        let source_manager = SessionMigrationManager::new("node-1".into(), vec!["node-2".into()]);

        let session = source_manager
            .prepare_migration(sid, "node-2".into(), state, &[event])
            .unwrap();

        let target_manager = SessionMigrationManager::new("node-2".into(), vec!["node-1".into()]);

        let received = target_manager.receive_migration(session).await.unwrap();
        assert_eq!(received.status, MigrationStatus::Validated);
        assert_eq!(received.state.status, SessionStatus::Intake);

        let committed = target_manager.commit_migration(received).await.unwrap();
        assert_eq!(committed.status, MigrationStatus::Committed);
        assert!(committed.duration_ms().is_some());
    }
}
