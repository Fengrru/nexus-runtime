use crate::types::*;
use std::collections::BTreeSet;

// ── Embedding Generator ───────────────────────────────────────────────

/// Trait for generating vector embeddings from memory content.
///
/// Implementations can range from content-addressed hashes (deterministic, no ML)
/// to external vector database APIs (semantic, requires network).
pub trait EmbeddingGenerator: Send + Sync {
    /// Generate an embedding vector for the given text content.
    /// Returns the raw bytes encoding the embedding (e.g. f32 little-endian).
    fn generate(&self, content: &MemoryContent) -> Vec<u8>;

    /// Dimensionality of the embedding (number of f32 elements).
    fn dim(&self) -> usize;
}

/// No-op embedding generator. Always returns an empty embedding.
/// Used when semantic memory retrieval is not configured.
#[derive(Debug, Clone, Default)]
pub struct NoopEmbeddingGenerator;

impl EmbeddingGenerator for NoopEmbeddingGenerator {
    fn generate(&self, _content: &MemoryContent) -> Vec<u8> {
        Vec::new()
    }

    fn dim(&self) -> usize {
        0
    }
}

/// Deterministic embedding generator using BLAKE3 hashing.
///
/// Produces content-addressed embeddings: identical content yields identical
/// vectors. This is useful for testing, offline usage, and as a baseline
/// before integrating semantic embedding APIs.
///
/// Generates `dim` f32 values (default 64) by expanding a BLAKE3 hash via
/// a counter mode (hash(content || counter)).
#[derive(Debug, Clone)]
pub struct Blake3EmbeddingGenerator {
    dim: usize,
}

impl Blake3EmbeddingGenerator {
    pub fn new(dim: usize) -> Self {
        assert!(dim > 0, "embedding dimension must be positive");
        Self { dim }
    }

    pub fn default_dim() -> Self {
        Self::new(64)
    }
}

impl EmbeddingGenerator for Blake3EmbeddingGenerator {
    fn generate(&self, content: &MemoryContent) -> Vec<u8> {
        let text = content.canonical_text();
        let text_bytes = text.as_bytes();
        let num_floats = self.dim;
        let mut embedding = Vec::with_capacity(num_floats * 4);

        // Expand hash to dim f32 values using counter mode:
        //   embedding[i] = H(text || i) as u32 / u32::MAX → f32 in [-1, 1]
        for i in 0..num_floats {
            let counter = i.to_le_bytes();
            let mut hasher = blake3::Hasher::new();
            hasher.update(text_bytes);
            hasher.update(&counter);
            let hash = hasher.finalize();
            let bytes = hash.as_bytes();
            let u = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            let f = (u as f64 / u32::MAX as f64) as f32 * 2.0 - 1.0;
            embedding.extend_from_slice(&f.to_le_bytes());
        }

        embedding
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

// ── MemoryContent helpers ─────────────────────────────────────────────

impl MemoryContent {
    /// Canonical text representation for embedding generation.
    pub fn canonical_text(&self) -> String {
        match self {
            MemoryContent::Text { text } => text.clone(),
            MemoryContent::Structured { data } => {
                let parts: Vec<&str> = data.values().map(|s| s.as_str()).collect();
                parts.join(" ")
            }
            MemoryContent::Proposition {
                subject,
                predicate,
                object,
                ..
            } => format!("{} {} {}", subject, predicate, object),
            MemoryContent::Skill {
                skill_id,
                version,
                parameters,
            } => {
                let mut s = format!("skill:{}:{}", skill_id, version);
                for (k, v) in parameters {
                    s.push(' ');
                    s.push_str(k);
                    s.push(':');
                    s.push_str(v);
                }
                s
            }
        }
    }
}

// ── MemoryGraph methods ───────────────────────────────────────────────

impl MemoryGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, node: MemoryNode) {
        self.nodes.insert(node.id.clone(), node);
    }

    pub fn add_edge(&mut self, edge: MemoryEdge) {
        self.edges.push(edge);
    }

    pub fn query_causal(
        &self,
        from: &str,
        edge_type: Option<MemoryEdgeType>,
        depth: usize,
    ) -> Vec<&MemoryNode> {
        let mut results = Vec::new();
        let mut visited = BTreeSet::new();
        let mut queue = vec![(from.to_string(), 0)];

        while let Some((current, current_depth)) = queue.pop() {
            if current_depth > depth || visited.contains(&current) {
                continue;
            }
            visited.insert(current.clone());

            if let Some(node) = self.nodes.get(&current) {
                results.push(node);
            }

            for edge in &self.edges {
                if edge.from == current {
                    if let Some(ref et) = edge_type {
                        if edge.edge_type != *et {
                            continue;
                        }
                    }
                    queue.push((edge.to.clone(), current_depth + 1));
                }
            }
        }
        results
    }

    pub fn compute_activation(&self, memory_id: &str, query_context: &QueryContext) -> u64 {
        let node = match self.nodes.get(memory_id) {
            Some(n) => n,
            None => return 0,
        };

        let relevance = match (&node.embedding, &query_context.embedding) {
            (Some(ne), Some(qe)) => cosine_similarity_u8(ne, qe),
            _ => 5000,
        };

        let importance = node.importance;

        let age_hours = if query_context.now > node.created_at {
            (query_context.now - node.created_at) / 3_600_000
        } else {
            0
        };
        let recency = if age_hours < 1 {
            10000
        } else {
            ((10000.0 / (1.0 + (age_hours as f64).ln())) as u64).min(10000)
        };

        let goal_alignment = if query_context
            .active_goals
            .iter()
            .any(|g| node.content.matches_goal(g))
        {
            8000
        } else {
            3000
        };

        let causal_proximity = query_context
            .recent_memories
            .iter()
            .map(|recent| self.graph_distance(memory_id, recent))
            .min()
            .map(|d| {
                if d == 0 {
                    10000
                } else if d == usize::MAX {
                    5000
                } else {
                    10000 / d as u64
                }
            })
            .unwrap_or(5000);

        (relevance * 3000
            + importance * 2500
            + recency * 2000
            + goal_alignment * 1500
            + causal_proximity * 1000)
            / 10000
    }

    fn graph_distance(&self, from: &str, to: &str) -> usize {
        let mut queue = std::collections::VecDeque::new();
        let mut visited = BTreeSet::new();
        queue.push_back((from.to_string(), 0));

        while let Some((current, distance)) = queue.pop_front() {
            if current == to {
                return distance;
            }
            if visited.contains(&current) {
                continue;
            }
            visited.insert(current.clone());

            for edge in &self.edges {
                if edge.from == current {
                    queue.push_back((edge.to.clone(), distance + 1));
                }
            }
        }
        usize::MAX
    }

    pub fn inherit_memories(
        &mut self,
        source: &MemoryGraph,
        source_session: SessionId,
        causal_vector: &CausalVector,
    ) -> Result<Vec<String>, String> {
        let mut imported = Vec::new();

        for (id, node) in &source.nodes {
            if node.causal_context.compare(causal_vector) == CausalRelation::Concurrent {
                continue;
            }

            let mut new_node = node.clone();
            new_node.session_lineage.push(source_session);
            new_node.causal_context.merge(causal_vector);

            let new_id = format!("{}:{}", source_session.to_hex(), id);
            self.nodes.insert(new_id.clone(), new_node);
            imported.push(new_id);
        }

        Ok(imported)
    }

    /// Apply an embedding generator to all nodes whose embedding is `None`.
    /// Returns the number of nodes that received a new embedding.
    pub fn embed_memories(&mut self, generator: &dyn EmbeddingGenerator) -> usize {
        let mut count = 0;
        for node in self.nodes.values_mut() {
            if node.embedding.is_none() {
                let embedding = generator.generate(&node.content);
                if !embedding.is_empty() {
                    node.embedding = Some(embedding);
                    count += 1;
                }
            }
        }
        count
    }

    /// Generate an embedding for a QueryContext from a text query.
    pub fn embed_query(generator: &dyn EmbeddingGenerator, query: &str) -> Vec<u8> {
        let content = MemoryContent::Text {
            text: query.to_string(),
        };
        generator.generate(&content)
    }
}

fn cosine_similarity_u8(a: &[u8], b: &[u8]) -> u64 {
    if a.len() < 4 || b.len() < 4 || a.len() != b.len() {
        return 5000;
    }

    let a_f32: Vec<f32> = a
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let b_f32: Vec<f32> = b
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;

    for i in 0..a_f32.len() {
        dot += a_f32[i] as f64 * b_f32[i] as f64;
        norm_a += (a_f32[i] as f64) * (a_f32[i] as f64);
        norm_b += (b_f32[i] as f64) * (b_f32[i] as f64);
    }

    if norm_a == 0.0 || norm_b == 0.0 {
        return 5000;
    }

    let sim = dot / (norm_a.sqrt() * norm_b.sqrt());
    ((sim.clamp(-1.0, 1.0) + 1.0) * 5000.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CausalVector;

    // ── NoopEmbeddingGenerator ────────────────────────────────────

    #[test]
    fn noop_returns_empty() {
        let gen = NoopEmbeddingGenerator;
        let content = MemoryContent::Text {
            text: "hello world".into(),
        };
        assert_eq!(gen.generate(&content).len(), 0);
        assert_eq!(gen.dim(), 0);
    }

    // ── Blake3EmbeddingGenerator ──────────────────────────────────

    #[test]
    fn blake3_has_correct_dim() {
        let gen = Blake3EmbeddingGenerator::new(16);
        let content = MemoryContent::Text {
            text: "test".into(),
        };
        let emb = gen.generate(&content);
        assert_eq!(emb.len(), 16 * 4);
        assert_eq!(gen.dim(), 16);
    }

    #[test]
    fn blake3_deterministic() {
        let gen = Blake3EmbeddingGenerator::default_dim();
        let content = MemoryContent::Text {
            text: "deterministic".into(),
        };
        let e1 = gen.generate(&content);
        let e2 = gen.generate(&content);
        assert_eq!(e1, e2, "same content must produce same embedding");
    }

    #[test]
    fn blake3_different_content_different_embedding() {
        let gen = Blake3EmbeddingGenerator::new(8);
        let c1 = MemoryContent::Text {
            text: "apple".into(),
        };
        let c2 = MemoryContent::Text {
            text: "banana".into(),
        };
        let e1 = gen.generate(&c1);
        let e2 = gen.generate(&c2);
        assert_ne!(e1, e2, "different content must produce different embedding");
    }

    #[test]
    fn blake3_values_in_range() {
        let gen = Blake3EmbeddingGenerator::new(32);
        let content = MemoryContent::Text {
            text: "range test".into(),
        };
        let emb = gen.generate(&content);
        for chunk in emb.chunks_exact(4) {
            let f = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            assert!((-1.0..=1.0).contains(&f), "f32 must be in [-1, 1], got {}", f);
        }
    }

    #[test]
    fn blake3_proposition_content() {
        let gen = Blake3EmbeddingGenerator::default_dim();
        let content = MemoryContent::Proposition {
            subject: "Rust".into(),
            predicate: "is".into(),
            object: "safe".into(),
            confidence: 9000,
        };
        let emb = gen.generate(&content);
        assert_eq!(emb.len(), 64 * 4);
    }

    // ── MemoryContent::canonical_text ─────────────────────────────

    #[test]
    fn canonical_text_text_variant() {
        let c = MemoryContent::Text {
            text: "hello".into(),
        };
        assert_eq!(c.canonical_text(), "hello");
    }

    #[test]
    fn canonical_text_structured_variant() {
        let mut data = std::collections::BTreeMap::new();
        data.insert("a".into(), "1".into());
        data.insert("b".into(), "2".into());
        let c = MemoryContent::Structured { data };
        let text = c.canonical_text();
        assert!(text.contains("1"));
        assert!(text.contains("2"));
    }

    #[test]
    fn canonical_text_proposition_variant() {
        let c = MemoryContent::Proposition {
            subject: "A".into(),
            predicate: "B".into(),
            object: "C".into(),
            confidence: 1000,
        };
        assert_eq!(c.canonical_text(), "A B C");
    }

    #[test]
    fn canonical_text_skill_variant() {
        let mut params = std::collections::BTreeMap::new();
        params.insert("lang".into(), "rs".into());
        let c = MemoryContent::Skill {
            skill_id: "build".into(),
            version: "1.0".into(),
            parameters: params,
        };
        let text = c.canonical_text();
        assert!(text.contains("skill:build:1.0"));
        assert!(text.contains("lang:rs"));
    }

    // ── MemoryGraph::embed_memories ───────────────────────────────

    #[test]
    fn embed_memories_fills_none() {
        let mut graph = MemoryGraph::new();
        graph.add_node(MemoryNode {
            id: "n1".into(),
            content: MemoryContent::Text {
                text: "memory one".into(),
            },
            embedding: None,
            causal_context: CausalVector::new(),
            importance: 5000,
            activation: 0,
            source_event_id: "e1".into(),
            session_lineage: vec![],
            created_at: 1000,
        });

        let gen = Blake3EmbeddingGenerator::new(8);
        let count = graph.embed_memories(&gen);
        assert_eq!(count, 1);
        assert!(graph.nodes["n1"].embedding.is_some());
    }

    #[test]
    fn embed_memories_skips_existing() {
        let mut graph = MemoryGraph::new();
        let existing_emb = vec![0u8; 32];
        graph.add_node(MemoryNode {
            id: "n1".into(),
            content: MemoryContent::Text {
                text: "has embedding".into(),
            },
            embedding: Some(existing_emb.clone()),
            causal_context: CausalVector::new(),
            importance: 5000,
            activation: 0,
            source_event_id: "e1".into(),
            session_lineage: vec![],
            created_at: 1000,
        });

        let gen = Blake3EmbeddingGenerator::default_dim();
        let count = graph.embed_memories(&gen);
        assert_eq!(count, 0);
        assert_eq!(graph.nodes["n1"].embedding, Some(existing_emb));
    }

    #[test]
    fn embed_memories_noop_skips_all() {
        let mut graph = MemoryGraph::new();
        graph.add_node(MemoryNode {
            id: "n1".into(),
            content: MemoryContent::Text {
                text: "text".into(),
            },
            embedding: None,
            causal_context: CausalVector::new(),
            importance: 5000,
            activation: 0,
            source_event_id: "e1".into(),
            session_lineage: vec![],
            created_at: 1000,
        });

        let gen = NoopEmbeddingGenerator;
        let count = graph.embed_memories(&gen);
        assert_eq!(count, 0);
        assert!(graph.nodes["n1"].embedding.is_none());
    }

    // ── Embedding-aware compute_activation ────────────────────────

    #[test]
    fn compute_activation_with_similar_embeddings() {
        let gen = Blake3EmbeddingGenerator::new(16);
        let mut graph = MemoryGraph::new();

        let content = MemoryContent::Text {
            text: "System design for distributed databases".into(),
        };
        let emb = gen.generate(&content);

        graph.add_node(MemoryNode {
            id: "n1".into(),
            content,
            embedding: Some(emb),
            causal_context: CausalVector::new(),
            importance: 8000,
            activation: 0,
            source_event_id: "e1".into(),
            session_lineage: vec![],
            created_at: 1_000_000,
        });

        let query_emb = gen.generate(&MemoryContent::Text {
            text: "distributed database architecture".into(),
        });

        let ctx = QueryContext {
            embedding: Some(query_emb),
            active_goals: vec!["design".into()],
            recent_memories: vec!["n1".into()],
            now: 2_000_000,
        };

        let activation = graph.compute_activation("n1", &ctx);
        assert!(activation > 0, "should get positive activation");
    }
}
