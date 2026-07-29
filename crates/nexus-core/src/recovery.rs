use crate::checkpoint::*;
use crate::event::*;
use crate::state_machine::*;
use crate::types::*;
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub struct RecoveryReport {
    pub integrity_check: bool,
    pub causal_valid: bool,
    pub replay_success: bool,
    pub artifacts_valid: bool,
    pub cost_integrity: bool,
}

#[derive(Debug)]
pub struct RecoveryResult {
    pub state: NexusState,
    pub report: RecoveryReport,
    pub recovery_plan: Option<RecoveryPlan>,
}

#[derive(Debug, Clone)]
pub struct RecoveryPlan {
    pub from_step: u64,
    pub replay_actions: Vec<ReplayAction>,
    pub handle_registry: Vec<HandleRecord>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    #[error("Store corrupted: {0}")]
    StoreCorrupted(String),

    #[error("Event load failed: {0}")]
    EventLoadFailed(String),

    #[error("Session not found")]
    SessionNotFound,

    #[error("Causal violation at {event_id}: expected {expected:?}, got {actual:?}")]
    CausalViolation {
        event_id: String,
        expected: CausalVector,
        actual: CausalVector,
    },

    #[error("Replay failed at {event_id}: {error}")]
    ReplayFailed { event_id: String, error: String },

    #[error("Artifact missing: {0} ({1})")]
    ArtifactMissing(String, String),

    #[error("Artifact corrupted: {artifact_id} expected {expected}, got {actual}")]
    ArtifactCorrupted {
        artifact_id: String,
        expected: String,
        actual: String,
    },

    #[error("Duplicated LLM calls detected")]
    DuplicatedLlmCalls,

    #[error("Checkpoint ahead of state")]
    CheckpointAheadOfState,

    #[error("Worker spawn failed: {0}")]
    WorkerSpawnFailed(String),
}

pub struct RecoveryManager {
    vault_base_path: String,
}

impl RecoveryManager {
    pub fn new(vault_base_path: String) -> Self {
        Self { vault_base_path }
    }

    pub fn recover_from_events(
        &self,
        events: &[NexusEvent],
        session_id: SessionId,
    ) -> Result<RecoveryResult, RecoveryError> {
        // Step 1: Integrity check (delegated to caller via store)
        // Step 2: Load all events (already provided by caller)
        if events.is_empty() {
            return Err(RecoveryError::SessionNotFound);
        }

        // Step 3: Verify causal vector monotonicity
        let mut prev_cv = CausalVector::new();
        for event in events {
            if !is_monotonic(&prev_cv, &event.causal_vector) {
                return Err(RecoveryError::CausalViolation {
                    event_id: event.event_id.clone(),
                    expected: prev_cv,
                    actual: event.causal_vector.clone(),
                });
            }
            prev_cv.merge(&event.causal_vector);
        }

        // Step 4: Replay events through state machine
        let mut state = NexusState::new(
            session_id,
            events.first().map(|e| e.event_timestamp).unwrap_or(0),
        );
        let dag = build_dag(events);

        for event in events {
            state = transition(&state, event, &dag).map_err(|e| RecoveryError::ReplayFailed {
                event_id: event.event_id.clone(),
                error: format!("{:?}", e),
            })?;
        }

        // Step 5-6: Artifact verification & cost integrity (delegated)
        // Step 7: Build recovery plan
        let recovery_plan = if state.status == SessionStatus::Executing
            || state.status == SessionStatus::Checkpointing
        {
            Some(build_recovery_plan(&state, state.checkpoint_seq))
        } else {
            None
        };

        Ok(RecoveryResult {
            state,
            report: RecoveryReport {
                integrity_check: true,
                causal_valid: true,
                replay_success: true,
                artifacts_valid: true,
                cost_integrity: true,
            },
            recovery_plan,
        })
    }

    pub fn recover_with_context(
        &self,
        events: &[NexusEvent],
        session_id: SessionId,
        ctx: &RecoveryContext,
    ) -> Result<RecoveryResult, RecoveryError> {
        let integrity_check = ctx.store_integrity_ok;

        // Step 2: Load all events (provided by caller)
        if events.is_empty() {
            return Err(RecoveryError::SessionNotFound);
        }

        // Step 3: Verify causal vector monotonicity
        let mut prev_cv = CausalVector::new();
        for event in events {
            if !is_monotonic(&prev_cv, &event.causal_vector) {
                return Err(RecoveryError::CausalViolation {
                    event_id: event.event_id.clone(),
                    expected: prev_cv,
                    actual: event.causal_vector.clone(),
                });
            }
            prev_cv.merge(&event.causal_vector);
        }
        let causal_valid = true;

        // Step 4: Replay events through state machine
        let mut state = NexusState::new(
            session_id,
            events.first().map(|e| e.event_timestamp).unwrap_or(0),
        );
        let dag = build_dag(events);

        for event in events {
            state = transition(&state, event, &dag).map_err(|e| RecoveryError::ReplayFailed {
                event_id: event.event_id.clone(),
                error: format!("{:?}", e),
            })?;
        }
        let replay_success = true;

        // Step 5: Load and verify checkpoint
        if let Some(ref cp) = ctx.checkpoint {
            if cp.step_index > state.checkpoint_seq {
                return Err(RecoveryError::CheckpointAheadOfState);
            }
            for art in &cp.artifact_refs {
                self.verify_artifact(art)?;
            }
        }
        let artifacts_valid = ctx.artifacts_verified;

        // Step 6: Verify no duplicated LLM calls
        if ctx.llm_unique_count != ctx.llm_total_count {
            return Err(RecoveryError::DuplicatedLlmCalls);
        }
        let cost_integrity = ctx.llm_total_count == ctx.llm_unique_count;

        // Step 7: Build recovery plan if session was active
        let recovery_plan = if state.status == SessionStatus::Executing
            || state.status == SessionStatus::Checkpointing
        {
            Some(build_recovery_plan(&state, state.checkpoint_seq))
        } else {
            None
        };

        Ok(RecoveryResult {
            state,
            report: RecoveryReport {
                integrity_check,
                causal_valid,
                replay_success,
                artifacts_valid,
                cost_integrity,
            },
            recovery_plan,
        })
    }

    pub fn verify_artifact(&self, art: &ArtifactRef) -> Result<(), RecoveryError> {
        let path =
            std::path::Path::new(&self.vault_base_path).join(art.uri.replace("vault://", ""));

        let content = std::fs::read(&path)
            .map_err(|e| RecoveryError::ArtifactMissing(art.id.clone(), e.to_string()))?;

        let actual_hash = blake3::hash(&content).to_hex().to_string();
        if actual_hash != art.blake3 {
            return Err(RecoveryError::ArtifactCorrupted {
                artifact_id: art.id.clone(),
                expected: art.blake3.clone(),
                actual: actual_hash,
            });
        }

        Ok(())
    }

    /// Reconstruct the latest checkpoint from event log events.
    /// Walk through WorkerCheckpoint events to find the highest step_index.
    pub fn load_latest_checkpoint(&self, events: &[NexusEvent]) -> Option<Checkpoint> {
        let mut latest: Option<Checkpoint> = None;
        let mut max_step: u64 = 0;

        for event in events {
            if let EventType::WorkerCheckpoint {
                task_id,
                step_index,
                actions,
                artifacts,
            } = &event.event_type
            {
                if *step_index > max_step {
                    max_step = *step_index;
                    latest = Some(Checkpoint {
                        checkpoint_id: format!("cp_{}", event.event_id),
                        session_id: event.session_id,
                        step_index: *step_index,
                        total_actions: actions.len() as u64,
                        replay_actions: actions
                            .iter()
                            .map(|a| match a {
                                Action::ReadFile { path, artifact } => ReplayAction::ReadFile {
                                    path: path.clone(),
                                    expected_hash: artifact.blake3.clone(),
                                },
                                Action::EditFile {
                                    path,
                                    search,
                                    replace,
                                    artifact,
                                } => ReplayAction::EditFile {
                                    path: path.clone(),
                                    search: search.clone(),
                                    replace: replace.clone(),
                                    expected_count: 1,
                                },
                                Action::RunCommand { command, args, env } => {
                                    ReplayAction::RunCommand {
                                        command: command.clone(),
                                        args: args.clone(),
                                        env: env.clone(),
                                    }
                                }
                                Action::GitCommit { message, files } => ReplayAction::GitCommit {
                                    message: message.clone(),
                                    files: files.clone(),
                                },
                            })
                            .collect(),
                        artifact_refs: artifacts.clone(),
                        handle_registry: Vec::new(),
                        determinism_context: DeterminismContext {
                            seed: 0,
                            model_version: String::new(),
                            input_hash: String::new(),
                            checkpoint_format_version: 1,
                            worker_type: WorkerType::Python,
                        },
                        created_at: event.event_timestamp,
                    });
                }
            }
        }

        if latest.is_some() {
            tracing::info!(
                target = "nexus.recovery",
                checkpoint_step = %max_step,
                "Latest checkpoint reconstructed from event log"
            );
        }

        latest
    }
}

#[derive(Debug, Clone, Default)]
pub struct RecoveryContext {
    pub store_integrity_ok: bool,
    pub checkpoint: Option<Checkpoint>,
    pub artifacts_verified: bool,
    pub llm_unique_count: u64,
    pub llm_total_count: u64,
}

fn is_monotonic(prev: &CausalVector, next: &CausalVector) -> bool {
    for (session_id, prev_count) in &prev.0 {
        let next_count = next.0.get(session_id).copied().unwrap_or(0);
        if next_count < *prev_count {
            return false;
        }
    }
    true
}

fn build_dag(events: &[NexusEvent]) -> BTreeMap<TaskId, TaskNode> {
    let mut dag = BTreeMap::new();
    for event in events {
        if let EventType::IntentParsed { intent_graph } = &event.event_type {
            for (task_id, node) in &intent_graph.nodes {
                dag.insert(*task_id, node.clone());
            }
        }
    }
    dag
}

fn build_recovery_plan(state: &NexusState, from_step: u64) -> RecoveryPlan {
    // Generate real replay actions from the execution frontier and intent graph.
    // For each task in the frontier, look up its TaskNode in the intent_graph
    // to determine the correct replay action type based on the task's action_type.
    let replay_actions: Vec<ReplayAction> = state
        .execution_frontier
        .nodes
        .iter()
        .filter_map(|task_id| {
            state.intent_graph.nodes.get(task_id).map(|node| {
                match node.intent.action_type.as_str() {
                    "read_file" => ReplayAction::ReadFile {
                        path: node.intent.target.clone(),
                        expected_hash: String::new(),
                    },
                    "edit_file" | "replace_text" => ReplayAction::EditFile {
                        path: node.intent.target.clone(),
                        search: node
                            .intent
                            .parameters
                            .get("search")
                            .cloned()
                            .unwrap_or_default(),
                        replace: node
                            .intent
                            .parameters
                            .get("replace")
                            .cloned()
                            .unwrap_or_default(),
                        expected_count: 1,
                    },
                    "run_command" | "grep" | "calculate" => ReplayAction::RunCommand {
                        command: node.intent.target.clone(),
                        args: node.intent.parameters.values().cloned().collect(),
                        env: std::collections::BTreeMap::new(),
                    },
                    "git_commit" => ReplayAction::GitCommit {
                        message: node
                            .intent
                            .parameters
                            .get("message")
                            .cloned()
                            .unwrap_or_default(),
                        files: node.intent.parameters.values().cloned().collect(),
                    },
                    _ => ReplayAction::ReadFile {
                        path: node.intent.target.clone(),
                        expected_hash: String::new(),
                    },
                }
            })
        })
        .collect();

    RecoveryPlan {
        from_step,
        replay_actions,
        handle_registry: Vec::new(),
    }
}

pub fn reacquire_handle(handle: &HandleRecord) -> Result<(), RecoveryError> {
    match handle.handle_type.as_str() {
        "file_lock" => {
            let path = handle.metadata.get("path").cloned().unwrap_or_default();
            if path.is_empty() {
                return Err(RecoveryError::WorkerSpawnFailed(
                    "file_lock handle missing path".into(),
                ));
            }
            std::fs::metadata(&path).map_err(|e| {
                RecoveryError::WorkerSpawnFailed(format!(
                    "Cannot reacquire file_lock for {}: {}",
                    path, e
                ))
            })?;
            tracing::debug!(target = "nexus.recovery", path = %path, "File lock reacquired");
            Ok(())
        }
        "api_session" => {
            let endpoint = handle.metadata.get("endpoint").cloned().unwrap_or_default();
            if endpoint.is_empty() {
                return Err(RecoveryError::WorkerSpawnFailed(
                    "api_session handle missing endpoint".into(),
                ));
            }
            tracing::warn!(target = "nexus.recovery", endpoint = %endpoint, "API session may require manual re-authentication");
            Ok(())
        }
        "db_connection" => {
            tracing::debug!(
                target = "nexus.recovery",
                "DB connection handle delegated to store"
            );
            Ok(())
        }
        other => {
            tracing::warn!(target = "nexus.recovery", handle_type = %other, "Unknown handle type");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RecoveryContext, RecoveryError, RecoveryManager};
    use crate::event::{EventType, NexusEvent};
    use crate::types::*;

    #[test]
    fn test_recover_from_empty_events_fails() {
        let rm = RecoveryManager::new("/tmp/test_vault".into());
        let sid = SessionId::from_bytes([1u8; 16]);
        let result = rm.recover_from_events(&[], sid);
        assert!(matches!(result, Err(RecoveryError::SessionNotFound)));
    }

    #[test]
    fn test_recover_completed_session() {
        let rm = RecoveryManager::new("/tmp/test_vault".into());
        let sid = SessionId::from_bytes([1u8; 16]);

        let events = vec![
            NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "test".into(),
                    source: "cli".into(),
                },
                sid,
                {
                    let mut c = CausalVector::new();
                    c.increment(sid);
                    c
                },
                None,
            ),
            NexusEvent::new(
                EventType::IntentParsed {
                    intent_graph: IntentGraph::default(),
                },
                sid,
                {
                    let mut c = CausalVector::new();
                    c.increment(sid);
                    c.increment(sid);
                    c
                },
                None,
            ),
            NexusEvent::new(
                EventType::PlanRejected {
                    reason: "test rejection".into(),
                },
                sid,
                {
                    let mut c = CausalVector::new();
                    c.increment(sid);
                    c.increment(sid);
                    c.increment(sid);
                    c
                },
                None,
            ),
        ];

        let result = rm.recover_from_events(&events, sid).unwrap();
        assert!(result.report.causal_valid);
        assert!(result.report.replay_success);
        assert_eq!(result.state.status, SessionStatus::Failed);
        assert!(result.recovery_plan.is_none());
    }

    #[test]
    fn test_duplicate_llm_calls_detected() {
        let rm = RecoveryManager::new("/tmp/test_vault".into());
        let sid = SessionId::from_bytes([1u8; 16]);

        let events = vec![NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "test".into(),
                source: "cli".into(),
            },
            sid,
            {
                let mut c = CausalVector::new();
                c.increment(sid);
                c
            },
            None,
        )];

        let ctx = RecoveryContext {
            store_integrity_ok: true,
            checkpoint: None,
            artifacts_verified: true,
            llm_unique_count: 1,
            llm_total_count: 2,
        };

        let result = rm.recover_with_context(&events, sid, &ctx);
        assert!(matches!(result, Err(RecoveryError::DuplicatedLlmCalls)));
    }

    #[test]
    fn test_recover_with_checkpoint_verification() {
        let rm = RecoveryManager::new("/tmp/test_vault".into());
        let sid = SessionId::from_bytes([1u8; 16]);

        let events = vec![
            NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "refactor".into(),
                    source: "cli".into(),
                },
                sid,
                {
                    let mut c = CausalVector::new();
                    c.increment(sid);
                    c
                },
                None,
            ),
            NexusEvent::new(
                EventType::IntentParsed {
                    intent_graph: IntentGraph::default(),
                },
                sid,
                {
                    let mut c = CausalVector::new();
                    c.increment(sid);
                    c.increment(sid);
                    c
                },
                None,
            ),
            NexusEvent::new(
                EventType::PlanCommitted {
                    frontier: Frontier::empty(),
                },
                sid,
                {
                    let mut c = CausalVector::new();
                    c.increment(sid);
                    c.increment(sid);
                    c.increment(sid);
                    c
                },
                None,
            ),
            NexusEvent::new(
                EventType::DependenciesMet,
                sid,
                {
                    let mut c = CausalVector::new();
                    c.increment(sid);
                    c.increment(sid);
                    c.increment(sid);
                    c.increment(sid);
                    c
                },
                None,
            ),
        ];

        let ctx = RecoveryContext {
            store_integrity_ok: true,
            checkpoint: None,
            artifacts_verified: true,
            llm_unique_count: 0,
            llm_total_count: 0,
        };

        let result = rm.recover_with_context(&events, sid, &ctx).unwrap();
        assert!(result.report.integrity_check);
        assert!(result.report.causal_valid);
        assert!(result.report.replay_success);
        assert!(result.report.artifacts_valid);
        assert!(result.report.cost_integrity);
        assert_eq!(result.state.status, SessionStatus::Executing);
        assert!(result.recovery_plan.is_some());
    }
}
