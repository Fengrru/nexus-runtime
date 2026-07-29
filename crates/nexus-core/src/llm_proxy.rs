use crate::event::*;
use crate::protocol::*;
use crate::types::*;
use serde::{Deserialize, Serialize};
/// LLM Proxy — All LLM API calls are routed through the Kernel proxy.
/// Workers NEVER have direct access to paid APIs.
/// The proxy enforces budget, caches responses, and records audit trails.
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    pub request_id: String,
    pub session_id: SessionId,
    pub model: String,
    pub prompt: String,
    pub max_tokens: u64,
    pub temperature: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub request_id: String,
    pub model: String,
    pub content: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_cents: u64,
    pub response_hash: String,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmCacheEntry {
    pub prompt_hash: String,
    pub response: LlmResponse,
    pub cached_at: u64,
    pub hit_count: u64,
}

pub struct LlmProxy {
    cache: BTreeMap<String, LlmCacheEntry>,
    #[allow(dead_code)]
    signing_key: Vec<u8>,
}

impl LlmProxy {
    pub fn new(signing_key: Vec<u8>) -> Self {
        Self {
            cache: BTreeMap::new(),
            signing_key,
        }
    }

    pub async fn proxy_call(
        &mut self,
        request: LlmRequest,
        budget: &mut BudgetState,
        causal_vector: &CausalVector,
    ) -> Result<(LlmResponse, NexusEvent), ProxyError> {
        let prompt_hash = compute_hash(request.prompt.as_bytes());

        // Check cache first (FR-4.3.2: never re-call LLM on recovery)
        if let Some(entry) = self.cache.get(&prompt_hash) {
            tracing::info!(
                target = "nexus.llm_proxy",
                request_id = %request.request_id,
                prompt_hash = %prompt_hash,
                "LLM cache hit — no API call needed"
            );
            return Ok((
                entry.response.clone(),
                self.build_llm_event(&request, &entry.response, &prompt_hash, true, causal_vector)?,
            ));
        }

        // Estimate cost (simple model pricing)
        let estimated_cost = self.estimate_cost(&request.model, request.max_tokens);

        if !budget.can_afford(estimated_cost) {
            return Err(ProxyError::BudgetExceeded {
                required: estimated_cost,
                remaining: budget.remaining_cents(),
            });
        }

        // Call real LLM API (falls back to simulation if keys not configured)
        let start = now_millis();
        let mut response = match self.call_real_api(&request).await {
            Ok(resp) => resp,
            Err(ProxyError::ApiError(msg)) if msg.contains("not set") => {
                tracing::warn!(
                    target = "nexus.llm_proxy",
                    reason = %msg,
                    "Falling back to simulated API call"
                );
                self.simulate_api_call(&request).await?
            }
            Err(e) => return Err(e),
        };
        response.latency_ms = now_millis() - start;

        let _response_hash = compute_hash(response.content.as_bytes());

        // Deduct from budget
        budget.add_cost(response.cost_cents, response.output_tokens, 1);

        // Cache the response (never re-call)
        self.cache.insert(
            prompt_hash.clone(),
            LlmCacheEntry {
                prompt_hash: prompt_hash.clone(),
                response: response.clone(),
                cached_at: now_millis(),
                hit_count: 0,
            },
        );

        let event =
            self.build_llm_event(&request, &response, &prompt_hash, false, causal_vector)?;

        tracing::info!(
            target = "nexus.llm_proxy",
            request_id = %request.request_id,
            model = %request.model,
            tokens_in = %response.input_tokens,
            tokens_out = %response.output_tokens,
            cost_cents = %response.cost_cents,
            latency_ms = %response.latency_ms,
            cached = false,
            "LLM call proxied"
        );

        Ok((response, event))
    }

    /// Generate a structured execution plan from intent keywords when no LLM is available.
    /// Produces realistic JSON arrays with action_type, target, and parameters.
    async fn simulate_api_call(&self, request: &LlmRequest) -> Result<LlmResponse, ProxyError> {
        let input_tokens = request.prompt.len() as u64 / 4;
        let output_tokens = request.max_tokens.min(4096);

        let cost_cents = self.estimate_cost(&request.model, output_tokens);

        // Generate a structured plan based on keywords in the intent prompt
        let prompt_lower = request.prompt.to_lowercase();
        let mut steps: Vec<serde_json::Value> = Vec::new();

        let has_read = prompt_lower.contains("read")
            || prompt_lower.contains("analyze")
            || prompt_lower.contains("audit")
            || prompt_lower.contains("review")
            || prompt_lower.contains("inspect");
        let has_search = prompt_lower.contains("grep")
            || prompt_lower.contains("search")
            || prompt_lower.contains("find")
            || prompt_lower.contains("locate");
        let has_write = prompt_lower.contains("write")
            || prompt_lower.contains("report")
            || prompt_lower.contains("create")
            || prompt_lower.contains("generate")
            || prompt_lower.contains("output")
            || prompt_lower.contains("save");
        let has_run = prompt_lower.contains("run")
            || prompt_lower.contains("test")
            || prompt_lower.contains("build")
            || prompt_lower.contains("compile")
            || prompt_lower.contains("execute")
            || prompt_lower.contains("command");

        // First, read input files if intent mentions reading/analysis
        if has_read {
            steps.push(serde_json::json!({
                "action_type": "read_file",
                "target": "README.md",
                "parameters": {"encoding": "utf-8"}
            }));
        }

        // Search for patterns if intent mentions searching
        if has_search {
            steps.push(serde_json::json!({
                "action_type": "grep",
                "target": ".",
                "parameters": {
                    "pattern": "TODO|FIXME|HACK|BUG",
                    "include": "*.{rs,py,js,ts}"
                }
            }));
        }

        // Run analysis/execution if intent mentions running
        if has_run {
            steps.push(serde_json::json!({
                "action_type": "run_command",
                "target": ".",
                "parameters": {"command": "echo analysis complete"}
            }));
        }

        // Write output if intent mentions writing
        if has_write {
            steps.push(serde_json::json!({
                "action_type": "write_file",
                "target": "output.md",
                "parameters": {"encoding": "utf-8", "append": false}
            }));
        }

        // Default fallback: at minimum, grep the README
        if steps.is_empty() {
            steps.push(serde_json::json!({
                "action_type": "grep",
                "target": "README.md",
                "parameters": {"pattern": "Nexus"}
            }));
            steps.push(serde_json::json!({
                "action_type": "read_file",
                "target": "README.md",
                "parameters": {"encoding": "utf-8"}
            }));
        }

        let content = serde_json::to_string_pretty(&steps)
            .unwrap_or_else(|_| "[]".to_string());

        tracing::info!(
            target = "nexus.llm_proxy",
            steps = %steps.len(),
            "Simulated structured plan generated"
        );

        Ok(LlmResponse {
            request_id: request.request_id.clone(),
            model: request.model.clone(),
            content,
            input_tokens,
            output_tokens,
            cost_cents,
            response_hash: compute_hash(b"simulated_structured_response"),
            latency_ms: 50,
        })
    }

    /// Real API call routed by model prefix: gpt → OpenAI, claude → Anthropic.
    async fn call_real_api(&self, request: &LlmRequest) -> Result<LlmResponse, ProxyError> {
        let model_lower = request.model.to_lowercase();

        if model_lower.contains("claude") {
            self.call_anthropic_api(request).await
        } else if model_lower.starts_with("deepseek") {
            self.call_deepseek_api(request).await
        } else if model_lower.starts_with("gpt")
            || model_lower.starts_with("o1")
            || model_lower.starts_with("o3")
        {
            self.call_openai_api(request).await
        } else {
            Err(ProxyError::ModelNotAvailable(request.model.clone()))
        }
    }

    async fn call_openai_api(&self, request: &LlmRequest) -> Result<LlmResponse, ProxyError> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| ProxyError::ApiError("OPENAI_API_KEY not set".into()))?;

        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "model": request.model,
            "messages": [
                {"role": "user", "content": request.prompt}
            ],
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
        });

        let resp = client
            .post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProxyError::ApiError(format!("OpenAI request failed: {}", e)))?;

        if resp.status() == 429 {
            return Err(ProxyError::RateLimited);
        }

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProxyError::ApiError(format!(
                "OpenAI HTTP {}: {}",
                status, text
            )));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProxyError::ApiError(format!("OpenAI response parse: {}", e)))?;

        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let input_tokens = data["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
        let output_tokens = data["usage"]["completion_tokens"].as_u64().unwrap_or(0);
        let cost_cents = self.estimate_openai_cost(&request.model, input_tokens, output_tokens);

        let response_hash = compute_hash(content.as_bytes());

        Ok(LlmResponse {
            request_id: request.request_id.clone(),
            model: request.model.clone(),
            content,
            input_tokens,
            output_tokens,
            cost_cents,
            response_hash,
            latency_ms: 0,
        })
    }

    async fn call_deepseek_api(&self, request: &LlmRequest) -> Result<LlmResponse, ProxyError> {
        let api_key = std::env::var("DEEPSEEK_API_KEY")
            .map_err(|_| ProxyError::ApiError("DEEPSEEK_API_KEY not set".into()))?;

        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "model": request.model,
            "messages": [
                {"role": "user", "content": request.prompt}
            ],
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
        });

        let resp = client
            .post("https://api.deepseek.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProxyError::ApiError(format!("DeepSeek request failed: {}", e)))?;

        if resp.status() == 429 {
            return Err(ProxyError::RateLimited);
        }

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProxyError::ApiError(format!(
                "DeepSeek HTTP {}: {}",
                status, text
            )));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProxyError::ApiError(format!("DeepSeek response parse: {}", e)))?;

        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let input_tokens = data["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
        let output_tokens = data["usage"]["completion_tokens"].as_u64().unwrap_or(0);
        let cost_cents = self.estimate_deepseek_cost(&request.model, input_tokens, output_tokens);

        let response_hash = compute_hash(content.as_bytes());

        Ok(LlmResponse {
            request_id: request.request_id.clone(),
            model: request.model.clone(),
            content,
            input_tokens,
            output_tokens,
            cost_cents,
            response_hash,
            latency_ms: 0,
        })
    }

    async fn call_anthropic_api(&self, request: &LlmRequest) -> Result<LlmResponse, ProxyError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| ProxyError::ApiError("ANTHROPIC_API_KEY not set".into()))?;

        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "messages": [
                {"role": "user", "content": request.prompt}
            ],
        });

        let resp = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProxyError::ApiError(format!("Anthropic request failed: {}", e)))?;

        if resp.status() == 429 {
            return Err(ProxyError::RateLimited);
        }

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProxyError::ApiError(format!(
                "Anthropic HTTP {}: {}",
                status, text
            )));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProxyError::ApiError(format!("Anthropic response parse: {}", e)))?;

        let content = data["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let input_tokens = data["usage"]["input_tokens"].as_u64().unwrap_or(0);
        let output_tokens = data["usage"]["output_tokens"].as_u64().unwrap_or(0);
        let cost_cents = self.estimate_claude_cost(&request.model, input_tokens, output_tokens);

        let response_hash = compute_hash(content.as_bytes());

        Ok(LlmResponse {
            request_id: request.request_id.clone(),
            model: request.model.clone(),
            content,
            input_tokens,
            output_tokens,
            cost_cents,
            response_hash,
            latency_ms: 0,
        })
    }

    fn estimate_openai_cost(&self, model: &str, input_tokens: u64, output_tokens: u64) -> u64 {
        let (input_per_1m, output_per_1m): (f64, f64) = match model {
            m if m.starts_with("gpt-4o") => (2.50, 10.00),
            m if m.starts_with("gpt-4-turbo") => (10.00, 30.00),
            m if m.starts_with("gpt-4") => (30.00, 60.00),
            m if m.starts_with("gpt-3.5") => (0.50, 1.50),
            m if m.starts_with("o1") => (15.00, 60.00),
            m if m.starts_with("o3") => (10.00, 40.00),
            _ => (2.50, 10.00),
        };
        let cost = (input_tokens as f64 / 1_000_000.0) * input_per_1m
            + (output_tokens as f64 / 1_000_000.0) * output_per_1m;
        (cost * 100.0).ceil() as u64
    }

    fn estimate_claude_cost(&self, model: &str, input_tokens: u64, output_tokens: u64) -> u64 {
        let (input_per_1m, output_per_1m): (f64, f64) = match model {
            m if m.contains("claude-3-5") || m.contains("claude-3.5") => (3.00, 15.00),
            m if m.contains("claude-3-opus") => (15.00, 75.00),
            m if m.contains("claude-3") => (3.00, 15.00),
            _ => (3.00, 15.00),
        };
        let cost = (input_tokens as f64 / 1_000_000.0) * input_per_1m
            + (output_tokens as f64 / 1_000_000.0) * output_per_1m;
        (cost * 100.0).ceil() as u64
    }

    fn estimate_deepseek_cost(&self, model: &str, input_tokens: u64, output_tokens: u64) -> u64 {
        let (input_per_1m, output_per_1m): (f64, f64) = match model {
            m if m.contains("deepseek-chat") || m.contains("deepseek-v3") => (0.27, 1.10),
            m if m.contains("deepseek-reasoner") || m.contains("deepseek-r1") => (0.55, 2.19),
            _ => (0.27, 1.10),
        };
        let cost = (input_tokens as f64 / 1_000_000.0) * input_per_1m
            + (output_tokens as f64 / 1_000_000.0) * output_per_1m;
        (cost * 100.0).ceil() as u64
    }

    fn estimate_cost(&self, model: &str, tokens: u64) -> u64 {
        let cost_per_1k: f64 = match model {
            m if m.contains("claude") => 0.015,
            m if m.contains("deepseek") => 0.001,
            m if m.contains("gpt-4") => 0.03,
            m if m.contains("gpt-3.5") => 0.002,
            _ => 0.01,
        };
        ((tokens as f64 / 1000.0) * cost_per_1k * 100.0).ceil() as u64
    }

    fn build_llm_event(
        &self,
        request: &LlmRequest,
        response: &LlmResponse,
        _prompt_hash: &str,
        _cached: bool,
        causal_vector: &CausalVector,
    ) -> Result<NexusEvent, ProxyError> {
        let mut cv = causal_vector.clone();
        cv.increment(request.session_id);

        let event = NexusEvent::new(
            EventType::PlanProposed {
                plan: ExecutionPlan {
                    plan_id: format!("plan_{}", request.request_id),
                    tasks: vec![],
                    estimated_tokens: response.output_tokens,
                    estimated_cost_cents: response.cost_cents,
                },
                model: response.model.clone(),
                prompt_tokens: response.input_tokens,
                completion_tokens: response.output_tokens,
            },
            request.session_id,
            cv,
            None,
        );

        Ok(event)
    }

    pub fn cache_stats(&self) -> CacheStats {
        CacheStats {
            entries: self.cache.len(),
            total_hits: self.cache.values().map(|e| e.hit_count).sum(),
        }
    }

    /// Persist cache entries as LLM call records for recovery.
    /// On restart, the event log provides immutable cache — no re-calling APIs.
    pub fn persist_to_events(&self, session_id: SessionId) -> Vec<NexusEvent> {
        self.cache
            .iter()
            .map(|(prompt_hash, entry)| {
                let mut cv = CausalVector::new();
                cv.increment(session_id);

                NexusEvent {
                    event_id: generate_event_id(),
                    event_type: EventType::MemoryConsolidated {
                        memory_ids: vec![format!("llm_cache:{}", &prompt_hash[..16])],
                    },
                    session_id,
                    trace_id: generate_trace_id(),
                    parent_event_id: None,
                    causal_vector: cv,
                    payload: serde_json::to_vec(&entry.response).unwrap_or_default(),
                    payload_hash: prompt_hash.clone(),
                    event_timestamp: entry.cached_at,
                    nonce: generate_nonce(),
                    integrity_hash: String::new(),
                }
            })
            .collect()
    }

    /// Restore cache from persisted LLM call records in the event log.
    pub fn restore_from_events(&mut self, events: &[NexusEvent]) -> usize {
        let mut restored = 0;
        for event in events {
            if let EventType::MemoryConsolidated { memory_ids } = &event.event_type {
                for id in memory_ids {
                    if id.starts_with("llm_cache:") {
                        if let Ok(response) = serde_json::from_slice::<LlmResponse>(&event.payload)
                        {
                            self.cache.insert(
                                event.payload_hash.clone(),
                                LlmCacheEntry {
                                    prompt_hash: event.payload_hash.clone(),
                                    response,
                                    cached_at: event.event_timestamp,
                                    hit_count: 1,
                                },
                            );
                            restored += 1;
                        }
                    }
                }
            }
        }
        tracing::info!(
            target = "nexus.llm_proxy",
            restored_entries = %restored,
            "LLM cache restored from event log"
        );
        restored
    }
}

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub entries: usize,
    pub total_hits: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("Budget exceeded: need {required} cents, have {remaining} cents")]
    BudgetExceeded { required: u64, remaining: u64 },

    #[error("API error: {0}")]
    ApiError(String),

    #[error("Rate limited")]
    RateLimited,

    #[error("Model not available: {0}")]
    ModelNotAvailable(String),
}

impl BudgetState {
    pub fn can_afford(&self, estimated_cents: u64) -> bool {
        self.consumed_cents.saturating_add(estimated_cents) <= self.budget_limit_cents
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_proxy_cache_hit() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut proxy = LlmProxy::new(b"test-key-32-bytes-long-key!!!".to_vec());
            let mut budget = BudgetState::default();
            let sid = SessionId::from_bytes([1u8; 16]);

            let req = LlmRequest {
                request_id: "req_001".into(),
                session_id: sid,
                model: "claude-3.5-sonnet".into(),
                prompt: "test prompt".into(),
                max_tokens: 100,
                temperature: 0.7,
            };

            // First call — simulate API
            let cv = CausalVector::new();
            let (resp1, _) = proxy
                .proxy_call(req.clone(), &mut budget, &cv)
                .await
                .unwrap();

            // Second call — should be cached
            let (resp2, _) = proxy.proxy_call(req, &mut budget, &cv).await.unwrap();

            assert_eq!(
                resp1.response_hash, resp2.response_hash,
                "Cached response must be identical"
            );
            assert!(proxy.cache_stats().entries >= 1);
        });
    }

    #[test]
    fn test_budget_enforcement() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut proxy = LlmProxy::new(b"test-key-32-bytes-long-key!!!".to_vec());
            let mut budget = BudgetState {
                budget_limit_cents: 1,
                ..Default::default()
            };
            let sid = SessionId::from_bytes([1u8; 16]);

            let req = LlmRequest {
                request_id: "req_002".into(),
                session_id: sid,
                model: "gpt-4o".into(),
                prompt: "expensive prompt".into(),
                max_tokens: 10000,
                temperature: 0.7,
            };

            let cv = CausalVector::new();
            let result = proxy.proxy_call(req, &mut budget, &cv).await;
            assert!(result.is_err());
        });
    }

    #[test]
    fn test_model_pricing() {
        let proxy = LlmProxy::new(b"key".to_vec());
        let gpt4_cost = proxy.estimate_cost("gpt-4o", 1000);
        let claude_cost = proxy.estimate_cost("claude-3.5-sonnet", 1000);
        assert!(
            gpt4_cost > claude_cost,
            "gpt-4 should be more expensive than claude"
        );
    }
}
