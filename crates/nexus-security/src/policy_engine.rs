//! PolicyEngine — runtime evaluation of OPA/Rego policies.
//!
//! Implements the semantics of the .rego files in `policies/rego/`:
//! - `budget.rego`: hot-path budget enforcement before LLM calls and side effects
//! - `capabilities.rego`: capability token validation on every action
//!
//! These are evaluated in-process (< 1ms) without an external OPA daemon.
//! The .rego files serve as the authoritative policy definition;
//! this module provides their Rust runtime implementation.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Result of a policy evaluation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyOutput {
    pub allowed: bool,
    pub denials: Vec<String>,
    /// The policy package that produced this result.
    pub policy: String,
}

impl PolicyOutput {
    pub fn allow(policy: impl Into<String>) -> Self {
        Self {
            allowed: true,
            denials: Vec::new(),
            policy: policy.into(),
        }
    }

    pub fn deny(policy: impl Into<String>, reasons: Vec<String>) -> Self {
        Self {
            allowed: false,
            denials: reasons,
            policy: policy.into(),
        }
    }
}

/// Lightweight policy engine.
///
/// Policies are loaded from .rego files at initialization time but
/// evaluated purely in Rust for sub-millisecond performance.
#[derive(Debug, Clone, Default)]
pub struct PolicyEngine {
    /// Loaded policy definitions keyed by package name.
    policies: BTreeMap<String, PolicyDef>,
}

#[derive(Debug, Clone)]
enum PolicyDef {
    Budget,
    Capabilities,
}

impl PolicyEngine {
    /// Create a new empty engine. Call `load_builtin_policies()` to load defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the built-in policy definitions (budget + capabilities).
    pub fn load_builtin_policies(&mut self) {
        self.policies
            .insert("nexus.policy".into(), PolicyDef::Budget);
        self.policies
            .insert("nexus.capabilities".into(), PolicyDef::Capabilities);
    }

    /// Evaluate the budget policy against the given input.
    ///
    /// Input should be a JSON object with at minimum:
    /// ```json
    /// {
    ///   "action": "llm_call" | "side_effect" | "human_override",
    ///   "cost_cents": 10,
    ///   "budget_remaining": 500,
    ///   "session_status": "active"
    /// }
    /// ```
    pub fn evaluate_budget(&self, input: &Value) -> PolicyOutput {
        let package = "nexus.policy";

        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let mut denials = Vec::new();

        match action {
            "llm_call" => {
                let cost = input.get("cost_cents").and_then(|v| v.as_u64()).unwrap_or(0);
                let remaining = input
                    .get("budget_remaining")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let status = input
                    .get("session_status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if cost > remaining {
                    denials.push(format!(
                        "Budget exceeded: need {}, have {}",
                        cost, remaining
                    ));
                }
                if status == "blocked" {
                    denials.push("Session is blocked pending human review".into());
                }
            }
            "side_effect" => {
                let effect_class = input
                    .get("effect_class")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let cap_valid = input
                    .get("capability_valid")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                if effect_class == "irreversible" {
                    denials.push("Irreversible side effects require human approval".into());
                }
                if !cap_valid {
                    denials.push("Capability token is invalid or expired".into());
                }
            }
            "human_override" => {
                let approver = input
                    .get("approver")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let ts = input.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0);
                let timeout = input.get("timeout").and_then(|v| v.as_u64()).unwrap_or(0);

                if approver.is_empty() {
                    denials.push("Human override requires non-empty approver identity".into());
                }
                if ts >= timeout {
                    denials.push("Human override window has expired".into());
                }
            }
            _ => {
                denials.push(format!("Unknown action: {}", action));
            }
        }

        if denials.is_empty() {
            PolicyOutput::allow(package)
        } else {
            PolicyOutput::deny(package, denials)
        }
    }

    /// Evaluate the capability policy against the given input.
    ///
    /// Input should be a JSON object with at minimum:
    /// ```json
    /// {
    ///   "token_signature_valid": true,
    ///   "token_not_expired": true,
    ///   "session_matches": true,
    ///   "task_matches": true,
    ///   "scope_covers_request": true,
    ///   "path_canonicalized": true
    /// }
    /// ```
    pub fn evaluate_capabilities(&self, input: &Value) -> PolicyOutput {
        let package = "nexus.capabilities";

        let sig_valid = input
            .get("token_signature_valid")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let not_expired = input
            .get("token_not_expired")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let session_ok = input
            .get("session_matches")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let task_ok = input
            .get("task_matches")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let scope_ok = input
            .get("scope_covers_request")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let path_ok = input
            .get("path_canonicalized")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut denials = Vec::new();

        if !sig_valid {
            denials.push("Invalid token signature".into());
        }
        if !not_expired {
            denials.push("Token expired".into());
        }
        if !session_ok {
            denials.push("Session ID mismatch".into());
        }
        if !task_ok {
            denials.push("Task ID mismatch".into());
        }
        if !scope_ok {
            let action = input
                .get("requested_action")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            denials.push(format!(
                "Token does not cover action: {}",
                action
            ));
        }
        if !path_ok {
            let path = input
                .get("requested_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            denials.push(format!(
                "Path contains traversal pattern: {}",
                path
            ));
        }

        if denials.is_empty() {
            PolicyOutput::allow(package)
        } else {
            PolicyOutput::deny(package, denials)
        }
    }

    /// Generic evaluate: dispatches to the correct policy based on `policy_package`.
    pub fn evaluate(&self, policy_package: &str, input: &Value) -> PolicyOutput {
        match self.policies.get(policy_package) {
            Some(PolicyDef::Budget) => self.evaluate_budget(input),
            Some(PolicyDef::Capabilities) => self.evaluate_capabilities(input),
            None => PolicyOutput::deny(
                "unknown",
                vec![format!("Unknown policy package: {}", policy_package)],
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn engine() -> PolicyEngine {
        let mut e = PolicyEngine::new();
        e.load_builtin_policies();
        e
    }

    // ── Budget policy tests ────────────────────────────────────────────

    #[test]
    fn budget_allow_llm_call_within_limit() {
        let input = json!({
            "action": "llm_call",
            "cost_cents": 10,
            "budget_remaining": 500,
            "session_status": "active"
        });
        let out = engine().evaluate_budget(&input);
        assert!(out.allowed, "LLM call within budget should be allowed");
    }

    #[test]
    fn budget_deny_llm_call_over_limit() {
        let input = json!({
            "action": "llm_call",
            "cost_cents": 600,
            "budget_remaining": 500,
            "session_status": "active"
        });
        let out = engine().evaluate_budget(&input);
        assert!(!out.allowed);
        assert!(out.denials.iter().any(|d| d.contains("Budget exceeded")));
    }

    #[test]
    fn budget_deny_blocked_session() {
        let input = json!({
            "action": "llm_call",
            "cost_cents": 10,
            "budget_remaining": 500,
            "session_status": "blocked"
        });
        let out = engine().evaluate_budget(&input);
        assert!(!out.allowed);
        assert!(out.denials.iter().any(|d| d.contains("blocked")));
    }

    #[test]
    fn budget_allow_side_effect_reversible() {
        let input = json!({
            "action": "side_effect",
            "effect_class": "reversible",
            "capability_valid": true
        });
        let out = engine().evaluate_budget(&input);
        assert!(out.allowed);
    }

    #[test]
    fn budget_deny_side_effect_irreversible() {
        let input = json!({
            "action": "side_effect",
            "effect_class": "irreversible",
            "capability_valid": true
        });
        let out = engine().evaluate_budget(&input);
        assert!(!out.allowed);
    }

    #[test]
    fn budget_deny_side_effect_invalid_capability() {
        let input = json!({
            "action": "side_effect",
            "effect_class": "reversible",
            "capability_valid": false
        });
        let out = engine().evaluate_budget(&input);
        assert!(!out.allowed);
    }

    #[test]
    fn budget_allow_human_override_valid() {
        let input = json!({
            "action": "human_override",
            "approver": "admin@example.com",
            "timestamp": 1000,
            "timeout": 2000
        });
        let out = engine().evaluate_budget(&input);
        assert!(out.allowed);
    }

    #[test]
    fn budget_deny_human_override_empty_approver() {
        let input = json!({
            "action": "human_override",
            "approver": "",
            "timestamp": 1000,
            "timeout": 2000
        });
        let out = engine().evaluate_budget(&input);
        assert!(!out.allowed);
    }

    #[test]
    fn budget_deny_human_override_expired() {
        let input = json!({
            "action": "human_override",
            "approver": "admin",
            "timestamp": 2000,
            "timeout": 1000
        });
        let out = engine().evaluate_budget(&input);
        assert!(!out.allowed);
    }

    #[test]
    fn budget_deny_unknown_action() {
        let input = json!({
            "action": "sudo",
            "cost_cents": 0,
            "budget_remaining": 100,
            "session_status": "active"
        });
        let out = engine().evaluate_budget(&input);
        assert!(!out.allowed);
    }

    // ── Capabilities policy tests ───────────────────────────────────────

    #[test]
    fn capabilities_allow_all_valid() {
        let input = json!({
            "token_signature_valid": true,
            "token_not_expired": true,
            "session_matches": true,
            "task_matches": true,
            "scope_covers_request": true,
            "path_canonicalized": true
        });
        let out = engine().evaluate_capabilities(&input);
        assert!(out.allowed, "All valid should allow: {:?}", out.denials);
    }

    #[test]
    fn capabilities_deny_invalid_signature() {
        let input = json!({
            "token_signature_valid": false,
            "token_not_expired": true,
            "session_matches": true,
            "task_matches": true,
            "scope_covers_request": true,
            "path_canonicalized": true
        });
        let out = engine().evaluate_capabilities(&input);
        assert!(!out.allowed);
        assert!(out.denials.iter().any(|d| d.contains("signature")));
    }

    #[test]
    fn capabilities_deny_expired() {
        let input = json!({
            "token_signature_valid": true,
            "token_not_expired": false,
            "session_matches": true,
            "task_matches": true,
            "scope_covers_request": true,
            "path_canonicalized": true
        });
        let out = engine().evaluate_capabilities(&input);
        assert!(!out.allowed);
        assert!(out.denials.iter().any(|d| d.contains("expired")));
    }

    #[test]
    fn capabilities_deny_session_mismatch() {
        let input = json!({
            "token_signature_valid": true,
            "token_not_expired": true,
            "session_matches": false,
            "task_matches": true,
            "scope_covers_request": true,
            "path_canonicalized": true
        });
        let out = engine().evaluate_capabilities(&input);
        assert!(!out.allowed);
    }

    #[test]
    fn capabilities_deny_scope_not_covered() {
        let input = json!({
            "token_signature_valid": true,
            "token_not_expired": true,
            "session_matches": true,
            "task_matches": true,
            "scope_covers_request": false,
            "path_canonicalized": true,
            "requested_action": "fs:write:/etc"
        });
        let out = engine().evaluate_capabilities(&input);
        assert!(!out.allowed);
        assert!(out.denials.iter().any(|d| d.contains("fs:write")));
    }

    #[test]
    fn capabilities_deny_path_traversal() {
        let input = json!({
            "token_signature_valid": true,
            "token_not_expired": true,
            "session_matches": true,
            "task_matches": true,
            "scope_covers_request": true,
            "path_canonicalized": false,
            "requested_path": "../../etc/passwd"
        });
        let out = engine().evaluate_capabilities(&input);
        assert!(!out.allowed);
        assert!(out.denials.iter().any(|d| d.contains("traversal")));
    }

    // ── Generic evaluate tests ──────────────────────────────────────────

    #[test]
    fn generic_evaluate_dispatches_to_budget() {
        let input = json!({
            "action": "llm_call",
            "cost_cents": 999,
            "budget_remaining": 100,
            "session_status": "active"
        });
        let out = engine().evaluate("nexus.policy", &input);
        assert!(!out.allowed);
        assert_eq!(out.policy, "nexus.policy");
    }

    #[test]
    fn generic_evaluate_dispatches_to_capabilities() {
        let input = json!({
            "token_signature_valid": true,
            "token_not_expired": true,
            "session_matches": true,
            "task_matches": true,
            "scope_covers_request": true,
            "path_canonicalized": true
        });
        let out = engine().evaluate("nexus.capabilities", &input);
        assert!(out.allowed);
        assert_eq!(out.policy, "nexus.capabilities");
    }

    #[test]
    fn generic_evaluate_unknown_package() {
        let out = engine().evaluate("nexus.unknown", &json!({}));
        assert!(!out.allowed);
    }
}
