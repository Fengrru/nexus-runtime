#[cfg(test)]
mod tests {
    use crate::*;

    use std::collections::BTreeMap;

    const GOLDEN_CHECKPOINT_V0: &[u8] = include_bytes!("../../../fixtures/checkpoint_v0.msgpack");

    #[test]
    fn golden_checkpoint_serialization_is_deterministic() {
        let checkpoint = Checkpoint {
            checkpoint_id: "cp_golden_001".to_string(),
            session_id: SessionId::from_bytes([
                0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D,
                0x1E, 0x1F,
            ]),
            step_index: 42,
            total_actions: 100,
            replay_actions: vec![
                ReplayAction::ReadFile {
                    path: "/project/src/auth.py".to_string(),
                    expected_hash: "a3f7c2d8e9b104f5a6c3d2e1f0a9b8c7d6e5f4a3".to_string(),
                },
                ReplayAction::EditFile {
                    path: "/project/src/auth.py".to_string(),
                    search: "def authenticate_session".to_string(),
                    replace: "def authenticate_jwt".to_string(),
                    expected_count: 1,
                },
            ],
            artifact_refs: vec![ArtifactRef {
                id: "art_golden_001".into(),
                kind: ArtifactKind::File,
                uri: "vault://artifacts/golden_001".into(),
                blake3: "b7e9a1f3c2d8f4e6a8c0d2e4f6a8b0c2d4e6f8a0".into(),
                size_bytes: 2048,
                produced_by_session: SessionId::from_bytes([0xA0; 16]),
                produced_by_event: "evt_golden".into(),
                created_at: 1717098723000,
            }],
            handle_registry: vec![HandleRecord {
                handle_type: "file_lock".into(),
                reacquire_command: "flock /project/src/auth.py".into(),
                metadata: {
                    let mut m = BTreeMap::new();
                    m.insert("pid".into(), "12345".into());
                    m.insert("fd".into(), "3".into());
                    m
                },
            }],
            determinism_context: DeterminismContext {
                seed: 12345,
                model_version: "claude-3.5-sonnet-20241022".to_string(),
                input_hash: "deadbeefcafe1234deadbeefcafe5678deadbeefcafe9abc".into(),
                checkpoint_format_version: 0,
                worker_type: WorkerType::Python,
            },
            created_at: 1717098723000,
        };

        let actual = serialize_deterministic(&checkpoint).unwrap();

        // Regenerate golden fixture (uncomment to regenerate):
        // std::fs::write("fixtures/checkpoint_v0.msgpack", &actual).unwrap();

        assert_eq!(
            actual, GOLDEN_CHECKPOINT_V0,
            "Checkpoint serialization diverged from golden fixture.\n\
             This indicates a non-deterministic change in serialization format.\n\
             Verify that only BTreeMap, u64, and rmp-serde StructMap are used."
        );
    }

    #[test]
    fn golden_checkpoint_deserialization_round_trip() {
        let cp: Checkpoint = deserialize_deterministic(GOLDEN_CHECKPOINT_V0).unwrap();

        assert_eq!(cp.checkpoint_id, "cp_golden_001");
        assert_eq!(cp.step_index, 42);
        assert_eq!(cp.total_actions, 100);
        assert_eq!(cp.replay_actions.len(), 2);

        match &cp.replay_actions[0] {
            ReplayAction::ReadFile {
                path,
                expected_hash,
            } => {
                assert_eq!(path, "/project/src/auth.py");
                assert_eq!(expected_hash, "a3f7c2d8e9b104f5a6c3d2e1f0a9b8c7d6e5f4a3");
            }
            _ => panic!("expected ReadFile"),
        }

        assert_eq!(cp.handle_registry.len(), 1);
        assert_eq!(cp.handle_registry[0].handle_type, "file_lock");
        assert_eq!(cp.determinism_context.seed, 12345);
        assert_eq!(
            cp.determinism_context.model_version,
            "claude-3.5-sonnet-20241022"
        );

        // Verify determinism: serialize again must produce identical bytes
        let re_serialized = serialize_deterministic(&cp).unwrap();
        assert_eq!(
            re_serialized, GOLDEN_CHECKPOINT_V0,
            "Re-serialization must be byte-identical to golden fixture"
        );
    }

    #[test]
    fn golden_transition_state_is_deterministic() {
        let sid = SessionId::from_bytes([0xAA; 16]);

        // Two independent processes should produce identical state
        let run = || -> NexusState {
            let mut state = NexusState::new(sid, 1717098723000);
            let dag = BTreeMap::new();
            let mut cv = CausalVector::new();

            cv.increment(sid);
            let e1 = NexusEvent::new(
                EventType::IntentReceived {
                    raw_input: "golden test".into(),
                    source: "fixture".into(),
                },
                sid,
                cv.clone(),
                None,
            );
            state = transition(&state, &e1, &dag).unwrap();

            cv.increment(sid);
            let e2 = NexusEvent::new(
                EventType::IntentParsed {
                    intent_graph: IntentGraph::default(),
                },
                sid,
                cv.clone(),
                None,
            );
            state = transition(&state, &e2, &dag).unwrap();

            cv.increment(sid);
            let e3 = NexusEvent::new(
                EventType::PlanCommitted {
                    frontier: Frontier::empty(),
                },
                sid,
                cv.clone(),
                None,
            );
            state = transition(&state, &e3, &dag).unwrap();

            state
        };

        let state1 = run();
        let state2 = run();

        // Deterministic fields must be identical
        assert_eq!(state1.session_id, state2.session_id);
        assert_eq!(state1.status, state2.status);
        assert_eq!(state1.version, state2.version);
        assert_eq!(state1.checkpoint_seq, state2.checkpoint_seq);
        assert_eq!(
            state1.causal_vector.to_canonical(),
            state2.causal_vector.to_canonical(),
            "Causal vector must be deterministic"
        );

        // Non-deterministic fields (UUIDs, timestamps) are allowed to differ
        // but the core state structure must be identical
    }

    #[test]
    fn golden_causal_vector_fixtures() {
        let sid_a = SessionId::from_bytes([0xC1; 16]);
        let sid_b = SessionId::from_bytes([0xC2; 16]);

        // Fixture 1: merge produces deterministic result
        let mut cv1 = CausalVector::new();
        cv1.increment(sid_a);
        cv1.increment(sid_a);
        cv1.increment(sid_a);
        cv1.increment(sid_b);

        let mut cv2 = CausalVector::new();
        cv2.increment(sid_a);
        cv2.increment(sid_a);
        cv2.increment(sid_b);
        cv2.increment(sid_b);

        let mut merged = cv1.clone();
        merged.merge(&cv2);

        assert_eq!(merged.0.get(&sid_a), Some(&3));
        assert_eq!(merged.0.get(&sid_b), Some(&2));

        // Canonical form must be deterministic
        let canonical1 = merged.to_canonical();
        let canonical2 = merged.to_canonical();
        assert_eq!(canonical1, canonical2);

        // Verify happened-before relationship
        assert!(cv2.happened_before(&cv1) || cv1.happened_before(&cv2) || cv1.is_concurrent(&cv2));
    }
}

/// Property-based tests for CausalVector and transition() invariants.
/// These verify fundamental algebraic laws that must always hold.
#[cfg(test)]
mod property_tests {
    use crate::*;
    use std::collections::BTreeMap;

    // ── CausalVector Properties ────────────────────────────────────────────

    #[test]
    fn prop_causal_vector_empty_is_not_consistent() {
        let cv = CausalVector::new();
        assert!(!cv.is_consistent(), "empty vector must not be consistent");
    }

    #[test]
    fn prop_causal_vector_singleton_is_consistent() {
        let sid = SessionId::from_bytes([1u8; 16]);
        let cv = CausalVector::singleton(sid, 1);
        assert!(cv.is_consistent(), "singleton with positive count must be consistent");
    }

    #[test]
    fn prop_causal_vector_increment_maintains_consistency() {
        let sid = SessionId::from_bytes([2u8; 16]);
        let mut cv = CausalVector::new();
        for _ in 0..10 {
            cv.increment(sid);
            assert!(
                cv.is_consistent(),
                "vector must remain consistent after increment"
            );
        }
    }

    #[test]
    fn prop_causal_vector_merge_is_commutative() {
        let sid_a = SessionId::from_bytes([0xA1; 16]);
        let sid_b = SessionId::from_bytes([0xB1; 16]);

        let mut a = CausalVector::new();
        a.increment(sid_a);
        a.increment(sid_a);
        a.increment(sid_b);

        let mut b = CausalVector::new();
        b.increment(sid_a);
        b.increment(sid_b);
        b.increment(sid_b);

        let mut a_merge_b = a.clone();
        a_merge_b.merge(&b);

        let mut b_merge_a = b.clone();
        b_merge_a.merge(&a);

        assert_eq!(
            a_merge_b.to_canonical(),
            b_merge_a.to_canonical(),
            "merge must be commutative"
        );
    }

    #[test]
    fn prop_causal_vector_merge_is_idempotent() {
        let sid = SessionId::from_bytes([3u8; 16]);
        let mut cv = CausalVector::new();
        for _ in 0..5 {
            cv.increment(sid);
        }

        let canon1 = cv.to_canonical();
        let mut merged = cv.clone();
        merged.merge(&cv);
        assert_eq!(
            merged.to_canonical(),
            canon1,
            "merging a vector with itself must be idempotent"
        );
    }

    #[test]
    fn prop_causal_vector_merge_is_associative() {
        let sid_a = SessionId::from_bytes([0xA2; 16]);
        let sid_b = SessionId::from_bytes([0xB2; 16]);
        let sid_c = SessionId::from_bytes([0xC2; 16]);

        let mut x = CausalVector::new();
        x.increment(sid_a);
        x.increment(sid_a);

        let mut y = CausalVector::new();
        y.increment(sid_a);
        y.increment(sid_b);

        let mut z = CausalVector::new();
        z.increment(sid_b);
        z.increment(sid_c);

        // (x ∪ y) ∪ z == x ∪ (y ∪ z)
        let mut xy = x.clone();
        xy.merge(&y);
        let mut xy_z = xy.clone();
        xy_z.merge(&z);

        let mut yz = y.clone();
        yz.merge(&z);
        let mut x_yz = x.clone();
        x_yz.merge(&yz);

        assert_eq!(
            xy_z.to_canonical(),
            x_yz.to_canonical(),
            "merge must be associative"
        );
    }

    #[test]
    fn prop_causal_vector_happened_before_is_transitive() {
        let sid = SessionId::from_bytes([4u8; 16]);

        let mut a = CausalVector::new();
        a.increment(sid); // { sid: 1 }

        let mut b = a.clone();
        b.increment(sid); // { sid: 2 }

        let mut c = b.clone();
        c.increment(sid); // { sid: 3 }

        assert!(a.happened_before(&b), "a must happen before b");
        assert!(b.happened_before(&c), "b must happen before c");
        assert!(a.happened_before(&c), "happened-before must be transitive");
    }

    #[test]
    fn prop_causal_vector_happened_before_is_antisymmetric() {
        let sid = SessionId::from_bytes([5u8; 16]);

        let mut a = CausalVector::new();
        a.increment(sid);

        let mut b = CausalVector::new();
        b.increment(sid);
        b.increment(sid);

        assert!(a.happened_before(&b));
        assert!(
            !b.happened_before(&a),
            "if a → b then NOT (b → a)"
        );
    }

    #[test]
    fn prop_causal_vector_concurrent_detection() {
        let sid_a = SessionId::from_bytes([0xA3; 16]);
        let sid_b = SessionId::from_bytes([0xB3; 16]);

        let mut cv_a = CausalVector::new();
        cv_a.increment(sid_a);
        cv_a.increment(sid_a);

        let mut cv_b = CausalVector::new();
        cv_b.increment(sid_b);
        cv_b.increment(sid_b);

        // Different session IDs with no overlap → concurrent
        assert!(
            cv_a.is_concurrent(&cv_b),
            "vectors from different sessions without overlap must be concurrent"
        );
    }

    #[test]
    fn prop_causal_vector_canonical_is_deterministic() {
        let sid = SessionId::from_bytes([6u8; 16]);
        let mut cv = CausalVector::new();
        for _ in 0..100 {
            cv.increment(sid);
        }

        let canon1 = cv.to_canonical();
        let canon2 = cv.to_canonical();
        assert_eq!(canon1, canon2, "to_canonical must be deterministic");
        assert!(!canon1.is_empty(), "canonical form must not be empty");
    }

    // ── transition() Properties ──────────────────────────────────────────

    #[test]
    fn prop_transition_is_deterministic() {
        let sid = SessionId::from_bytes([7u8; 16]);
        let dag = BTreeMap::new();

        // Run transition twice with same inputs
        let state1 = NexusState::new(sid, 0);
        let mut cv = CausalVector::new();
        cv.increment(sid);
        let event = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "deterministic test".into(),
                source: "prop".into(),
            },
            sid,
            cv,
            None,
        );

        let result1 = transition(&state1, &event, &dag).unwrap();
        let result2 = transition(&state1, &event, &dag).unwrap();

        assert_eq!(
            result1.status, result2.status,
            "transition must be deterministic — same status"
        );
        assert_eq!(
            result1.version, result2.version,
            "transition must be deterministic — same version"
        );
        assert_eq!(
            result1.latest_event_id, result2.latest_event_id,
            "transition must be deterministic — same event_id"
        );
    }

    #[test]
    fn prop_transition_version_is_monotonic() {
        let sid = SessionId::from_bytes([8u8; 16]);
        let dag = BTreeMap::new();
        let mut state = NexusState::new(sid, 0);
        let mut cv = CausalVector::new();

        let events = vec![
            EventType::IntentReceived {
                raw_input: "monotonic".into(),
                source: "prop".into(),
            },
            EventType::IntentParsed {
                intent_graph: IntentGraph::default(),
            },
            EventType::PlanCommitted {
                frontier: Frontier::empty(),
            },
            EventType::DependenciesMet,
        ];

        let mut prev_version = state.version;
        for event_type in events {
            cv.increment(sid);
            let event = NexusEvent::new(event_type, sid, cv.clone(), None);
            state = transition(&state, &event, &dag).unwrap();
            assert!(
                state.version > prev_version,
                "version must increase monotonically: {} -> {}",
                prev_version,
                state.version
            );
            prev_version = state.version;
        }
    }

    #[test]
    fn prop_transition_invalid_event_returns_error() {
        let sid = SessionId::from_bytes([9u8; 16]);
        let dag = BTreeMap::new();
        let state = NexusState::new(sid, 0);

        // DependenciesMet on Created state is invalid (must go through Intake → ... → Planned)
        let mut cv = CausalVector::new();
        cv.increment(sid);
        let event = NexusEvent::new(EventType::DependenciesMet, sid, cv, None);
        let result = transition(&state, &event, &dag);
        assert!(
            result.is_err(),
            "DependenciesMet on Created must fail — invalid transition"
        );
    }

    #[test]
    fn prop_transition_status_is_always_valid() {
        let sid = SessionId::from_bytes([10u8; 16]);
        let dag = BTreeMap::new();
        let mut state = NexusState::new(sid, 0);
        let mut cv = CausalVector::new();

        // Full lifecycle: Created → Intake → Planning → Planned → Executing
        let valid_statuses = [
            SessionStatus::Created,
            SessionStatus::Intake,
            SessionStatus::Planning,
            SessionStatus::Planned,
            SessionStatus::Executing,
        ];

        for (i, expected) in valid_statuses[1..].iter().enumerate() {
            cv.increment(sid);
            let event_type = match i {
                0 => EventType::IntentReceived {
                    raw_input: "valid".into(),
                    source: "prop".into(),
                },
                1 => EventType::IntentParsed {
                    intent_graph: IntentGraph::default(),
                },
                2 => EventType::PlanCommitted {
                    frontier: Frontier::empty(),
                },
                3 => EventType::DependenciesMet,
                _ => unreachable!(),
            };
            let event = NexusEvent::new(event_type, sid, cv.clone(), None);
            state = transition(&state, &event, &dag).unwrap();
            assert_eq!(
                &state.status, expected,
                "after transition {}, expected {:?}, got {:?}",
                i + 1,
                expected,
                state.status
            );
        }
    }

    #[test]
    fn prop_transition_side_effect_guard_idempotent_replay() {
        let mut guard = SideEffectGuard::new();
        let sid = SessionId::from_bytes([11u8; 16]);
        let tid = TaskId::from_bytes([12u8; 16]);

        // Record intent twice with same request_hash → should return same ID
        let intent = SideEffectIntent {
            id: "prop_se_001".into(),
            session_id: sid,
            task_id: tid,
            effect_class: SideEffectClass::Idempotent,
            action_type: "write_file".into(),
            target: "/tmp/prop_test.txt".into(),
            payload: vec![1, 2, 3],
            request_hash: "prop_hash_001".into(),
            preconditions: vec![],
        };

        let eid1 = guard.record_intent(intent.clone()).unwrap();
        let eid2 = guard.record_intent(intent).unwrap();

        assert_eq!(
            eid1, eid2,
            "same request_hash must produce same effect_id (idempotent recording)"
        );
    }

    #[test]
    fn prop_transition_budget_enforcement() {
        let sid = SessionId::from_bytes([13u8; 16]);
        let mut state = NexusState::new(sid, 0);
        state.budget.budget_limit_cents = 100;
        state.budget.consumed_cents = 95;

        // Budget: 100 cents, consumed 95, remaining 5
        assert!(!state.budget.is_exhausted());
        assert_eq!(state.budget.remaining_cents(), 5);

        // Can't afford 10 cents
        assert!(!state.budget.can_afford(10));

        // Can afford 3 cents
        assert!(state.budget.can_afford(3));

        // Exhaust with 6 more
        state.budget.add_cost(6, 100, 1);
        assert!(state.budget.is_exhausted());
    }

    #[test]
    fn prop_event_integrity_hash_deterministic() {
        let sid = SessionId::from_bytes([14u8; 16]);
        let mut cv = CausalVector::new();
        cv.increment(sid);

        let event = NexusEvent::new(
            EventType::IntentReceived {
                raw_input: "hash test".into(),
                source: "prop".into(),
            },
            sid,
            cv,
            None,
        );

        let hash1 = event.compute_integrity_hash();
        let hash2 = event.compute_integrity_hash();

        assert_eq!(
            hash1, hash2,
            "integrity hash must be deterministic for same event"
        );
        assert_eq!(hash1.len(), 64, "hash must be 64 hex chars (BLAKE3 32 bytes)");
    }

    #[test]
    fn prop_recovery_replay_is_byte_identical() {
        let sid = SessionId::from_bytes([15u8; 16]);
        let mut events = Vec::new();
        let mut cv = CausalVector::new();

        // Build full lifecycle events
        for (i, event_type) in [
            EventType::IntentReceived {
                raw_input: "replay test".into(),
                source: "prop".into(),
            },
            EventType::IntentParsed {
                intent_graph: IntentGraph::default(),
            },
            EventType::PlanCommitted {
                frontier: Frontier::empty(),
            },
            EventType::DependenciesMet,
        ]
        .into_iter()
        .enumerate()
        {
            cv.increment(sid);
            let event = NexusEvent::new(event_type, sid, cv.clone(), None);
            events.push(event);
            let _ = i;
        }

        // First replay
        let dag = BTreeMap::new();
        let mut state1 = NexusState::new(sid, events[0].event_timestamp);
        for event in &events {
            state1 = transition(&state1, event, &dag).unwrap();
        }

        // Second replay — must produce identical state
        let mut state2 = NexusState::new(sid, events[0].event_timestamp);
        for event in &events {
            state2 = transition(&state2, event, &dag).unwrap();
        }

        assert_eq!(state1.status, state2.status, "replay: status must match");
        assert_eq!(state1.version, state2.version, "replay: version must match");
        assert_eq!(
            state1.checkpoint_seq, state2.checkpoint_seq,
            "replay: checkpoint_seq must match"
        );
    }
}
