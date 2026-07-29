//! # Nexus Core
//!
//! **Causally-consistent execution substrate for autonomous agent systems.**
//!
//! Nexus Core provides the foundational runtime for building durable, auditable,
//! and portable AI agent execution. The event log is the source of truth; state
//! is a materialized view; workers are stateless; the Kernel owns causality.
//!
//! ## Architecture
//!
//! The crate is organized around these core subsystems:
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`state_machine`] | Pure `transition()` function — 12 states, 25+ event types, zero I/O |
//! | [`event`] | Immutable event types with causal vector clocks |
//! | [`types`] | Shared types: `NexusState`, `CausalVector`, `Budget`, `TaskId` |
//! | [`session_driver`] | Standard session lifecycle (intake → plan → execute → checkpoint) |
//! | [`recovery`] | Crash recovery: replay events, verify causal integrity, resume |
//! | [`checkpoint`] | Worker checkpoints with BLAKE3 content addressing |
//! | [`llm_proxy`] | LLM API proxy with cost tracking and simulation fallback |
//! | [`effects`] | Side-effect guard: Pure / Idempotent / Reversible / Irreversible |
//! | [`vault`] | Content-addressable artifact store (BLAKE3 content hash) |
//! | [`entropy`] | Entropy injection for controlled non-determinism |
//! | [`memory`] | Memory graph with vector embeddings |
//! | [`protocol`] | JSON-RPC 2.0 worker protocol over stdio (NDJSON framing) |
//! | [`worker_spawner`] | Spawn local workers (Python, Node.js, Rust, WASM) |
//! | [`wasm_worker`] | WASM sandbox with fuel metering and capability tokens |
//! | [`export`] | Session export/import for cross-tool migration |
//! | [`migration`] | Cross-node session migration with causal merge |
//!
//! ## Quick Example
//!
//! ```rust,no_run
//! use nexus_core::{SessionDriver, SessionId, LlmProxy};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create a session with an LLM proxy for planning
//! let llm_proxy = LlmProxy::new(b"signing-key".to_vec());
//! let mut session = SessionDriver::new(SessionId::new(), my_store, llm_proxy);
//!
//! // Run the standard lifecycle: intake → plan → commit → execute
//! session.run_standard_lifecycle("audit auth.js for security flaws", "deepseek-chat").await?;
//!
//! println!("Session status: {:?}", session.state.status);
//! # Ok(())
//! # }
//! ```
//!
//! ## Design Invariants
//!
//! - **Deterministic state machine**: `transition()` is pure — no async, no I/O, no clock
//! - **Append-only event log**: Events are immutable once written
//! - **Causal consistency**: Vector clocks track happens-before across sessions
//! - **Stateless workers**: No persistent memory, no network, capability-constrained
//! - **Two-phase side effects**: Intent → Validate → Execute → Commit

#![deny(clippy::disallowed_types)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod checkpoint;
pub mod effects;
pub mod entropy;
pub mod event;
pub mod export;
pub mod llm_proxy;
pub mod memory;
pub mod migration;
pub mod protocol;
pub mod recovery;
pub mod session_driver;
pub mod state_machine;
pub mod types;
pub mod vault;
pub mod wasm_worker;
pub mod worker_spawner;

pub use checkpoint::*;
pub use effects::*;
pub use entropy::*;
pub use event::*;
pub use export::SessionExport;
pub use llm_proxy::{LlmProxy, LlmRequest, LlmResponse, ProxyError};
pub use memory::{
    Blake3EmbeddingGenerator, EmbeddingGenerator, NoopEmbeddingGenerator,
};
pub use migration::{CrossNodeSession, MigrationStatus, SessionMigrationManager};
pub use protocol::*;
pub use recovery::*;
pub use session_driver::*;
pub use state_machine::*;
pub use types::*;
pub use vault::{ContentVault, VaultEntry, VaultError};
pub use wasm_worker::{
    SandboxViolation, WasmInput, WasmOutput, WasmSandboxWorker, WasmSkill, WasmSkillRegistry,
};
pub use worker_spawner::{
    WorkerConfig as SpawnerConfig, WorkerHandle, WorkerSpawner, WorkerStatus,
};

#[cfg(test)]
mod golden_tests;
