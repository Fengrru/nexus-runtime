use crate::event::*;
use crate::types::*;
use std::collections::BTreeMap;

#[derive(Debug, Clone, thiserror::Error)]
pub enum TransitionError {
    #[error("Session mismatch: event for {event} applied to session {current}")]
    SessionMismatch { current: String, event: String },

    #[error("Causal violation: event vector not monotonic")]
    CausalViolation,

    #[error("Stale checkpoint: expected {expected}, received {received}")]
    StaleCheckpoint { expected: u64, received: u64 },

    #[error("Illegal transition from {from} via {event}")]
    IllegalTransition { from: String, event: String },

    #[error("Budget exceeded: {consumed}/{limit}")]
    BudgetExceeded { consumed: u64, limit: u64 },

    #[error("Capability denied: {capability}")]
    CapabilityDenied { capability: String },

    #[error("Worker not found: {worker_id}")]
    WorkerNotFound { worker_id: String },

    #[error("Timeout")]
    Timeout,
}

/// The core deterministic state machine — the heart of Nexus.
///
/// **This function is pure** — no async, no I/O, no clock, no random.
/// Given the same `(state, event, dag)`, it always produces the same output.
/// This property is verified by golden fixtures and Phoenix invariant tests.
///
/// # State Transitions
///
/// ```text
/// Created → Intake → Planning → Planned → Executing → Checkpointing → Executing → ...
///                                                    ↘ Converging → Reflecting → Completed
///                                                    ↘ Failed
///
/// Executing/Checkpointing/Planned + HumanApprovalRequested → Blocked → HumanApproved → Executing
/// Any state + SessionSuspended → Checkpointing → SessionResumed → Executing
/// Any state + SessionArchived → {final_status}
/// ```
///
/// # Arguments
///
/// * `current` - The current session state (immutable borrow).
/// * `event` - The event to apply, carrying a causal vector clock.
/// * `dag` - The task DAG, consulted for fan-in convergence detection.
///
/// # Returns
///
/// * `Ok(NexusState)` - The next state with incremented version and merged causal vector.
/// * `Err(TransitionError)` - If the transition violates any invariant.
///
/// # Errors
///
/// * [`TransitionError::SessionMismatch`] — Event targets a different session.
/// * [`TransitionError::CausalViolation`] — Event vector is not monotonic.
/// * [`TransitionError::StaleCheckpoint`] — Checkpoint sequence is behind current.
/// * [`TransitionError::IllegalTransition`] — Event not valid from current state.
/// * [`TransitionError::BudgetExceeded`] — Budget exhausted.
///
/// # Examples
///
/// ```
/// use nexus_core::{NexusState, NexusEvent, transition, SessionId, CausalVector, EventType};
/// use std::collections::BTreeMap;
///
/// let session_id = SessionId::from_bytes([1u8; 16]);
/// let state = NexusState::new(session_id, 0);
///
/// let mut cv = CausalVector::new();
/// cv.increment(session_id);
///
/// let event = NexusEvent::new(
///     EventType::IntentReceived {
///         raw_input: "analyze auth.js".into(),
///         source: "cli".into(),
///     },
///     session_id,
///     cv,
///     None,
/// );
///
/// let next = transition(&state, &event, &BTreeMap::new()).unwrap();
/// assert_eq!(next.status, nexus_core::SessionStatus::Intake);
/// ```
pub fn transition(
    current: &NexusState,
    event: &NexusEvent,
    dag: &BTreeMap<TaskId, TaskNode>,
) -> Result<NexusState, TransitionError> {
    if event.session_id != current.session_id {
        return Err(TransitionError::SessionMismatch {
            current: current.session_id.to_hex(),
            event: event.session_id.to_hex(),
        });
    }

    if !is_causally_valid(&current.causal_vector, &event.causal_vector) {
        return Err(TransitionError::CausalViolation);
    }

    let mut next = current.clone();
    next.version = current.version.wrapping_add(1);
    next.causal_vector.merge(&event.causal_vector);
    next.last_activity_at = event.event_timestamp;
    next.latest_event_id = event.event_id.clone();

    match (current.status, &event.event_type) {
        // === INTAKE PHASE ===
        (SessionStatus::Created, EventType::IntentReceived { .. }) => {
            next.status = SessionStatus::Intake;
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (SessionStatus::Intake, EventType::IntentParsed { intent_graph }) => {
            next.status = SessionStatus::Planning;
            next.intent_graph = intent_graph.clone();
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        // === PLANNING PHASE ===
        (SessionStatus::Planning, EventType::PlanCommitted { frontier }) => {
            next.status = SessionStatus::Planned;
            next.execution_frontier = frontier.clone();
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (SessionStatus::Planning, EventType::PlanProposed { .. }) => {
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (SessionStatus::Planning, EventType::PlanRejected { .. }) => {
            next.status = SessionStatus::Failed;
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        // === EXECUTION PHASE ===
        (SessionStatus::Planned, EventType::DependenciesMet) => {
            if current.execution_frontier.has_fan_in(dag) {
                next.status = SessionStatus::Converging;
            } else {
                next.status = SessionStatus::Executing;
            }
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (SessionStatus::Planned, EventType::FrontierValidated { .. }) => {
            next.status = SessionStatus::Executing;
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (SessionStatus::Executing, EventType::WorkerDispatched { .. }) => Ok(next),

        (SessionStatus::Executing, EventType::WorkerStarted { .. }) => Ok(next),

        (SessionStatus::Executing, EventType::WorkerCheckpoint { step_index, .. }) => {
            if *step_index <= current.checkpoint_seq {
                return Err(TransitionError::StaleCheckpoint {
                    expected: current.checkpoint_seq + 1,
                    received: *step_index,
                });
            }
            next.status = SessionStatus::Checkpointing;
            next.checkpoint_seq = *step_index;
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (SessionStatus::Checkpointing, EventType::WorkerCheckpoint { step_index, .. }) => {
            if *step_index <= current.checkpoint_seq {
                return Err(TransitionError::StaleCheckpoint {
                    expected: current.checkpoint_seq + 1,
                    received: *step_index,
                });
            }
            next.checkpoint_seq = *step_index;
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (SessionStatus::Checkpointing, EventType::WorkerCompleted { .. }) => {
            next.status = SessionStatus::Executing;
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (SessionStatus::Executing, EventType::WorkerCompleted { .. }) => {
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (SessionStatus::Executing, EventType::WorkerFailed { error_code, .. }) => {
            match error_code {
                ErrorCode::Retryable => {
                    if current.retry_policy.max_attempts > 0 {
                        next.status = SessionStatus::Planned;
                    } else {
                        next.status = SessionStatus::Failed;
                    }
                }
                ErrorCode::Fatal => {
                    next.status = SessionStatus::Failed;
                }
            }
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        // === CONVERGENCE PHASE ===
        (SessionStatus::Converging, EventType::ConvergeComplete { .. }) => {
            next.status = SessionStatus::Reflecting;
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        // === REFLECTION PHASE ===
        (SessionStatus::Reflecting, EventType::ReflectionComplete { memory_delta, .. }) => {
            next.status = SessionStatus::Completed;
            next.memory_refs = merge_memory_refs(&current.memory_refs, memory_delta);
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        // === GOVERNANCE INTERRUPTIONS ===
        (SessionStatus::Executing, EventType::HumanApprovalRequested { .. })
        | (SessionStatus::Checkpointing, EventType::HumanApprovalRequested { .. })
        | (SessionStatus::Planned, EventType::HumanApprovalRequested { .. }) => {
            next.status = SessionStatus::Blocked;
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (SessionStatus::Blocked, EventType::HumanApproved { .. }) => {
            next.status = SessionStatus::Executing;
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (SessionStatus::Blocked, EventType::HumanRejected { .. }) => {
            next.status = SessionStatus::Failed;
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        // === SESSION LIFECYCLE ===
        (
            SessionStatus::Intake
            | SessionStatus::Planning
            | SessionStatus::Planned
            | SessionStatus::Executing
            | SessionStatus::Checkpointing,
            EventType::SessionSuspended { .. },
        ) => {
            next.status = SessionStatus::Checkpointing;
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (
            SessionStatus::Checkpointing,
            EventType::SessionResumed {
                inherited_memories, ..
            },
        ) => {
            let mut new_memories = current.memory_refs.clone();
            for m in inherited_memories {
                if !new_memories.iter().any(|existing| existing.memory_id == *m) {
                    new_memories.push(MemoryRef {
                        memory_id: m.clone(),
                        session_origin: current.session_id,
                        causal_vector_at_creation: current.causal_vector.clone(),
                        importance_score: 500,
                    });
                }
            }
            next.memory_refs = new_memories;
            next.status = SessionStatus::Executing;
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (
            SessionStatus::Created
            | SessionStatus::Intake
            | SessionStatus::Planning
            | SessionStatus::Planned
            | SessionStatus::Executing
            | SessionStatus::Checkpointing
            | SessionStatus::Blocked
            | SessionStatus::Converging
            | SessionStatus::Reflecting
            | SessionStatus::Completed
            | SessionStatus::Failed,
            EventType::SessionArchived { final_status, .. },
        ) => {
            next.status = *final_status;
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        // === MEMORY EVENTS ===
        (_, EventType::MemoryConsolidated { .. }) => {
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        // === SIDE EFFECT EVENTS ===
        (_, EventType::SideEffectIntent { .. }) => {
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (_, EventType::SideEffectCommitted { .. }) => {
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        (_, EventType::SideEffectCompensated { .. }) => {
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        // === POLICY ===
        (_, EventType::PolicyDecision { .. }) => {
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        // === CROSS-SESSION ===
        (_, EventType::SessionMigrated { .. }) => {
            next.causal_vector.increment(current.session_id);
            Ok(next)
        }

        // === ILLEGAL TRANSITIONS ===
        _ => Err(TransitionError::IllegalTransition {
            from: format!("{:?}", current.status),
            event: format!("{:?}", event.event_type.as_str()),
        }),
    }
}

fn is_causally_valid(current: &CausalVector, event: &CausalVector) -> bool {
    for (session_id, current_count) in &current.0 {
        let event_count = event.0.get(session_id).copied().unwrap_or(0);
        if event_count < *current_count {
            return false;
        }
    }
    true
}

fn merge_memory_refs(current: &[MemoryRef], delta: &[MemoryDelta]) -> Vec<MemoryRef> {
    let mut result = current.to_vec();
    for d in delta {
        match d.operation {
            MemoryOperation::Add => {
                if !result.iter().any(|m| m.memory_id == d.memory_ref.memory_id) {
                    result.push(d.memory_ref.clone());
                }
            }
            MemoryOperation::Update => {
                if let Some(idx) = result
                    .iter()
                    .position(|m| m.memory_id == d.memory_ref.memory_id)
                {
                    result[idx] = d.memory_ref.clone();
                }
            }
            MemoryOperation::Remove => {
                result.retain(|m| m.memory_id != d.memory_ref.memory_id);
            }
        }
    }
    result.sort_by_key(|m| std::cmp::Reverse(m.importance_score));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventType, NexusEvent};
    use std::collections::BTreeMap;

    fn make_event(event_type: EventType, session_id: SessionId, cv: CausalVector) -> NexusEvent {
        NexusEvent::new(event_type, session_id, cv, None)
    }

    #[test]
    fn test_created_to_intake() {
        let sid = SessionId::from_bytes([1u8; 16]);
        let state = NexusState::new(sid, 0);
        let mut cv = CausalVector::new();
        cv.increment(sid);

        let event = make_event(
            EventType::IntentReceived {
                raw_input: "test".into(),
                source: "test".into(),
            },
            sid,
            cv,
        );

        let result = transition(&state, &event, &BTreeMap::new()).unwrap();
        assert_eq!(result.status, SessionStatus::Intake);
        assert_eq!(result.version, 2);
    }

    #[test]
    fn test_illegal_transition() {
        let sid = SessionId::from_bytes([1u8; 16]);
        let state = NexusState::new(sid, 0);
        let mut cv = CausalVector::new();
        cv.increment(sid);

        let event = make_event(
            EventType::PlanCommitted {
                frontier: Frontier::empty(),
            },
            sid,
            cv,
        );

        let result = transition(&state, &event, &BTreeMap::new());
        assert!(result.is_err());
    }

    #[test]
    fn test_full_lifecycle() {
        let sid = SessionId::from_bytes([1u8; 16]);
        let mut state = NexusState::new(sid, 0);
        let dag = BTreeMap::new();

        // Created -> Intake
        let mut cv = CausalVector::new();
        cv.increment(sid);
        let e1 = make_event(
            EventType::IntentReceived {
                raw_input: "refactor".into(),
                source: "cli".into(),
            },
            sid,
            cv.clone(),
        );
        state = transition(&state, &e1, &dag).unwrap();
        assert_eq!(state.status, SessionStatus::Intake);

        // Intake -> Planning
        cv.increment(sid);
        let e2 = make_event(
            EventType::IntentParsed {
                intent_graph: IntentGraph::default(),
            },
            sid,
            cv.clone(),
        );
        state = transition(&state, &e2, &dag).unwrap();
        assert_eq!(state.status, SessionStatus::Planning);

        // Planning -> Planned
        cv.increment(sid);
        let e3 = make_event(
            EventType::PlanCommitted {
                frontier: Frontier::empty(),
            },
            sid,
            cv.clone(),
        );
        state = transition(&state, &e3, &dag).unwrap();
        assert_eq!(state.status, SessionStatus::Planned);

        // Planned -> Executing
        cv.increment(sid);
        let e4 = make_event(EventType::DependenciesMet, sid, cv.clone());
        state = transition(&state, &e4, &dag).unwrap();
        assert_eq!(state.status, SessionStatus::Executing);

        // Worker checkpoint
        cv.increment(sid);
        let e5 = make_event(
            EventType::WorkerCheckpoint {
                task_id: TaskId::from_bytes([2u8; 16]),
                step_index: 1,
                actions: vec![],
                artifacts: vec![],
            },
            sid,
            cv.clone(),
        );
        state = transition(&state, &e5, &dag).unwrap();
        assert_eq!(state.status, SessionStatus::Checkpointing);
        assert_eq!(state.checkpoint_seq, 1);

        // Resume
        cv.increment(sid);
        let e6 = make_event(
            EventType::SessionResumed {
                from_checkpoint: 1,
                inherited_memories: vec![],
            },
            sid,
            cv.clone(),
        );
        state = transition(&state, &e6, &dag).unwrap();
        assert_eq!(state.status, SessionStatus::Executing);
    }

    #[test]
    fn test_causal_violation_rejected() {
        let sid = SessionId::from_bytes([1u8; 16]);
        let mut state = NexusState::new(sid, 0);
        state.causal_vector = CausalVector::singleton(sid, 5);

        let event_cv = CausalVector::singleton(sid, 3); // Less than current (5)
        let event = make_event(
            EventType::IntentReceived {
                raw_input: "test".into(),
                source: "test".into(),
            },
            sid,
            event_cv,
        );

        let result = transition(&state, &event, &BTreeMap::new());
        assert!(matches!(result, Err(TransitionError::CausalViolation)));
    }

    #[test]
    fn test_session_mismatch_rejected() {
        let sid1 = SessionId::from_bytes([1u8; 16]);
        let sid2 = SessionId::from_bytes([2u8; 16]);
        let state = NexusState::new(sid1, 0);
        let cv = CausalVector::singleton(sid2, 1);

        let event = make_event(
            EventType::IntentReceived {
                raw_input: "test".into(),
                source: "test".into(),
            },
            sid2,
            cv,
        );

        let result = transition(&state, &event, &BTreeMap::new());
        assert!(matches!(
            result,
            Err(TransitionError::SessionMismatch { .. })
        ));
    }

    #[test]
    fn test_deterministic_same_input_same_output() {
        let sid = SessionId::from_bytes([1u8; 16]);
        let state = NexusState::new(sid, 0);
        let dag = BTreeMap::new();
        let cv = CausalVector::singleton(sid, 1);

        let event = make_event(
            EventType::IntentReceived {
                raw_input: "test".into(),
                source: "test".into(),
            },
            sid,
            cv,
        );

        let r1 = transition(&state, &event, &dag).unwrap();
        let r2 = transition(&state, &event, &dag).unwrap();

        assert_eq!(r1.session_id, r2.session_id);
        assert_eq!(r1.status, r2.status);
        assert_eq!(r1.version, r2.version);
        assert_eq!(r1.checkpoint_seq, r2.checkpoint_seq);
    }
}
