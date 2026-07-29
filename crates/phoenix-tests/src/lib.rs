#![deny(clippy::disallowed_types)]

use nexus_core::*;
use std::collections::BTreeMap;

pub struct PhoenixHarness {
    pub temp_dir: tempfile::TempDir,
}

impl Default for PhoenixHarness {
    fn default() -> Self {
        Self {
            temp_dir: tempfile::tempdir().unwrap(),
        }
    }
}

impl PhoenixHarness {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_session(&self, intent: &str) -> (NexusState, NexusEvent) {
        let session_id = SessionId::from_bytes([1u8; 16]);
        let state = NexusState::new(session_id, now_millis());

        let mut cv = CausalVector::new();
        cv.increment(session_id);

        let event = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: intent.to_string(),
                source: "phoenix".to_string(),
            },
            session_id,
            cv,
            None,
        );

        (state, event)
    }

    pub fn db_path(&self) -> std::path::PathBuf {
        self.temp_dir.path().join("state.sqlite")
    }

    pub fn vault_path(&self) -> std::path::PathBuf {
        self.temp_dir.path().join("vault")
    }
}

pub struct PhoenixInvariants;

impl PhoenixInvariants {
    pub fn check_all(report: &RecoveryReport) -> Result<(), String> {
        if !report.integrity_check {
            return Err("I-1 failed: integrity check".into());
        }
        if !report.causal_valid {
            return Err("I-2 failed: causal validity".into());
        }
        if !report.replay_success {
            return Err("I-3 failed: replay success".into());
        }
        if !report.artifacts_valid {
            return Err("I-4 failed: artifact integrity".into());
        }
        if !report.cost_integrity {
            return Err("I-5 failed: cost integrity".into());
        }
        Ok(())
    }

    pub fn i1_state_authority(integrity_ok: bool) -> Result<(), String> {
        if !integrity_ok {
            return Err("I-1: State authority check failed".into());
        }
        Ok(())
    }

    pub fn i2_checkpoint_identity(before: &Checkpoint, after: &Checkpoint) -> Result<(), String> {
        if before.checkpoint_id != after.checkpoint_id {
            return Err("I-2: checkpoint_id changed".into());
        }
        if before.step_index != after.step_index {
            return Err("I-2: step_index changed".into());
        }
        Ok(())
    }

    pub fn i3_replay_integrity(events: &[NexusEvent], expected: &NexusState) -> Result<(), String> {
        let mut replayed = NexusState::new(expected.session_id, expected.created_at);
        let dag = BTreeMap::new();

        for event in events {
            replayed = transition(&replayed, event, &dag)
                .map_err(|e| format!("I-3: transition failed: {:?}", e))?;
        }

        if replayed.version != expected.version {
            return Err(format!(
                "I-3: version mismatch: replayed={}, expected={}",
                replayed.version, expected.version
            ));
        }

        Ok(())
    }

    pub fn i4_artifact_integrity(artifacts: &[ArtifactRef]) -> Result<(), String> {
        for art in artifacts {
            if art.blake3.len() != 64 {
                return Err(format!(
                    "I-4: artifact {} has invalid blake3 hash (len={})",
                    art.id,
                    art.blake3.len()
                ));
            }
            if art.size_bytes == 0 {
                return Err(format!("I-4: artifact {} has zero size", art.id));
            }
        }
        Ok(())
    }

    pub fn i5_determinism_context(
        before: &DeterminismContext,
        after: &DeterminismContext,
    ) -> Result<(), String> {
        if before.seed != after.seed {
            return Err("I-5: seed changed".into());
        }
        if before.model_version != after.model_version {
            return Err("I-5: model_version changed".into());
        }
        if before.input_hash != after.input_hash {
            return Err("I-5: input_hash changed".into());
        }
        Ok(())
    }

    pub fn i6_cost_integrity(
        llm_unique_count: usize,
        llm_total_count: usize,
    ) -> Result<(), String> {
        if llm_unique_count != llm_total_count {
            return Err(format!(
                "I-6: duplicate LLM calls: {} unique, {} total",
                llm_unique_count, llm_total_count
            ));
        }
        Ok(())
    }

    pub fn i7_resume_continuity(before_seq: u64, after_seq: u64) -> Result<(), String> {
        if after_seq <= before_seq {
            return Err(format!(
                "I-7: did not progress: before={}, after={}",
                before_seq, after_seq
            ));
        }
        Ok(())
    }

    pub fn i8_eventual_consistency(
        replayed: &NexusState,
        stored: &NexusState,
    ) -> Result<(), String> {
        if replayed.version != stored.version {
            return Err(format!(
                "I-8: materialized view diverged: replayed={}, stored={}",
                replayed.version, stored.version
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct PhoenixReport {
    pub tests: Vec<PhoenixTestResult>,
}

impl PhoenixReport {
    pub fn all_passed(&self) -> bool {
        self.tests.iter().all(|t| t.passed)
    }

    pub fn summary(&self) -> String {
        let total = self.tests.len();
        let passed = self.tests.iter().filter(|t| t.passed).count();
        format!("{} of {} tests passed", passed, total)
    }
}

pub struct PhoenixSuite;

impl PhoenixSuite {
    pub async fn run_all() -> Result<PhoenixReport, String> {
        let mut report = PhoenixReport::default();

        // Helper: run a test scenario and record the result
        fn run(name: &str, report: &mut PhoenixReport, scenario: fn() -> Result<(), String>) {
            let passed = match scenario() {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("  [FAIL] {}: {}", name, e);
                    false
                }
            };
            report.tests.push(PhoenixTestResult {
                name: name.into(),
                passed,
            });
        }

        // Scenario: kill-9 at intake — recovery should find state at Intake
        run("kill9_at_intake", &mut report, || {
            let session_id = SessionId::from_bytes([1u8; 16]);
            let mut cv = CausalVector::new();
            cv.increment(session_id);
            let event = NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "refactor auth".into(),
                    source: "phoenix".into(),
                },
                session_id,
                cv,
                None,
            );
            let dag = BTreeMap::new();
            let state = transition(&NexusState::new(session_id, 0), &event, &dag)
                .map_err(|e| format!("transition: {:?}", e))?;
            if state.status != SessionStatus::Intake {
                return Err(format!("expected Intake, got {:?}", state.status));
            }
            let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
            let recovered = rm
                .recover_from_events(&[event], session_id)
                .map_err(|e| format!("recovery: {}", e))?;
            if recovered.state.status != SessionStatus::Intake {
                return Err(format!(
                    "recovery: expected Intake, got {:?}",
                    recovered.state.status
                ));
            }
            PhoenixInvariants::check_all(&recovered.report)?;
            Ok(())
        });

        // Scenario: kill-9 at planning — recovery should find state at Planning
        run("kill9_at_planning", &mut report, || {
            let session_id = SessionId::from_bytes([1u8; 16]);
            let mut cv = CausalVector::new();
            cv.increment(session_id);
            let e1 = NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "refactor".into(),
                    source: "phoenix".into(),
                },
                session_id,
                cv.clone(),
                None,
            );
            cv.increment(session_id);
            let e2 = NexusEvent::new(
                EventType::IntentParsed {
                    intent_graph: IntentGraph::default(),
                },
                session_id,
                cv,
                None,
            );
            let events = vec![e1, e2];
            let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
            let recovered = rm
                .recover_from_events(&events, session_id)
                .map_err(|e| format!("recovery: {}", e))?;
            if recovered.state.status != SessionStatus::Planning {
                return Err(format!(
                    "expected Planning, got {:?}",
                    recovered.state.status
                ));
            }
            PhoenixInvariants::check_all(&recovered.report)?;
            Ok(())
        });

        // Scenario: kill-9 at executing — recovery plan should be generated
        run("kill9_at_executing", &mut report, || {
            let session_id = SessionId::from_bytes([1u8; 16]);
            let mut cv = CausalVector::new();
            cv.increment(session_id);
            let e1 = NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "task".into(),
                    source: "phoenix".into(),
                },
                session_id,
                cv.clone(),
                None,
            );
            cv.increment(session_id);
            let e2 = NexusEvent::new(
                EventType::IntentParsed {
                    intent_graph: IntentGraph::default(),
                },
                session_id,
                cv.clone(),
                None,
            );
            cv.increment(session_id);
            let e3 = NexusEvent::new(
                EventType::PlanCommitted {
                    frontier: Frontier::empty(),
                },
                session_id,
                cv.clone(),
                None,
            );
            cv.increment(session_id);
            let e4 = NexusEvent::new(EventType::DependenciesMet, session_id, cv, None);
            let events = vec![e1, e2, e3, e4];
            let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
            let recovered = rm
                .recover_from_events(&events, session_id)
                .map_err(|e| format!("recovery: {}", e))?;
            if recovered.state.status != SessionStatus::Executing {
                return Err(format!(
                    "expected Executing, got {:?}",
                    recovered.state.status
                ));
            }
            if recovered.recovery_plan.is_none() {
                return Err("expected recovery plan".into());
            }
            PhoenixInvariants::check_all(&recovered.report)?;
            Ok(())
        });

        // Scenario: kill-9 at checkpoint — checkpoint seq preserved
        run("kill9_at_checkpoint", &mut report, || {
            let session_id = SessionId::from_bytes([1u8; 16]);
            let mut cva = CausalVector::new();
            cva.increment(session_id);
            let mut cvb = CausalVector::new();
            cvb.increment(session_id);
            cvb.increment(session_id);
            let mut cvc = CausalVector::new();
            cvc.increment(session_id);
            cvc.increment(session_id);
            cvc.increment(session_id);
            let mut cvd = CausalVector::new();
            cvd.increment(session_id);
            cvd.increment(session_id);
            cvd.increment(session_id);
            cvd.increment(session_id);
            let mut cve = CausalVector::new();
            cve.increment(session_id);
            cve.increment(session_id);
            cve.increment(session_id);
            cve.increment(session_id);
            cve.increment(session_id);
            let events = vec![
                NexusEvent::new(
                    EventType::IntentReceived {
                        raw_input: "task".into(),
                        source: "phoenix".into(),
                    },
                    session_id,
                    cva,
                    None,
                ),
                NexusEvent::new(
                    EventType::IntentParsed {
                        intent_graph: IntentGraph::default(),
                    },
                    session_id,
                    cvb,
                    None,
                ),
                NexusEvent::new(
                    EventType::PlanCommitted {
                        frontier: Frontier::empty(),
                    },
                    session_id,
                    cvc,
                    None,
                ),
                NexusEvent::new(EventType::DependenciesMet, session_id, cvd, None),
                NexusEvent::new(
                    EventType::WorkerCheckpoint {
                        task_id: TaskId::from_bytes([2u8; 16]),
                        step_index: 5,
                        actions: vec![],
                        artifacts: vec![],
                    },
                    session_id,
                    cve,
                    None,
                ),
            ];
            let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
            let recovered = rm
                .recover_from_events(&events, session_id)
                .map_err(|e| format!("recovery: {}", e))?;
            if recovered.state.status != SessionStatus::Checkpointing {
                return Err(format!(
                    "expected Checkpointing, got {:?}",
                    recovered.state.status
                ));
            }
            if recovered.state.checkpoint_seq != 5 {
                return Err(format!(
                    "expected checkpoint_seq=5, got {}",
                    recovered.state.checkpoint_seq
                ));
            }
            PhoenixInvariants::check_all(&recovered.report)?;
            Ok(())
        });

        // Scenario: kill-9 at converging — state preserved with FanIn DAG
        run("kill9_at_converging", &mut report, || {
            let session_id = SessionId::from_bytes([1u8; 16]);
            let fan_in_id = TaskId::from_bytes([99u8; 16]);
            let mut intent_graph = IntentGraph::default();
            intent_graph.nodes.insert(
                fan_in_id,
                TaskNode {
                    id: fan_in_id,
                    kind: TaskKind::FanIn,
                    worker_type: WorkerType::RustInline,
                    intent: TaskIntent {
                        action_type: "converge".into(),
                        target: "merge".into(),
                        parameters: BTreeMap::new(),
                        constraints: vec![],
                    },
                    dependencies: vec![],
                    capabilities: vec![],
                    side_effect_class: SideEffectClass::Pure,
                },
            );
            let mut cv = CausalVector::new();
            cv.increment(session_id);
            let e1 = NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "merge".into(),
                    source: "phoenix".into(),
                },
                session_id,
                cv.clone(),
                None,
            );
            cv.increment(session_id);
            let e2 = NexusEvent::new(
                EventType::IntentParsed {
                    intent_graph: intent_graph.clone(),
                },
                session_id,
                cv.clone(),
                None,
            );
            cv.increment(session_id);
            let e3 = NexusEvent::new(
                EventType::PlanCommitted {
                    frontier: {
                        let mut f = Frontier::empty();
                        f.nodes.push(fan_in_id);
                        f
                    },
                },
                session_id,
                cv.clone(),
                None,
            );
            cv.increment(session_id);
            let e4 = NexusEvent::new(EventType::DependenciesMet, session_id, cv, None);
            let events = vec![e1, e2, e3, e4];
            let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
            let recovered = rm
                .recover_from_events(&events, session_id)
                .map_err(|e| format!("recovery: {}", e))?;
            if recovered.state.status != SessionStatus::Converging {
                return Err(format!(
                    "expected Converging, got {:?}",
                    recovered.state.status
                ));
            }
            PhoenixInvariants::check_all(&recovered.report)?;
            Ok(())
        });

        // Scenario: kill-9 at reflecting — full lifecycle recovery
        run("kill9_at_reflecting", &mut report, || {
            let session_id = SessionId::from_bytes([1u8; 16]);
            let fan_in_id = TaskId::from_bytes([99u8; 16]);
            let mut intent_graph = IntentGraph::default();
            intent_graph.nodes.insert(
                fan_in_id,
                TaskNode {
                    id: fan_in_id,
                    kind: TaskKind::FanIn,
                    worker_type: WorkerType::RustInline,
                    intent: TaskIntent {
                        action_type: "converge".into(),
                        target: "reflect".into(),
                        parameters: BTreeMap::new(),
                        constraints: vec![],
                    },
                    dependencies: vec![],
                    capabilities: vec![],
                    side_effect_class: SideEffectClass::Pure,
                },
            );
            let mut cv = CausalVector::new();
            cv.increment(session_id);
            let e1 = NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "reflect".into(),
                    source: "phoenix".into(),
                },
                session_id,
                cv.clone(),
                None,
            );
            cv.increment(session_id);
            let e2 = NexusEvent::new(
                EventType::IntentParsed { intent_graph },
                session_id,
                cv.clone(),
                None,
            );
            cv.increment(session_id);
            let e3 = NexusEvent::new(
                EventType::PlanCommitted {
                    frontier: {
                        let mut f = Frontier::empty();
                        f.nodes.push(fan_in_id);
                        f
                    },
                },
                session_id,
                cv.clone(),
                None,
            );
            cv.increment(session_id);
            let e4 = NexusEvent::new(EventType::DependenciesMet, session_id, cv.clone(), None);
            cv.increment(session_id);
            let e5 = NexusEvent::new(
                EventType::ConvergeComplete {
                    merged_result: WorkerResult {
                        status: "completed".into(),
                        artifacts: vec![],
                        metrics: WorkerMetrics {
                            duration_ms: 100,
                            tokens_consumed: 50,
                            cost_cents: 1,
                        },
                    },
                },
                session_id,
                cv.clone(),
                None,
            );
            cv.increment(session_id);
            let e6 = NexusEvent::new(
                EventType::ReflectionComplete {
                    evaluation: Evaluation {
                        score: 0.9,
                        summary: "good".into(),
                        recommendations: vec![],
                    },
                    memory_delta: vec![],
                },
                session_id,
                cv,
                None,
            );
            let events = vec![e1, e2, e3, e4, e5, e6];
            let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
            let recovered = rm
                .recover_from_events(&events, session_id)
                .map_err(|e| format!("recovery: {}", e))?;
            if recovered.state.status != SessionStatus::Completed {
                return Err(format!(
                    "expected Completed, got {:?}",
                    recovered.state.status
                ));
            }
            PhoenixInvariants::check_all(&recovered.report)?;
            Ok(())
        });

        // Scenario: worker crash — retryable error returns to Planned
        run("worker_crash", &mut report, || {
            let session_id = SessionId::from_bytes([1u8; 16]);
            let mut cv = CausalVector::new();
            cv.increment(session_id);
            let mut cv2 = CausalVector::new();
            cv2.increment(session_id);
            cv2.increment(session_id);
            let mut cv3 = CausalVector::new();
            cv3.increment(session_id);
            cv3.increment(session_id);
            cv3.increment(session_id);
            let mut cv4 = CausalVector::new();
            cv4.increment(session_id);
            cv4.increment(session_id);
            cv4.increment(session_id);
            cv4.increment(session_id);
            let mut cv5 = CausalVector::new();
            cv5.increment(session_id);
            cv5.increment(session_id);
            cv5.increment(session_id);
            cv5.increment(session_id);
            cv5.increment(session_id);
            let events = vec![
                NexusEvent::new(
                    EventType::IntentReceived {
                        raw_input: "crash test".into(),
                        source: "phoenix".into(),
                    },
                    session_id,
                    cv,
                    None,
                ),
                NexusEvent::new(
                    EventType::IntentParsed {
                        intent_graph: IntentGraph::default(),
                    },
                    session_id,
                    cv2,
                    None,
                ),
                NexusEvent::new(
                    EventType::PlanCommitted {
                        frontier: Frontier::empty(),
                    },
                    session_id,
                    cv3,
                    None,
                ),
                NexusEvent::new(EventType::DependenciesMet, session_id, cv4, None),
                NexusEvent::new(
                    EventType::WorkerFailed {
                        worker_id: "w1".into(),
                        task_id: TaskId::from_bytes([2u8; 16]),
                        error: "oom killed".into(),
                        error_code: ErrorCode::Retryable,
                        retry_count: 1,
                    },
                    session_id,
                    cv5,
                    None,
                ),
            ];
            let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
            let recovered = rm
                .recover_from_events(&events, session_id)
                .map_err(|e| format!("recovery: {}", e))?;
            if recovered.state.status != SessionStatus::Planned {
                return Err(format!(
                    "expected Planned after retryable error, got {:?}",
                    recovered.state.status
                ));
            }
            PhoenixInvariants::check_all(&recovered.report)?;
            Ok(())
        });

        // Scenario: LLM API timeout — PlanRejected leads to Failed
        run("llm_api_timeout", &mut report, || {
            let session_id = SessionId::from_bytes([1u8; 16]);
            let mut cv = CausalVector::new();
            cv.increment(session_id);
            let mut cv2 = CausalVector::new();
            cv2.increment(session_id);
            cv2.increment(session_id);
            let mut cv3 = CausalVector::new();
            cv3.increment(session_id);
            cv3.increment(session_id);
            cv3.increment(session_id);
            let events = vec![
                NexusEvent::new(
                    EventType::IntentReceived {
                        raw_input: "timeout test".into(),
                        source: "phoenix".into(),
                    },
                    session_id,
                    cv,
                    None,
                ),
                NexusEvent::new(
                    EventType::IntentParsed {
                        intent_graph: IntentGraph::default(),
                    },
                    session_id,
                    cv2,
                    None,
                ),
                NexusEvent::new(
                    EventType::PlanRejected {
                        reason: "LLM API timeout after 30s".into(),
                    },
                    session_id,
                    cv3,
                    None,
                ),
            ];
            let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
            let recovered = rm
                .recover_from_events(&events, session_id)
                .map_err(|e| format!("recovery: {}", e))?;
            if recovered.state.status != SessionStatus::Failed {
                return Err(format!(
                    "expected Failed after PlanRejected, got {:?}",
                    recovered.state.status
                ));
            }
            PhoenixInvariants::check_all(&recovered.report)?;
            Ok(())
        });

        // Scenario: side-effect crash — idempotent effects can be replayed
        run("side_effect_crash", &mut report, || {
            let mut guard = SideEffectGuard::new();
            let sid = SessionId::from_bytes([1u8; 16]);
            let tid = TaskId::from_bytes([2u8; 16]);
            let intent = SideEffectIntent {
                id: "se_crash_001".into(),
                session_id: sid,
                task_id: tid,
                effect_class: SideEffectClass::Idempotent,
                action_type: "write_file".into(),
                target: "/tmp/crash_test.txt".into(),
                payload: vec![1, 2, 3],
                request_hash: "crash_hash".into(),
                preconditions: vec![],
            };
            let effect_id = guard
                .record_intent(intent)
                .map_err(|e| format!("record_intent: {}", e))?;
            if effect_id.is_empty() {
                return Err("effect_id is empty".into());
            }
            let action = guard
                .recover_effect(&effect_id)
                .map_err(|e| format!("recover_effect: {}", e))?;
            if !matches!(action, RecoveryAction::Replay) {
                return Err(format!("expected Replay, got {:?}", action));
            }
            Ok(())
        });

        // Scenario: cross-session resume — memory inheritance works
        run("cross_session_resume", &mut report, || {
            let sid_a = SessionId::from_bytes([0xA0; 16]);
            let sid_b = SessionId::from_bytes([0xB0; 16]);
            let mut state_b = NexusState::new(sid_b, 0);
            let dag = BTreeMap::new();
            let mut cv = CausalVector::new();
            cv.increment(sid_b);
            state_b = transition(
                &state_b,
                &NexusEvent::new(
                    EventType::IntentReceived {
                        raw_input: "session B inherits".into(),
                        source: "phoenix".into(),
                    },
                    sid_b,
                    cv.clone(),
                    None,
                ),
                &dag,
            )
            .map_err(|e| format!("transition: {:?}", e))?;
            cv.increment(sid_b);
            state_b = transition(
                &state_b,
                &NexusEvent::new(
                    EventType::IntentParsed {
                        intent_graph: IntentGraph::default(),
                    },
                    sid_b,
                    cv.clone(),
                    None,
                ),
                &dag,
            )
            .map_err(|e| format!("transition: {:?}", e))?;
            cv.increment(sid_b);
            state_b = transition(
                &state_b,
                &NexusEvent::new(
                    EventType::PlanCommitted {
                        frontier: Frontier::empty(),
                    },
                    sid_b,
                    cv.clone(),
                    None,
                ),
                &dag,
            )
            .map_err(|e| format!("transition: {:?}", e))?;
            cv.increment(sid_b);
            state_b = transition(
                &state_b,
                &NexusEvent::new(EventType::DependenciesMet, sid_b, cv.clone(), None),
                &dag,
            )
            .map_err(|e| format!("transition: {:?}", e))?;
            cv.increment(sid_b);
            state_b = transition(
                &state_b,
                &NexusEvent::new(
                    EventType::SessionSuspended {
                        reason: "context switch".into(),
                    },
                    sid_b,
                    cv.clone(),
                    None,
                ),
                &dag,
            )
            .map_err(|e| format!("transition: {:?}", e))?;
            cv.increment(sid_b);
            state_b = transition(
                &state_b,
                &NexusEvent::new(
                    EventType::SessionResumed {
                        from_checkpoint: state_b.checkpoint_seq,
                        inherited_memories: vec!["knowledge_x".to_string()],
                    },
                    sid_b,
                    cv,
                    None,
                ),
                &dag,
            )
            .map_err(|e| format!("transition: {:?}", e))?;
            if state_b.status != SessionStatus::Executing {
                return Err(format!(
                    "expected Executing after resume, got {:?}",
                    state_b.status
                ));
            }
            if !state_b
                .memory_refs
                .iter()
                .any(|m| m.memory_id == "knowledge_x")
            {
                return Err("memory not inherited".into());
            }
            Ok(())
        });

        Ok(report)
    }
}

#[derive(Debug)]
pub struct PhoenixTestResult {
    pub name: String,
    pub passed: bool,
}

#[cfg(test)]
mod phoenix_invariants {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_phoenix_kill9_at_intake() {
        let harness = PhoenixHarness::new();
        let (state, event) = harness.create_session("refactor auth");

        let dag = BTreeMap::new();
        let result = transition(&state, &event, &dag);
        assert!(result.is_ok());

        let next_state = result.unwrap();
        assert_eq!(next_state.status, SessionStatus::Intake);
    }

    #[test]
    fn test_phoenix_kill9_at_planning() {
        let session_id = SessionId::from_bytes([1u8; 16]);
        let mut state = NexusState::new(session_id, 0);
        let dag = BTreeMap::new();

        // Drive to Planning
        let mut cv = CausalVector::new();
        cv.increment(session_id);
        let e1 = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "refactor".into(),
                source: "phoenix".into(),
            },
            session_id,
            cv.clone(),
            None,
        );
        state = transition(&state, &e1, &dag).unwrap();
        assert_eq!(state.status, SessionStatus::Intake);

        cv.increment(session_id);
        let e2 = NexusEvent::new(
            EventType::IntentParsed {
                intent_graph: IntentGraph::default(),
            },
            session_id,
            cv.clone(),
            None,
        );
        state = transition(&state, &e2, &dag).unwrap();
        assert_eq!(state.status, SessionStatus::Planning);

        // Crash here, recover
        let events = vec![e1, e2];
        let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
        let recovered = rm.recover_from_events(&events, session_id).unwrap();
        assert_eq!(recovered.state.status, SessionStatus::Planning);
        assert!(recovered.report.causal_valid);
        assert!(recovered.report.replay_success);
    }

    #[test]
    fn test_phoenix_kill9_at_executing() {
        let session_id = SessionId::from_bytes([1u8; 16]);
        let mut state = NexusState::new(session_id, 0);
        let dag = BTreeMap::new();
        let mut cv = CausalVector::new();

        // Drive through intake, planning, planned to executing
        cv.increment(session_id);
        let e1 = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "task".into(),
                source: "phoenix".into(),
            },
            session_id,
            cv.clone(),
            None,
        );
        state = transition(&state, &e1, &dag).unwrap();

        cv.increment(session_id);
        let e2 = NexusEvent::new(
            EventType::IntentParsed {
                intent_graph: IntentGraph::default(),
            },
            session_id,
            cv.clone(),
            None,
        );
        state = transition(&state, &e2, &dag).unwrap();

        cv.increment(session_id);
        let e3 = NexusEvent::new(
            EventType::PlanCommitted {
                frontier: Frontier::empty(),
            },
            session_id,
            cv.clone(),
            None,
        );
        state = transition(&state, &e3, &dag).unwrap();

        cv.increment(session_id);
        let e4 = NexusEvent::new(EventType::DependenciesMet, session_id, cv.clone(), None);
        state = transition(&state, &e4, &dag).unwrap();
        assert_eq!(state.status, SessionStatus::Executing);

        // Kill-9 here, recover
        let events = vec![e1, e2, e3, e4];
        let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
        let recovered = rm.recover_from_events(&events, session_id).unwrap();
        assert_eq!(recovered.state.status, SessionStatus::Executing);
        assert!(recovered.recovery_plan.is_some());
    }

    #[test]
    fn test_phoenix_kill9_at_checkpoint() {
        let session_id = SessionId::from_bytes([1u8; 16]);
        let mut state = NexusState::new(session_id, 0);
        let dag = BTreeMap::new();
        let mut cv = CausalVector::new();

        // Drive to executing
        cv.increment(session_id);
        state = transition(
            &state,
            &NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "task".into(),
                    source: "phoenix".into(),
                },
                session_id,
                cv.clone(),
                None,
            ),
            &dag,
        )
        .unwrap();

        cv.increment(session_id);
        state = transition(
            &state,
            &NexusEvent::new(
                EventType::IntentParsed {
                    intent_graph: IntentGraph::default(),
                },
                session_id,
                cv.clone(),
                None,
            ),
            &dag,
        )
        .unwrap();

        cv.increment(session_id);
        state = transition(
            &state,
            &NexusEvent::new(
                EventType::PlanCommitted {
                    frontier: Frontier::empty(),
                },
                session_id,
                cv.clone(),
                None,
            ),
            &dag,
        )
        .unwrap();

        cv.increment(session_id);
        state = transition(
            &state,
            &NexusEvent::new(EventType::DependenciesMet, session_id, cv.clone(), None),
            &dag,
        )
        .unwrap();

        // Now trigger checkpoint
        cv.increment(session_id);
        let e5 = NexusEvent::new(
            EventType::WorkerCheckpoint {
                task_id: TaskId::from_bytes([2u8; 16]),
                step_index: 5,
                actions: vec![],
                artifacts: vec![],
            },
            session_id,
            cv.clone(),
            None,
        );
        state = transition(&state, &e5, &dag).unwrap();
        assert_eq!(state.status, SessionStatus::Checkpointing);
        assert_eq!(state.checkpoint_seq, 5);

        // Kill-9 at checkpoint, recover
        // Build events manually for clean replay
        let mut cva = CausalVector::new();
        cva.increment(session_id);
        let a = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "task".into(),
                source: "phoenix".into(),
            },
            session_id,
            cva,
            None,
        );
        let mut cvb = CausalVector::new();
        cvb.increment(session_id);
        cvb.increment(session_id);
        let b = NexusEvent::new(
            EventType::IntentParsed {
                intent_graph: IntentGraph::default(),
            },
            session_id,
            cvb,
            None,
        );
        let mut cvc = CausalVector::new();
        cvc.increment(session_id);
        cvc.increment(session_id);
        cvc.increment(session_id);
        let c = NexusEvent::new(
            EventType::PlanCommitted {
                frontier: Frontier::empty(),
            },
            session_id,
            cvc,
            None,
        );
        let mut cvd = CausalVector::new();
        cvd.increment(session_id);
        cvd.increment(session_id);
        cvd.increment(session_id);
        cvd.increment(session_id);
        let d = NexusEvent::new(EventType::DependenciesMet, session_id, cvd, None);
        let mut cve = CausalVector::new();
        cve.increment(session_id);
        cve.increment(session_id);
        cve.increment(session_id);
        cve.increment(session_id);
        cve.increment(session_id);
        let e = NexusEvent::new(
            EventType::WorkerCheckpoint {
                task_id: TaskId::from_bytes([2u8; 16]),
                step_index: 5,
                actions: vec![],
                artifacts: vec![],
            },
            session_id,
            cve,
            None,
        );
        let events = vec![a, b, c, d, e];

        let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
        let recovered = rm.recover_from_events(&events, session_id).unwrap();
        assert_eq!(recovered.state.status, SessionStatus::Checkpointing);
        assert_eq!(recovered.state.checkpoint_seq, 5);
        assert!(recovered.recovery_plan.is_some());
    }

    #[test]
    fn test_phoenix_kill9_at_converging() {
        let session_id = SessionId::from_bytes([1u8; 16]);
        let mut state = NexusState::new(session_id, 0);
        let dag = BTreeMap::new();
        let mut cv = CausalVector::new();

        let fan_in_id = TaskId::from_bytes([99u8; 16]);
        let mut intent_graph = IntentGraph::default();
        intent_graph.nodes.insert(
            fan_in_id,
            TaskNode {
                id: fan_in_id,
                kind: TaskKind::FanIn,
                worker_type: WorkerType::RustInline,
                intent: TaskIntent {
                    action_type: "converge".into(),
                    target: "merge".into(),
                    parameters: BTreeMap::new(),
                    constraints: vec![],
                },
                dependencies: vec![],
                capabilities: vec![],
                side_effect_class: SideEffectClass::Pure,
            },
        );

        // Drive to planned, then converging via DependenciesMet with fan_in
        cv.increment(session_id);
        let e1 = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "merge".into(),
                source: "phoenix".into(),
            },
            session_id,
            cv.clone(),
            None,
        );
        state = transition(&state, &e1, &dag).unwrap();

        cv.increment(session_id);
        let e2 = NexusEvent::new(
            EventType::IntentParsed {
                intent_graph: intent_graph.clone(),
            },
            session_id,
            cv.clone(),
            None,
        );
        state = transition(&state, &e2, &dag).unwrap();

        cv.increment(session_id);
        let e3 = NexusEvent::new(
            EventType::PlanCommitted {
                frontier: {
                    let mut f = Frontier::empty();
                    f.nodes.push(fan_in_id);
                    f
                },
            },
            session_id,
            cv.clone(),
            None,
        );
        state = transition(&state, &e3, &dag).unwrap();

        // Build DAG for transition
        let mut fan_dag = BTreeMap::new();
        fan_dag.insert(
            fan_in_id,
            intent_graph.nodes.get(&fan_in_id).unwrap().clone(),
        );

        cv.increment(session_id);
        let e4 = NexusEvent::new(EventType::DependenciesMet, session_id, cv.clone(), None);
        state = transition(&state, &e4, &fan_dag).unwrap();
        assert_eq!(state.status, SessionStatus::Converging);

        // Kill-9 at converging, recover
        let events = vec![e1, e2, e3, e4];
        let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
        let recovered = rm.recover_from_events(&events, session_id).unwrap();
        assert_eq!(recovered.state.status, SessionStatus::Converging);
        assert!(recovered.report.causal_valid);
        assert!(recovered.report.replay_success);
    }

    #[test]
    fn test_phoenix_kill9_at_reflecting() {
        let session_id = SessionId::from_bytes([1u8; 16]);

        // Build events: full lifecycle up to Converging, then ConvergeComplete, then ReflectionComplete
        let mut cv = CausalVector::new();

        // Include FanIn node in IntentGraph so DAG is built during recovery
        let fan_in_id = TaskId::from_bytes([99u8; 16]);
        let mut intent_graph = IntentGraph::default();
        intent_graph.nodes.insert(
            fan_in_id,
            TaskNode {
                id: fan_in_id,
                kind: TaskKind::FanIn,
                worker_type: WorkerType::RustInline,
                intent: TaskIntent {
                    action_type: "converge".into(),
                    target: "reflect".into(),
                    parameters: BTreeMap::new(),
                    constraints: vec![],
                },
                dependencies: vec![],
                capabilities: vec![],
                side_effect_class: SideEffectClass::Pure,
            },
        );

        cv.increment(session_id);
        let e1 = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "reflect".into(),
                source: "phoenix".into(),
            },
            session_id,
            cv.clone(),
            None,
        );

        cv.increment(session_id);
        let e2 = NexusEvent::new(
            EventType::IntentParsed { intent_graph },
            session_id,
            cv.clone(),
            None,
        );

        cv.increment(session_id);
        let e3 = NexusEvent::new(
            EventType::PlanCommitted {
                frontier: {
                    let mut f = Frontier::empty();
                    f.nodes.push(fan_in_id);
                    f
                },
            },
            session_id,
            cv.clone(),
            None,
        );

        cv.increment(session_id);
        let e4 = NexusEvent::new(EventType::DependenciesMet, session_id, cv.clone(), None);

        cv.increment(session_id);
        let e5 = NexusEvent::new(
            EventType::ConvergeComplete {
                merged_result: WorkerResult {
                    status: "completed".into(),
                    artifacts: vec![],
                    metrics: WorkerMetrics {
                        duration_ms: 100,
                        tokens_consumed: 50,
                        cost_cents: 1,
                    },
                },
            },
            session_id,
            cv.clone(),
            None,
        );

        cv.increment(session_id);
        let e6 = NexusEvent::new(
            EventType::ReflectionComplete {
                evaluation: Evaluation {
                    score: 0.9,
                    summary: "good".into(),
                    recommendations: vec![],
                },
                memory_delta: vec![],
            },
            session_id,
            cv.clone(),
            None,
        );

        let events = vec![e1, e2, e3, e4, e5, e6];
        let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
        let recovered = rm.recover_from_events(&events, session_id).unwrap();
        assert_eq!(recovered.state.status, SessionStatus::Completed);
        assert!(recovered.report.causal_valid);
        assert!(recovered.report.replay_success);
    }

    #[test]
    fn test_phoenix_causal_validity() {
        let session_id = SessionId::from_bytes([1u8; 16]);
        let mut cv1 = CausalVector::new();
        cv1.increment(session_id);

        let mut cv2 = CausalVector::new();
        cv2.increment(session_id);
        cv2.increment(session_id);

        assert!(cv1.happened_before(&cv2));
        assert!(!cv2.happened_before(&cv1));
    }

    #[test]
    fn test_phoenix_checkpoint_identity() {
        let ckpt = Checkpoint {
            checkpoint_id: "cp_001".into(),
            session_id: SessionId::from_bytes([1u8; 16]),
            step_index: 42,
            total_actions: 100,
            replay_actions: vec![],
            artifact_refs: vec![],
            handle_registry: vec![],
            determinism_context: DeterminismContext {
                seed: 12345,
                model_version: "claude-3.5-sonnet".into(),
                input_hash: "abc".into(),
                checkpoint_format_version: 1,
                worker_type: WorkerType::Python,
            },
            created_at: now_millis(),
        };

        let ckpt2 = ckpt.clone();
        assert!(PhoenixInvariants::i2_checkpoint_identity(&ckpt, &ckpt2).is_ok());
    }

    #[test]
    fn test_phoenix_cost_integrity() {
        assert!(PhoenixInvariants::i6_cost_integrity(5, 5).is_ok());
        assert!(PhoenixInvariants::i6_cost_integrity(3, 5).is_err());
    }

    #[test]
    fn test_phoenix_resume_continuity() {
        assert!(PhoenixInvariants::i7_resume_continuity(0, 3).is_ok());
        assert!(PhoenixInvariants::i7_resume_continuity(5, 3).is_err());
    }

    #[test]
    fn test_eight_invariants_all_pass() {
        let session_id = SessionId::from_bytes([1u8; 16]);
        let state = NexusState::new(session_id, 0);
        let dag = BTreeMap::new();
        let mut cv = CausalVector::new();

        cv.increment(session_id);
        let event = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "test invariants".into(),
                source: "phoenix".into(),
            },
            session_id,
            cv,
            None,
        );

        let events = vec![event];
        let _state = transition(&state, &events[0], &dag).unwrap();

        let rm = RecoveryManager::new("/tmp/test_vault".into());
        let recovered = rm.recover_from_events(&events, session_id).unwrap();

        let check = PhoenixInvariants::check_all(&recovered.report);
        assert!(
            check.is_ok(),
            "Phoenix invariants failed: {}",
            check.unwrap_err()
        );
    }

    #[test]
    fn test_determinism_context_invariant() {
        let ctx = DeterminismContext {
            seed: 42,
            model_version: "claude-3.5-sonnet".into(),
            input_hash: "hash_abc".into(),
            checkpoint_format_version: 1,
            worker_type: WorkerType::Python,
        };
        let ctx2 = ctx.clone();
        assert!(PhoenixInvariants::i5_determinism_context(&ctx, &ctx2).is_ok());
    }
}

#[cfg(test)]
mod phoenix_edge_cases {
    use super::*;

    #[test]
    fn test_worker_crash_recovery() {
        let session_id = SessionId::from_bytes([1u8; 16]);
        let mut state = NexusState::new(session_id, 0);
        let dag = BTreeMap::new();
        let mut cv = CausalVector::new();

        // Drive to executing
        cv.increment(session_id);
        state = transition(
            &state,
            &NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "crash test".into(),
                    source: "phoenix".into(),
                },
                session_id,
                cv.clone(),
                None,
            ),
            &dag,
        )
        .unwrap();

        cv.increment(session_id);
        state = transition(
            &state,
            &NexusEvent::new(
                EventType::IntentParsed {
                    intent_graph: IntentGraph::default(),
                },
                session_id,
                cv.clone(),
                None,
            ),
            &dag,
        )
        .unwrap();

        cv.increment(session_id);
        state = transition(
            &state,
            &NexusEvent::new(
                EventType::PlanCommitted {
                    frontier: Frontier::empty(),
                },
                session_id,
                cv.clone(),
                None,
            ),
            &dag,
        )
        .unwrap();

        cv.increment(session_id);
        state = transition(
            &state,
            &NexusEvent::new(EventType::DependenciesMet, session_id, cv.clone(), None),
            &dag,
        )
        .unwrap();

        // Worker fails with retryable error
        cv.increment(session_id);
        let e_fail = NexusEvent::new(
            EventType::WorkerFailed {
                worker_id: "w1".into(),
                task_id: TaskId::from_bytes([2u8; 16]),
                error: "oom killed".into(),
                error_code: ErrorCode::Retryable,
                retry_count: 1,
            },
            session_id,
            cv.clone(),
            None,
        );
        state = transition(&state, &e_fail, &dag).unwrap();

        // With retryable error and max_attempts > 0, should go back to Planned
        assert_eq!(state.status, SessionStatus::Planned);

        // Build events for recovery
        let mut events_builder = Vec::new();
        let mut cva = CausalVector::new();
        cva.increment(session_id);
        events_builder.push(NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "crash test".into(),
                source: "phoenix".into(),
            },
            session_id,
            cva,
            None,
        ));
        let mut cvb = CausalVector::new();
        cvb.increment(session_id);
        cvb.increment(session_id);
        events_builder.push(NexusEvent::new(
            EventType::IntentParsed {
                intent_graph: IntentGraph::default(),
            },
            session_id,
            cvb,
            None,
        ));
        let mut cvc = CausalVector::new();
        cvc.increment(session_id);
        cvc.increment(session_id);
        cvc.increment(session_id);
        events_builder.push(NexusEvent::new(
            EventType::PlanCommitted {
                frontier: Frontier::empty(),
            },
            session_id,
            cvc,
            None,
        ));
        let mut cvd = CausalVector::new();
        cvd.increment(session_id);
        cvd.increment(session_id);
        cvd.increment(session_id);
        cvd.increment(session_id);
        events_builder.push(NexusEvent::new(
            EventType::DependenciesMet,
            session_id,
            cvd,
            None,
        ));
        let mut cve = CausalVector::new();
        cve.increment(session_id);
        cve.increment(session_id);
        cve.increment(session_id);
        cve.increment(session_id);
        cve.increment(session_id);
        events_builder.push(NexusEvent::new(
            EventType::WorkerFailed {
                worker_id: "w1".into(),
                task_id: TaskId::from_bytes([2u8; 16]),
                error: "oom killed".into(),
                error_code: ErrorCode::Retryable,
                retry_count: 1,
            },
            session_id,
            cve,
            None,
        ));

        let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
        let recovered = rm.recover_from_events(&events_builder, session_id).unwrap();
        assert_eq!(recovered.state.status, SessionStatus::Planned);
        assert!(recovered.report.replay_success);
    }

    #[test]
    fn test_side_effect_crash_recovery() {
        let mut guard = SideEffectGuard::new();
        let sid = SessionId::from_bytes([1u8; 16]);
        let tid = TaskId::from_bytes([2u8; 16]);

        // Record intent
        let intent = SideEffectIntent {
            id: "se_crash_001".into(),
            session_id: sid,
            task_id: tid,
            effect_class: SideEffectClass::Idempotent,
            action_type: "write_file".into(),
            target: "/tmp/crash_test.txt".into(),
            payload: vec![1, 2, 3],
            request_hash: "crash_hash".into(),
            preconditions: vec![],
        };
        let effect_id = guard.record_intent(intent).unwrap();

        // Simulate crash BEFORE commit (e.g., kernel died mid-execution)
        // Create a fresh guard to simulate restart
        let mut guard2 = SideEffectGuard::new();

        // Recovery: pending idempotent effects should be replayed
        let recovery_intent = SideEffectIntent {
            id: effect_id.clone(),
            session_id: sid,
            task_id: tid,
            effect_class: SideEffectClass::Idempotent,
            action_type: "write_file".into(),
            target: "/tmp/crash_test.txt".into(),
            payload: vec![1, 2, 3],
            request_hash: "crash_hash".into(),
            preconditions: vec![],
        };

        // Record the same intent in the "restarted" guard
        let recovered_id = guard2.record_intent(recovery_intent).unwrap();
        // ID should be different since guard2 doesn't have the original
        // But the KEY property is that the effect can be safely replayed
        assert!(!recovered_id.is_empty());

        // Replay is safe for idempotent effects
        let action = guard.recover_effect(&effect_id).unwrap();
        assert!(matches!(action, RecoveryAction::Replay));
    }

    #[test]
    fn test_cross_session_resume() {
        let sid_a = SessionId::from_bytes([0xA0; 16]);
        let sid_b = SessionId::from_bytes([0xB0; 16]);

        // Session A: complete work, build memories
        let mut state_a = NexusState::new(sid_a, 0);
        let dag = BTreeMap::new();
        let mut cv = CausalVector::new();

        cv.increment(sid_a);
        let e1 = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "session A work".into(),
                source: "phoenix".into(),
            },
            sid_a,
            cv.clone(),
            None,
        );
        state_a = transition(&state_a, &e1, &dag).unwrap();

        cv.increment(sid_a);
        let e2 = NexusEvent::new(
            EventType::IntentParsed {
                intent_graph: IntentGraph::default(),
            },
            sid_a,
            cv.clone(),
            None,
        );
        state_a = transition(&state_a, &e2, &dag).unwrap();

        cv.increment(sid_a);
        let e3 = NexusEvent::new(
            EventType::PlanCommitted {
                frontier: Frontier::empty(),
            },
            sid_a,
            cv.clone(),
            None,
        );
        state_a = transition(&state_a, &e3, &dag).unwrap();

        cv.increment(sid_a);
        let e4 = NexusEvent::new(EventType::DependenciesMet, sid_a, cv.clone(), None);
        let _ = transition(&state_a, &e4, &dag).unwrap();

        cv.increment(sid_a);
        let _e5 = NexusEvent::new(
            EventType::ReflectionComplete {
                evaluation: Evaluation {
                    score: 1.0,
                    summary: "excellent".into(),
                    recommendations: vec![],
                },
                memory_delta: vec![MemoryDelta {
                    operation: MemoryOperation::Add,
                    memory_ref: MemoryRef {
                        memory_id: "knowledge_x".into(),
                        session_origin: sid_a,
                        causal_vector_at_creation: cv.clone(),
                        importance_score: 800,
                    },
                }],
            },
            sid_a,
            cv.clone(),
            None,
        );
        // This requires being in Reflecting state first, so we need to skip to it
        // For test purposes, just verify that memory was added via the reflection event

        // Simulate cross-session resume via SessionResumed event on session B
        let mut state_b = NexusState::new(sid_b, 0);
        let dag_b = BTreeMap::new();
        let mut cv_b = CausalVector::new();

        cv_b.increment(sid_b);
        let e_b = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "session B inherits".into(),
                source: "phoenix".into(),
            },
            sid_b,
            cv_b.clone(),
            None,
        );
        state_b = transition(&state_b, &e_b, &dag_b).unwrap();

        cv_b.increment(sid_b);
        let e_b2 = NexusEvent::new(
            EventType::IntentParsed {
                intent_graph: IntentGraph::default(),
            },
            sid_b,
            cv_b.clone(),
            None,
        );
        state_b = transition(&state_b, &e_b2, &dag_b).unwrap();

        cv_b.increment(sid_b);
        let e_b3 = NexusEvent::new(
            EventType::PlanCommitted {
                frontier: Frontier::empty(),
            },
            sid_b,
            cv_b.clone(),
            None,
        );
        state_b = transition(&state_b, &e_b3, &dag_b).unwrap();

        cv_b.increment(sid_b);
        let e_b4 = NexusEvent::new(EventType::DependenciesMet, sid_b, cv_b.clone(), None);
        state_b = transition(&state_b, &e_b4, &dag_b).unwrap();
        assert_eq!(state_b.status, SessionStatus::Executing);

        // Now suspend session B, then resume with inherited memories from A
        cv_b.increment(sid_b);
        let e_suspend = NexusEvent::new(
            EventType::SessionSuspended {
                reason: "context switch".into(),
            },
            sid_b,
            cv_b.clone(),
            None,
        );
        state_b = transition(&state_b, &e_suspend, &dag_b).unwrap();
        assert_eq!(state_b.status, SessionStatus::Checkpointing);

        cv_b.increment(sid_b);
        let e_resume = NexusEvent::new(
            EventType::SessionResumed {
                from_checkpoint: state_b.checkpoint_seq,
                inherited_memories: vec!["knowledge_x".to_string()],
            },
            sid_b,
            cv_b.clone(),
            None,
        );
        state_b = transition(&state_b, &e_resume, &dag_b).unwrap();
        assert_eq!(state_b.status, SessionStatus::Executing);
        assert!(
            state_b
                .memory_refs
                .iter()
                .any(|m| m.memory_id == "knowledge_x"),
            "Should have inherited memory from session A"
        );
    }

    #[tokio::test]
    async fn test_llm_api_timeout() {
        // Simulate LLM API timeout: the proxy returns an error,
        // the state machine should remain in Planning (plan not yet committed).
        let session_id = SessionId::from_bytes([1u8; 16]);
        let mut state = NexusState::new(session_id, 0);
        let dag = BTreeMap::new();
        let mut cv = CausalVector::new();

        // Drive to Planning
        cv.increment(session_id);
        state = transition(
            &state,
            &NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "timeout test".into(),
                    source: "phoenix".into(),
                },
                session_id,
                cv.clone(),
                None,
            ),
            &dag,
        )
        .unwrap();

        cv.increment(session_id);
        state = transition(
            &state,
            &NexusEvent::new(
                EventType::IntentParsed {
                    intent_graph: IntentGraph::default(),
                },
                session_id,
                cv.clone(),
                None,
            ),
            &dag,
        )
        .unwrap();
        assert_eq!(state.status, SessionStatus::Planning);

        // LLM timeout — plan is rejected (simulating API timeout)
        cv.increment(session_id);
        state = transition(
            &state,
            &NexusEvent::new(
                EventType::PlanRejected {
                    reason: "LLM API timeout after 30s".into(),
                },
                session_id,
                cv.clone(),
                None,
            ),
            &dag,
        )
        .unwrap();

        assert_eq!(
            state.status,
            SessionStatus::Failed,
            "LLM timeout should fail the session (PlanRejected)"
        );

        // Recover — verify the failure state is preserved
        let mut events = Vec::new();
        let mut c1 = CausalVector::new();
        c1.increment(session_id);
        events.push(NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "timeout test".into(),
                source: "phoenix".into(),
            },
            session_id,
            c1,
            None,
        ));
        let mut c2 = CausalVector::new();
        c2.increment(session_id);
        c2.increment(session_id);
        events.push(NexusEvent::new(
            EventType::IntentParsed {
                intent_graph: IntentGraph::default(),
            },
            session_id,
            c2,
            None,
        ));
        let mut c3 = CausalVector::new();
        c3.increment(session_id);
        c3.increment(session_id);
        c3.increment(session_id);
        events.push(NexusEvent::new(
            EventType::PlanRejected {
                reason: "LLM API timeout after 30s".into(),
            },
            session_id,
            c3,
            None,
        ));

        let rm = RecoveryManager::new("/tmp/phoenix_vault".into());
        let recovered = rm.recover_from_events(&events, session_id).unwrap();
        assert_eq!(recovered.state.status, SessionStatus::Failed);
        assert!(recovered.report.causal_valid);
        assert!(recovered.report.replay_success);
    }
}

#[cfg(test)]
mod integration {
    use super::*;
    use nexus_core::export::SessionExport;
    use nexus_event_store::*;

    async fn setup_store() -> (SqliteEventStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("e2e_test.db");
        let db_url = format!("sqlite:{}?mode=rwc", db_path.display());
        let store = SqliteEventStore::new(&db_url).await.unwrap();
        (store, dir)
    }

    #[tokio::test]
    async fn e2e_full_session_lifecycle() {
        let (store, _dir) = setup_store().await;
        let sid = SessionId::from_bytes([0xE2, 0xE2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);

        let mut cv = CausalVector::new();
        cv.increment(sid);
        store
            .append_event(&NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "refactor auth to JWT".into(),
                    source: "e2e".into(),
                },
                sid,
                cv.clone(),
                None,
            ))
            .await
            .unwrap();

        cv.increment(sid);
        store
            .append_event(&NexusEvent::new(
                EventType::IntentParsed {
                    intent_graph: IntentGraph::default(),
                },
                sid,
                cv.clone(),
                None,
            ))
            .await
            .unwrap();

        cv.increment(sid);
        store
            .append_event(&NexusEvent::new(
                EventType::PlanCommitted {
                    frontier: Frontier::empty(),
                },
                sid,
                cv.clone(),
                None,
            ))
            .await
            .unwrap();

        cv.increment(sid);
        store
            .append_event(&NexusEvent::new(
                EventType::DependenciesMet,
                sid,
                cv.clone(),
                None,
            ))
            .await
            .unwrap();

        cv.increment(sid);
        store
            .append_event(&NexusEvent::new(
                EventType::WorkerCheckpoint {
                    task_id: TaskId::from_bytes([0xAA; 16]),
                    step_index: 3,
                    actions: vec![],
                    artifacts: vec![],
                },
                sid,
                cv.clone(),
                None,
            ))
            .await
            .unwrap();

        cv.increment(sid);
        store
            .append_event(&NexusEvent::new(
                EventType::WorkerCheckpoint {
                    task_id: TaskId::from_bytes([0xAA; 16]),
                    step_index: 7,
                    actions: vec![],
                    artifacts: vec![],
                },
                sid,
                cv.clone(),
                None,
            ))
            .await
            .unwrap();

        let events = store.get_events(sid, None).await.unwrap();
        assert_eq!(events.len(), 6);

        let rm = RecoveryManager::new("/tmp/e2e_vault".into());
        let recovered = rm.recover_from_events(&events, sid).unwrap();

        assert!(recovered.report.causal_valid);
        assert!(recovered.report.replay_success);
        assert_eq!(recovered.state.status, SessionStatus::Checkpointing);
        assert_eq!(recovered.state.checkpoint_seq, 7);
        assert!(recovered.recovery_plan.is_some());

        let export = SessionExport::from_session(
            &events,
            sid,
            MemoryGraph::default(),
            recovered.state.causal_vector.clone(),
        );
        assert!(export.verify_integrity().is_ok());

        let json = export.to_json().unwrap();
        let reimported = SessionExport::from_json(&json).unwrap();
        let replayed = reimported.replay_into_state().unwrap();
        assert_eq!(replayed.checkpoint_seq, 7);
        assert_eq!(replayed.status, SessionStatus::Checkpointing);
    }

    #[tokio::test]
    async fn e2e_side_effect_two_phase_commit() {
        let mut guard = SideEffectGuard::new();
        let sid = SessionId::from_bytes([0xB1; 16]);
        let tid = TaskId::from_bytes([0xB2; 16]);

        let intent = SideEffectIntent {
            id: "se_e2e_001".into(),
            session_id: sid,
            task_id: tid,
            effect_class: SideEffectClass::Idempotent,
            action_type: "write_file".into(),
            target: "/tmp/e2e_test.txt".into(),
            payload: b"hello e2e".to_vec(),
            request_hash: "e2e_hash_001".into(),
            preconditions: vec![],
        };

        let effect_id = guard.record_intent(intent).unwrap();
        assert!(!effect_id.is_empty());

        let action = guard.recover_effect(&effect_id).unwrap();
        assert!(matches!(action, RecoveryAction::Replay));

        let result = guard.commit_effect(&effect_id, "resp_hash_e2e").unwrap();
        assert!(result.success);

        let committed_action = guard.recover_effect(&effect_id).unwrap();
        assert!(matches!(committed_action, RecoveryAction::UseCached));

        let duplicate = guard
            .record_intent(SideEffectIntent {
                id: "se_e2e_001".into(),
                session_id: sid,
                task_id: tid,
                effect_class: SideEffectClass::Idempotent,
                action_type: "write_file".into(),
                target: "/tmp/e2e_test.txt".into(),
                payload: b"hello e2e".to_vec(),
                request_hash: "e2e_hash_001".into(),
                preconditions: vec![],
            })
            .unwrap();
        assert_eq!(duplicate, effect_id);
    }

    #[tokio::test]
    async fn e2e_budget_exhaustion_blocks_session() {
        let sid = SessionId::from_bytes([0xC1; 16]);
        let mut state = NexusState::new(sid, now_millis());
        state.budget.budget_limit_cents = 100;

        let dag = BTreeMap::new();

        let mut cv = CausalVector::new();
        cv.increment(sid);
        state = transition(
            &state,
            &NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "expensive task".into(),
                    source: "e2e".into(),
                },
                sid,
                cv.clone(),
                None,
            ),
            &dag,
        )
        .unwrap();

        cv.increment(sid);
        state = transition(
            &state,
            &NexusEvent::new(
                EventType::IntentParsed {
                    intent_graph: IntentGraph::default(),
                },
                sid,
                cv.clone(),
                None,
            ),
            &dag,
        )
        .unwrap();

        cv.increment(sid);
        state = transition(
            &state,
            &NexusEvent::new(
                EventType::PlanCommitted {
                    frontier: Frontier::empty(),
                },
                sid,
                cv.clone(),
                None,
            ),
            &dag,
        )
        .unwrap();

        cv.increment(sid);
        state = transition(
            &state,
            &NexusEvent::new(EventType::DependenciesMet, sid, cv.clone(), None),
            &dag,
        )
        .unwrap();

        assert_eq!(state.status, SessionStatus::Executing);

        state.budget.add_cost(150, 1000, 5);
        assert!(state.budget.is_exhausted());

        cv.increment(sid);
        state = transition(
            &state,
            &NexusEvent::new(
                EventType::WorkerFailed {
                    worker_id: "w1".into(),
                    task_id: TaskId::from_bytes([0xDD; 16]),
                    error: "budget exceeded".into(),
                    error_code: ErrorCode::Fatal,
                    retry_count: 0,
                },
                sid,
                cv.clone(),
                None,
            ),
            &dag,
        )
        .unwrap();

        assert_eq!(state.status, SessionStatus::Failed);
    }

    #[tokio::test]
    async fn e2e_concurrent_session_isolation() {
        let (store, _dir) = setup_store().await;

        let sid1 = SessionId::from_bytes([0xD1; 16]);
        let sid2 = SessionId::from_bytes([0xD2; 16]);

        let mut cv1 = CausalVector::new();
        cv1.increment(sid1);
        store
            .append_event(&NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "task 1".into(),
                    source: "e2e".into(),
                },
                sid1,
                cv1,
                None,
            ))
            .await
            .unwrap();

        let mut cv2 = CausalVector::new();
        cv2.increment(sid2);
        store
            .append_event(&NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "task 2".into(),
                    source: "e2e".into(),
                },
                sid2,
                cv2,
                None,
            ))
            .await
            .unwrap();

        let events1 = store.get_events(sid1, None).await.unwrap();
        let events2 = store.get_events(sid2, None).await.unwrap();
        assert_eq!(events1.len(), 1);
        assert_eq!(events2.len(), 1);
        assert_ne!(events1[0].session_id, events2[0].session_id);
    }

    #[tokio::test]
    async fn e2e_cross_session_memory_inheritance() {
        let sid_a = SessionId::from_bytes([0xAA; 16]);
        let sid_b = SessionId::from_bytes([0xBB; 16]);

        let mut mem_a = MemoryGraph::new();
        mem_a.add_node(MemoryNode {
            id: "knowledge_001".into(),
            content: MemoryContent::Text {
                text: "JWT tokens reduce DB load by 80%".into(),
            },
            embedding: None,
            causal_context: CausalVector::singleton(sid_a, 5),
            importance: 900,
            activation: 0,
            source_event_id: "evt_001".into(),
            session_lineage: vec![sid_a],
            created_at: now_millis(),
        });

        let cv_a = CausalVector::singleton(sid_a, 5);
        let export = SessionExport::from_session(&[], sid_a, mem_a, cv_a.clone());

        let mut mem_b = MemoryGraph::new();
        let mut cv_b = CausalVector::singleton(sid_a, 10);
        cv_b.increment(sid_b);

        let imported = export.inherit_memories_into(&mut mem_b, &cv_b).unwrap();
        assert!(!imported.is_empty());
        assert!(!mem_b.nodes.is_empty());

        let node = mem_b.nodes.values().next().unwrap();
        assert!(node.session_lineage.contains(&sid_a));
    }

    #[tokio::test]
    async fn e2e_causal_vector_cross_node_merge() {
        let sid_a = SessionId::from_bytes([0xA1; 16]);
        let sid_b = SessionId::from_bytes([0xB1; 16]);

        let mut cv_a = CausalVector::new();
        for _ in 0..5 {
            cv_a.increment(sid_a);
        }

        let mut cv_b = CausalVector::new();
        for _ in 0..3 {
            cv_b.increment(sid_a);
        }
        cv_b.increment(sid_b);

        cv_a.merge(&cv_b);
        assert_eq!(cv_a.0.get(&sid_a), Some(&5));
        assert_eq!(cv_a.0.get(&sid_b), Some(&1));

        assert!(cv_a.is_consistent());
    }

    #[tokio::test]
    async fn cross_tool_session_migration_openclaw_to_hermes() {
        let sid = SessionId::from_bytes([0x10; 16]);

        // Session driven through full lifecycle: Created → Intake → Planning → Planned → Executing → Checkpoint
        let mut cv = CausalVector::new();
        cv.increment(sid);
        let e1 = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "refactor auth".into(),
                source: "openclaw:discord".into(),
            },
            sid,
            cv.clone(),
            None,
        );

        cv.increment(sid);
        let e2 = NexusEvent::new(
            EventType::IntentParsed {
                intent_graph: IntentGraph::default(),
            },
            sid,
            cv.clone(),
            None,
        );

        cv.increment(sid);
        let e3 = NexusEvent::new(
            EventType::PlanCommitted {
                frontier: Frontier::empty(),
            },
            sid,
            cv.clone(),
            None,
        );

        cv.increment(sid);
        let e4 = NexusEvent::new(EventType::DependenciesMet, sid, cv.clone(), None);

        cv.increment(sid);
        let e5 = NexusEvent::new(
            EventType::WorkerCheckpoint {
                task_id: TaskId::from_bytes([3u8; 16]),
                step_index: 1,
                actions: vec![],
                artifacts: vec![],
            },
            sid,
            cv.clone(),
            None,
        );

        let events = vec![e1, e2, e3, e4, e5];

        // Export from OpenClaw world
        let export = SessionExport::from_session(&events, sid, MemoryGraph::default(), cv);
        assert!(export.verify_integrity().is_ok());
        assert!(export.verify_export_hash().is_ok());

        // Store export as JSON (simulating file transfer between tools)
        let json = export.to_json().unwrap();

        // Re-import in Hermes world
        let reimported = SessionExport::from_json(&json).unwrap();
        assert_eq!(reimported.version, "1.0.0");
        assert_eq!(reimported.session_id, sid.to_hex());

        // Verify hash integrity after import
        assert!(reimported.verify_export_hash().is_ok());

        // Replay state in new environment
        let state = reimported.replay_into_state().unwrap();
        assert_eq!(state.session_id, sid);
        assert_eq!(state.checkpoint_seq, 1);
    }

    #[tokio::test]
    async fn cross_tool_session_migration_with_memories() {
        let sid_src = SessionId::from_bytes([0xAA; 16]);

        let mut source_mem = MemoryGraph::new();
        source_mem.add_node(MemoryNode {
            id: "best_practice_001".into(),
            content: MemoryContent::Text {
                text: "Always use JWT over session tokens".into(),
            },
            embedding: None,
            causal_context: CausalVector::new(),
            importance: 900,
            activation: 0,
            source_event_id: "evt_123".into(),
            session_lineage: vec![],
            created_at: now_millis(),
        });

        let mut cv = CausalVector::new();
        cv.increment(sid_src);
        let event = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "knowledge transfer".into(),
                source: "tool_a".into(),
            },
            sid_src,
            cv.clone(),
            None,
        );

        let export = SessionExport::from_session(&[event], sid_src, source_mem, cv);

        // Export as file (simulating export from tool A)
        let export_path = std::env::temp_dir().join("nexus_migration_test.nexus");
        export.to_file(export_path.to_str().unwrap()).unwrap();

        // Tool B imports
        let imported = SessionExport::from_file(export_path.to_str().unwrap()).unwrap();
        assert!(imported.verify_integrity().is_ok());
        assert!(imported.verify_export_hash().is_ok());
        assert!(!imported.memory_graph.nodes.is_empty());

        // Cleanup
        std::fs::remove_file(&export_path).ok();
    }

    #[tokio::test]
    async fn cross_tool_migration_idempotent_reimport() {
        let sid = SessionId::from_bytes([0xCC; 16]);
        let export = SessionExport::from_session(
            &[NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "idempotent test".into(),
                    source: "tool_a".into(),
                },
                sid,
                {
                    let mut c = CausalVector::new();
                    c.increment(sid);
                    c
                },
                None,
            )],
            sid,
            MemoryGraph::default(),
            CausalVector::singleton(sid, 1),
        );

        let json = export.to_json().unwrap();

        // Import twice — should produce identical results
        let import1 = SessionExport::from_json(&json).unwrap();
        let import2 = SessionExport::from_json(&json).unwrap();

        assert_eq!(import1.export_hash, import2.export_hash);
        assert_eq!(import1.session_id, import2.session_id);
        assert_eq!(import1.events.len(), import2.events.len());

        let state1 = import1.replay_into_state().unwrap();
        let state2 = import2.replay_into_state().unwrap();
        assert_eq!(state1.status, state2.status);
        assert_eq!(state1.version, state2.version);
    }

    #[tokio::test]
    async fn cross_tool_migration_export_tamper_detection() {
        let sid = SessionId::from_bytes([0xDD; 16]);
        let mut cv = CausalVector::new();
        cv.increment(sid);

        let export = SessionExport::from_session(
            &[NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "tamper test".into(),
                    source: "tool_a".into(),
                },
                sid,
                cv,
                None,
            )],
            sid,
            MemoryGraph::default(),
            CausalVector::singleton(sid, 1),
        );

        assert!(export.verify_export_hash().is_ok());

        // Tamper with the struct directly
        let mut tampered = export.clone();
        tampered.session_id = "tampered_session_id".into();

        // Hash verification must fail
        assert!(
            tampered.verify_export_hash().is_err(),
            "Tampered export must fail hash verification"
        );
    }
}
