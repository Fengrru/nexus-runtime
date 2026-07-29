use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SessionId(pub [u8; 16]);

impl SessionId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().into_bytes())
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
        let mut bytes = [0u8; 16];
        hex::decode_to_slice(s, &mut bytes)?;
        Ok(Self(bytes))
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaskId(pub [u8; 16]);

impl TaskId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().into_bytes())
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TraceId(pub [u8; 16]);

impl TraceId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().into_bytes())
    }
}

impl Default for TraceId {
    fn default() -> Self {
        Self::new()
    }
}

/// A vector clock tracking causal ordering across distributed sessions.
///
/// Each entry maps a [`SessionId`] to a monotonically increasing counter.
/// The vector clock enables:
///
/// - **Happens-before** detection via [`happened_before`](CausalVector::happened_before)
/// - **Concurrency** detection via [`is_concurrent`](CausalVector::is_concurrent)
/// - **Commutative merges** via [`merge`](CausalVector::merge) — merging is idempotent
///
/// Uses `BTreeMap` (not `HashMap`) for deterministic iteration order,
/// which is required for byte-identical serialization during replay.
///
/// # Examples
///
/// ```
/// use nexus_core::{CausalVector, SessionId};
///
/// let sid_a = SessionId::from_bytes([1u8; 16]);
/// let sid_b = SessionId::from_bytes([2u8; 16]);
///
/// let mut cv1 = CausalVector::new();
/// cv1.increment(sid_a);
/// cv1.increment(sid_a);
///
/// let mut cv2 = CausalVector::new();
/// cv2.increment(sid_a);
/// cv2.increment(sid_b);
///
/// // cv1 happened before cv2 at sid_a (1 < 2), and cv2 has sid_b
/// assert!(cv1.happened_before(&cv2));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CausalVector(pub BTreeMap<SessionId, u64>);

impl CausalVector {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn singleton(session_id: SessionId, count: u64) -> Self {
        let mut map = BTreeMap::new();
        map.insert(session_id, count);
        Self(map)
    }

    pub fn increment(&mut self, session_id: SessionId) {
        *self.0.entry(session_id).or_insert(0) += 1;
    }

    pub fn merge(&mut self, other: &CausalVector) {
        for (k, v) in &other.0 {
            let entry = self.0.entry(*k).or_insert(0);
            *entry = (*entry).max(*v);
        }
    }

    pub fn happened_before(&self, other: &CausalVector) -> bool {
        let mut strictly_less = false;
        for (session, count) in &self.0 {
            let other_count = other.0.get(session).copied().unwrap_or(0);
            if *count > other_count {
                return false;
            }
            if *count < other_count {
                strictly_less = true;
            }
        }
        strictly_less || self.0.len() < other.0.len()
    }

    pub fn is_concurrent(&self, other: &CausalVector) -> bool {
        !self.happened_before(other) && !other.happened_before(self)
    }

    pub fn is_consistent(&self) -> bool {
        // A causal vector is consistent if all counts are positive
        // and the vector is not empty (must have at least one entry)
        if self.0.is_empty() {
            return false;
        }
        self.0.values().all(|&count| count > 0)
    }

    pub fn to_canonical(&self) -> String {
        let map: std::collections::BTreeMap<String, u64> =
            self.0.iter().map(|(k, v)| (k.to_hex(), *v)).collect();
        serde_json::to_string(&map).unwrap_or_default()
    }

    pub fn from_canonical(s: &str) -> Result<Self, String> {
        let map: std::collections::BTreeMap<String, u64> =
            serde_json::from_str(s).map_err(|e| format!("causal_vector parse: {}", e))?;
        let mut result = BTreeMap::new();
        for (hex_key, count) in map {
            let sid =
                SessionId::from_hex(&hex_key).map_err(|e| format!("session_id parse: {}", e))?;
            result.insert(sid, count);
        }
        Ok(CausalVector(result))
    }
}

impl Default for CausalVector {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BudgetState {
    pub budget_limit_cents: u64,
    pub consumed_cents: u64,
    pub token_count: u64,
    pub tool_call_count: u64,
}

impl Default for BudgetState {
    fn default() -> Self {
        Self {
            budget_limit_cents: 500,
            consumed_cents: 0,
            token_count: 0,
            tool_call_count: 0,
        }
    }
}

impl BudgetState {
    pub fn remaining_cents(&self) -> u64 {
        self.budget_limit_cents.saturating_sub(self.consumed_cents)
    }

    pub fn is_exhausted(&self) -> bool {
        self.consumed_cents >= self.budget_limit_cents
    }

    pub fn add_cost(&mut self, cents: u64, tokens: u64, tool_calls: u64) {
        self.consumed_cents = self.consumed_cents.saturating_add(cents);
        self.token_count = self.token_count.saturating_add(tokens);
        self.tool_call_count = self.tool_call_count.saturating_add(tool_calls);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_interval_ms: u64,
    pub backoff_multiplier: f64,
    pub max_interval_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_interval_ms: 1000,
            backoff_multiplier: 2.0,
            max_interval_ms: 60000,
        }
    }
}

impl RetryPolicy {
    pub fn can_retry(&self, attempts: u32) -> bool {
        attempts < self.max_attempts
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    #[default]
    Created,
    Intake,
    Planning,
    Planned,
    Executing,
    Checkpointing,
    Blocked,
    Converging,
    Reflecting,
    Completed,
    Failed,
    Archived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Action,
    FanIn,
    HumanGate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerType {
    Python,
    NodeJs,
    RustInline,
    WasmSandbox,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectClass {
    Pure,
    Idempotent,
    Reversible,
    Irreversible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Retryable,
    Fatal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOperation {
    Add,
    Update,
    Remove,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Constraint {
    #[serde(rename = "constraint_type")]
    pub constraint_type: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntentGraph {
    pub root: TaskId,
    pub nodes: BTreeMap<TaskId, TaskNode>,
    pub edges: Vec<(TaskId, TaskId)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskNode {
    pub id: TaskId,
    pub kind: TaskKind,
    pub worker_type: WorkerType,
    pub intent: TaskIntent,
    pub dependencies: Vec<TaskId>,
    pub capabilities: Vec<String>,
    pub side_effect_class: SideEffectClass,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskIntent {
    pub action_type: String,
    pub target: String,
    pub parameters: BTreeMap<String, String>,
    pub constraints: Vec<Constraint>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Frontier {
    pub nodes: Vec<TaskId>,
    pub blocked: Vec<TaskId>,
    pub completed: Vec<TaskId>,
}

impl Frontier {
    pub fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            blocked: Vec::new(),
            completed: Vec::new(),
        }
    }

    pub fn has_fan_in(&self, dag: &BTreeMap<TaskId, TaskNode>) -> bool {
        self.nodes.iter().any(|task_id| {
            dag.get(task_id)
                .map(|node| node.kind == TaskKind::FanIn)
                .unwrap_or(false)
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryRef {
    pub memory_id: String,
    pub session_origin: SessionId,
    pub causal_vector_at_creation: CausalVector,
    pub importance_score: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryDelta {
    pub operation: MemoryOperation,
    pub memory_ref: MemoryRef,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryGraph {
    pub nodes: BTreeMap<String, MemoryNode>,
    pub edges: Vec<MemoryEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryNode {
    pub id: String,
    pub content: MemoryContent,
    pub embedding: Option<Vec<u8>>,
    pub causal_context: CausalVector,
    pub importance: u64,
    pub activation: u64,
    pub source_event_id: String,
    pub session_lineage: Vec<SessionId>,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryContent {
    Text {
        text: String,
    },
    Structured {
        data: BTreeMap<String, String>,
    },
    Proposition {
        subject: String,
        predicate: String,
        object: String,
        confidence: u64,
    },
    Skill {
        skill_id: String,
        version: String,
        parameters: BTreeMap<String, String>,
    },
}

impl MemoryContent {
    pub fn matches_goal(&self, goal: &str) -> bool {
        match self {
            MemoryContent::Text { text } => text.contains(goal),
            MemoryContent::Structured { data } => data.values().any(|v| v.contains(goal)),
            MemoryContent::Proposition {
                subject,
                predicate,
                object,
                ..
            } => subject.contains(goal) || predicate.contains(goal) || object.contains(goal),
            MemoryContent::Skill {
                skill_id,
                parameters,
                ..
            } => skill_id.contains(goal) || parameters.values().any(|v| v.contains(goal)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryEdge {
    pub from: String,
    pub to: String,
    pub edge_type: MemoryEdgeType,
    pub confidence: u64,
    pub created_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEdgeType {
    DerivesFrom,
    Contradicts,
    Refines,
    Generalizes,
    Enables,
    CausedBy,
    SimilarTo,
    PartOf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactRef {
    pub id: String,
    pub kind: ArtifactKind,
    pub uri: String,
    pub blake3: String,
    pub size_bytes: u64,
    pub produced_by_session: SessionId,
    pub produced_by_event: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArtifactKind {
    File,
    Directory,
    Json,
    Embedding,
    Diff,
    Log,
}

/// The full state of a Nexus session — the materialized view of the event log.
///
/// All fields are derived deterministically from the append-only event log.
/// State is reconstructed by replaying events through [`transition`](crate::transition).
///
/// # Key Fields
///
/// | Field | Purpose |
/// |-------|---------|
/// | `session_id` | Unique identifier (UUID v4 in 16 bytes) |
/// | `version` | Monotonic counter, incremented on every transition |
/// | `status` | Current [`SessionStatus`] in the state machine |
/// | `causal_vector` | Vector clock for causal ordering across sessions |
/// | `budget` | Cost tracking with limit enforcement |
/// | `checkpoint_seq` | Latest worker checkpoint sequence number |
/// | `execution_frontier` | Tasks currently in the execution front |
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NexusState {
    pub session_id: SessionId,
    pub version: u64,
    pub status: SessionStatus,
    pub causal_vector: CausalVector,
    pub intent_graph: IntentGraph,
    pub execution_frontier: Frontier,
    pub memory_refs: Vec<MemoryRef>,
    pub memory_graph: MemoryGraph,
    pub budget: BudgetState,
    pub retry_policy: RetryPolicy,
    pub checkpoint_seq: u64,
    pub created_at: u64,
    pub last_activity_at: u64,
    pub latest_event_id: String,
}

impl NexusState {
    pub fn new(session_id: SessionId, created_at: u64) -> Self {
        Self {
            session_id,
            version: 1,
            status: SessionStatus::Created,
            causal_vector: CausalVector::new(),
            intent_graph: IntentGraph::default(),
            execution_frontier: Frontier::empty(),
            memory_refs: Vec::new(),
            memory_graph: MemoryGraph::default(),
            budget: BudgetState::default(),
            retry_policy: RetryPolicy::default(),
            checkpoint_seq: 0,
            created_at,
            last_activity_at: created_at,
            latest_event_id: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionPlan {
    pub plan_id: String,
    pub tasks: Vec<TaskNode>,
    pub estimated_tokens: u64,
    pub estimated_cost_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValidationResult {
    pub valid: bool,
    pub issues: Vec<String>,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerResult {
    pub status: String,
    pub artifacts: Vec<ArtifactRef>,
    pub metrics: WorkerMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerMetrics {
    pub duration_ms: u64,
    pub tokens_consumed: u64,
    pub cost_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Evaluation {
    pub score: f64,
    pub summary: String,
    pub recommendations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryContext {
    pub embedding: Option<Vec<u8>>,
    pub active_goals: Vec<String>,
    pub recent_memories: Vec<String>,
    pub now: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CausalRelation {
    Before,
    After,
    Concurrent,
}

impl CausalVector {
    pub fn compare(&self, other: &CausalVector) -> CausalRelation {
        if self.happened_before(other) {
            CausalRelation::Before
        } else if other.happened_before(self) {
            CausalRelation::After
        } else {
            CausalRelation::Concurrent
        }
    }
}

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn generate_event_id() -> String {
    let timestamp = now_millis();
    let uuid1 = uuid::Uuid::new_v4().to_string();
    format!("e_{}_{}", timestamp, &uuid1[..8])
}

pub fn generate_trace_id() -> [u8; 16] {
    uuid::Uuid::new_v4().into_bytes()
}

pub fn generate_nonce() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SideEffectIntent {
    pub id: String,
    pub session_id: SessionId,
    pub task_id: TaskId,
    pub effect_class: SideEffectClass,
    pub action_type: String,
    pub target: String,
    pub payload: Vec<u8>,
    pub request_hash: String,
    pub preconditions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectStatus {
    Pending,
    Committed,
    Compensated,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SideEffectRecord {
    pub id: String,
    pub session_id: SessionId,
    pub event_id: String,
    pub intent: SideEffectIntent,
    pub status: EffectStatus,
    pub response_payload: Option<Vec<u8>>,
    pub response_hash: Option<String>,
    pub compensation_data: Option<CompensationData>,
    pub committed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CompensationData {
    FileEdit {
        original_content_hash: String,
        original_content_uri: String,
    },
    FileCreate {
        created_path: String,
    },
    Command {
        undo_command: String,
        undo_args: Vec<String>,
    },
    DatabaseTransaction {
        rollback_sql: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmCallRecord {
    pub request_id: String,
    pub session_id: SessionId,
    pub event_id: String,
    pub model: String,
    pub prompt_hash: String,
    pub response_hash: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd_cents: i64,
    pub status: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceLock {
    pub resource_id: String,
    pub owner_session: SessionId,
    pub owner_task: Option<TaskId>,
    pub mode: LockMode,
    pub acquired_at: u64,
    pub lease_expiry: Option<u64>,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockMode {
    Exclusive,
    Shared,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_causal_vector_canonical_round_trip() {
        let sid = SessionId::from_bytes([1u8; 16]);
        let mut cv = CausalVector::new();
        cv.increment(sid);
        cv.increment(sid);
        cv.increment(sid);
        cv.increment(sid);
        cv.increment(sid);

        let canonical = cv.to_canonical();
        assert!(!canonical.is_empty());
        assert!(canonical.contains(&sid.to_hex()));

        let parsed = CausalVector::from_canonical(&canonical).unwrap();
        assert_eq!(parsed.0.get(&sid), Some(&5u64));

        // Round-trip through to_canonical -> from_canonical
        let canonical2 = parsed.to_canonical();
        assert_eq!(canonical, canonical2, "canonical must be idempotent");
    }

    #[test]
    fn test_session_id_to_hex_round_trip() {
        let sid =
            SessionId::from_bytes([0xde, 0xad, 0xbe, 0xef, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let hex_str = sid.to_hex();
        let parsed = SessionId::from_hex(&hex_str).unwrap();
        assert_eq!(sid, parsed);
    }
}
