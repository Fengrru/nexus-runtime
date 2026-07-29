use crate::event::*;
use crate::llm_proxy::{LlmProxy, LlmRequest, LlmResponse, ProxyError};
use crate::state_machine::*;
use crate::types::*;
use async_trait::async_trait;
use std::collections::BTreeMap;

/// Minimal storage trait for SessionDriver.
/// Implemented by nexus-event-store for SQLite/Postgres backends.
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn append_event(&self, event: &NexusEvent) -> Result<(), String>;
    async fn update_state(&self, state: &NexusState, expected_version: u64)
        -> Result<bool, String>;
}

/// SessionDriver encapsulates the standard session lifecycle,
/// eliminating duplicate event construction code across CLI, tests, and recovery.
pub struct SessionDriver<S: SessionStore> {
    pub state: NexusState,
    pub causal_vector: CausalVector,
    pub store: S,
    pub llm_proxy: LlmProxy,
    pub dag: BTreeMap<TaskId, TaskNode>,
}

impl<S: SessionStore> SessionDriver<S> {
    pub fn new(session_id: SessionId, store: S, llm_proxy: LlmProxy) -> Self {
        Self {
            state: NexusState::new(session_id, now_millis()),
            causal_vector: CausalVector::new(),
            store,
            llm_proxy,
            dag: BTreeMap::new(),
        }
    }

    /// Append an event to the store and transition the state machine.
    pub async fn apply(&mut self, event_type: EventType) -> Result<(), SessionDriverError> {
        self.causal_vector.increment(self.state.session_id);
        let event = NexusEvent::new(
            event_type,
            self.state.session_id,
            self.causal_vector.clone(),
            Some(self.state.latest_event_id.clone()),
        );

        self.store
            .append_event(&event)
            .await
                    .map_err(SessionDriverError::StoreError)?;

        self.state.latest_event_id = event.event_id.clone();
        self.state = transition(&self.state, &event, &self.dag)
            .map_err(|e| SessionDriverError::TransitionError(format!("{:?}", e)))?;

        Ok(())
    }

    /// Phase 1: Record user intent — moves Created → Intake.
    pub async fn intake(
        &mut self,
        raw_input: &str,
        source: &str,
    ) -> Result<(), SessionDriverError> {
        self.apply(EventType::IntentReceived {
            raw_input: raw_input.to_string(),
            source: source.to_string(),
        })
        .await
    }

    /// Phase 2: Parse intent into a task graph — moves Intake → Planning.
    pub async fn parse(&mut self, intent_graph: IntentGraph) -> Result<(), SessionDriverError> {
        self.dag = intent_graph.nodes.clone();
        self.apply(EventType::IntentParsed { intent_graph }).await
    }

    /// Phase 3: LLM planning via proxy — stays in Planning, records plan proposal.
    pub async fn plan_with_llm(
        &mut self,
        model: &str,
        prompt: &str,
    ) -> Result<LlmResponse, SessionDriverError> {
        let llm_request = LlmRequest {
            request_id: format!("req_{}", now_millis()),
            session_id: self.state.session_id,
            model: model.to_string(),
            prompt: prompt.to_string(),
            max_tokens: 2048,
            temperature: 0.3,
        };

        let mut budget = self.state.budget.clone();
        match self
            .llm_proxy
            .proxy_call(llm_request, &mut budget, &self.causal_vector)
            .await
        {
            Ok((response, llm_event)) => {
                self.causal_vector.increment(self.state.session_id);
                self.store
                    .append_event(&llm_event)
                    .await
            .map_err(SessionDriverError::StoreError)?;

                self.state.latest_event_id = llm_event.event_id.clone();
                self.state.budget = budget;

                if let Ok(next) = transition(&self.state, &llm_event, &self.dag) {
                    self.state = next;
                }
                Ok(response)
            }
            Err(ProxyError::ApiError(ref msg)) if msg.contains("not set") => {
                Err(SessionDriverError::LlmApiKeyNotSet(msg.clone()))
            }
            Err(e) => Err(SessionDriverError::LlmError(format!("{}", e))),
        }
    }

    /// Phase 4: Commit the execution plan — moves Planning → Planned.
    pub async fn commit_plan(&mut self, frontier: Frontier) -> Result<(), SessionDriverError> {
        self.apply(EventType::PlanCommitted { frontier }).await
    }

    /// Phase 5: Mark dependencies as met — moves Planned → Executing.
    pub async fn mark_dependencies_met(&mut self) -> Result<(), SessionDriverError> {
        self.apply(EventType::DependenciesMet).await
    }

    /// Record a worker checkpoint — moves Executing → Checkpointing.
    pub async fn checkpoint(
        &mut self,
        task_id: TaskId,
        step_index: u64,
        actions: Vec<Action>,
        artifacts: Vec<ArtifactRef>,
    ) -> Result<(), SessionDriverError> {
        self.apply(EventType::WorkerCheckpoint {
            task_id,
            step_index,
            actions,
            artifacts,
        })
        .await
    }

    /// Mark worker as completed — moves Checkpointing → Executing.
    pub async fn worker_completed(
        &mut self,
        worker_id: &str,
        task_id: TaskId,
        result: WorkerResult,
        duration_ms: u64,
    ) -> Result<(), SessionDriverError> {
        self.apply(EventType::WorkerCompleted {
            worker_id: worker_id.to_string(),
            task_id,
            result,
            duration_ms,
        })
        .await
    }

    /// Mark worker as failed.
    pub async fn worker_failed(
        &mut self,
        worker_id: &str,
        task_id: TaskId,
        error: &str,
        error_code: ErrorCode,
        retry_count: u32,
    ) -> Result<(), SessionDriverError> {
        self.apply(EventType::WorkerFailed {
            worker_id: worker_id.to_string(),
            task_id,
            error: error.to_string(),
            error_code,
            retry_count,
        })
        .await
    }

    /// Suspend the session.
    pub async fn suspend(&mut self, reason: &str) -> Result<(), SessionDriverError> {
        self.apply(EventType::SessionSuspended {
            reason: reason.to_string(),
        })
        .await
    }

    /// Resume with inherited memories.
    pub async fn resume(
        &mut self,
        from_checkpoint: u64,
        inherited_memories: Vec<String>,
    ) -> Result<(), SessionDriverError> {
        self.apply(EventType::SessionResumed {
            from_checkpoint,
            inherited_memories,
        })
        .await
    }

    /// Persist current state to the store.
    pub async fn persist_state(&self) -> Result<(), SessionDriverError> {
        self.store
            .update_state(&self.state, self.state.version.saturating_sub(1))
            .await
            .map_err(SessionDriverError::StoreError)?;
        Ok(())
    }

    /// Run the standard lifecycle: intake → parse → plan → commit → execute.
    pub async fn run_standard_lifecycle(
        &mut self,
        intent: &str,
        model: &str,
    ) -> Result<(), SessionDriverError> {
        self.intake(intent, "cli").await?;
        self.parse(IntentGraph::default()).await?;

        let prompt = format!(
            "You are a task planner. Decompose this user intent into executable steps.\n\
             Intent: {}\n\
             Output a JSON array of steps, each step has: action_type (read_file/write_file/grep/run_command), target (file path), and parameters (key-value map).\n\
             Reply with ONLY the JSON array, no other text.",
            intent
        );

        match self.plan_with_llm(model, &prompt).await {
            Ok(_) => {}
            Err(SessionDriverError::LlmApiKeyNotSet(_)) => {
                tracing::info!(
                    target = "nexus.session_driver",
                    "No API key, continuing without LLM plan"
                );
            }
            Err(e) => return Err(e),
        }

        self.commit_plan(Frontier::empty()).await?;
        self.mark_dependencies_met().await?;
        self.persist_state().await?;

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionDriverError {
    #[error("Store error: {0}")]
    StoreError(String),

    #[error("Transition error: {0}")]
    TransitionError(String),

    #[error("LLM error: {0}")]
    LlmError(String),

    #[error("LLM API key not set: {0}")]
    LlmApiKeyNotSet(String),
}
