use chrono::Utc;
use loci_codebase::{CodebaseIndexer, GitHistory, ParsedFile, SymbolKind};
use loci_core::types::{MemoryScope, Message, Role};
use loci_graph::{Edge, EdgeKind, GraphStore, KnowledgeGraph, Node, NodeKind, VectorIndex};
use loci_knowledge::KnowledgeStore;
use loci_llm::{config::BsConfig, LlmClient};
use loci_memory::{remember, MemoryStore};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Serialize)]
pub struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Serialize)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub description: Option<String>,
    pub file_path: Option<String>,
}

#[derive(Serialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
}

#[derive(Serialize)]
pub struct TraceData {
    pub anchors: Vec<GraphNode>,
    pub decisions: Vec<GraphNode>,
    pub commits: Vec<GraphNode>,
    pub evidence: Vec<GraphEdge>,
    pub related: Vec<GraphNode>,
}

#[derive(Serialize)]
pub struct DocData {
    pub kind: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EvalScore {
    pub score: u8,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    pub category: String,
    pub prompt: String,
    pub answer: String,
    pub score: EvalScore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalData {
    pub average_score: f32,
    pub results: Vec<EvalResult>,
    pub drift_check: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvalSample {
    category: String,
    prompt: String,
}

fn normalize_project_path(path: impl AsRef<Path>) -> String {
    std::fs::canonicalize(path.as_ref())
        .unwrap_or_else(|_| path.as_ref().to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn registry_path() -> PathBuf {
    let dir = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/bs");
    std::fs::create_dir_all(&dir).ok();
    dir.join("projects.json")
}

fn default_project_path() -> String {
    let active = std::fs::read_to_string(registry_path())
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|json| {
            let active = json.get("active")?.as_str()?.to_string();
            json.get("projects")?.as_array()?.iter()
                .find(|project| project.get("name").and_then(|name| name.as_str()) == Some(active.as_str()))
                .and_then(|project| project.get("path").and_then(|path| path.as_str()))
                .map(|path| path.to_string())
        });

    active.unwrap_or_else(|| normalize_project_path("."))
}

fn resolve_project_path(project_path: &str) -> PathBuf {
    let trimmed = project_path.trim();
    if trimmed.is_empty() || trimmed == "." {
        PathBuf::from(default_project_path())
    } else {
        PathBuf::from(normalize_project_path(trimmed))
    }
}

fn bs_dir(path: &Path) -> PathBuf {
    let dir = path.join(".bs");
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn graph_db_path(path: &Path) -> String {
    bs_dir(path).join("graph.db").to_string_lossy().to_string()
}

fn memory_db_path(path: &Path) -> String {
    bs_dir(path).join("memory.db").to_string_lossy().to_string()
}

fn knowledge_db_path(path: &Path) -> String {
    bs_dir(path).join("knowledge.db").to_string_lossy().to_string()
}

fn map_node(node: &Node) -> GraphNode {
    GraphNode {
        id: node.id.to_string(),
        label: node.name.clone(),
        kind: format!("{:?}", node.kind),
        description: node.description.clone(),
        file_path: node.file_path.clone(),
    }
}

fn map_edge(edge: &Edge) -> GraphEdge {
    GraphEdge {
        from: edge.from.to_string(),
        to: edge.to.to_string(),
        kind: format!("{:?}", edge.kind),
    }
}

fn is_evidence_edge(kind: &EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::EvidenceFromFile
            | EdgeKind::EvidenceFromCommit
            | EdgeKind::EvidenceFromConcept
            | EdgeKind::EvidenceFromDecision
    )
}

fn require_llm(cfg: &BsConfig, provider: Option<&str>) -> Result<Box<dyn LlmClient>, String> {
    cfg.build_client(provider)
        .map_err(|_| "No LLM configured. Create .bs/config.toml or set OPENAI_API_KEY.".to_string())
}

fn build_doc_prompt(graph: &KnowledgeGraph, kind: &str) -> String {
    let decisions: Vec<&Node> = graph.nodes.iter()
        .filter(|n| n.kind == NodeKind::Decision)
        .take(12)
        .collect();
    let concepts: Vec<&Node> = graph.nodes.iter()
        .filter(|n| n.kind == NodeKind::Concept)
        .take(12)
        .collect();
    let files: Vec<&Node> = graph.nodes.iter()
        .filter(|n| n.kind == NodeKind::File)
        .take(12)
        .collect();

    let context = format!(
        "## Decisions\n{}\n\n## Concepts\n{}\n\n## Files\n{}",
        decisions.iter().map(|n| format!("- {}: {}", n.name, n.description.as_deref().unwrap_or(""))).collect::<Vec<_>>().join("\n"),
        concepts.iter().map(|n| format!("- {}: {}", n.name, n.description.as_deref().unwrap_or(""))).collect::<Vec<_>>().join("\n"),
        files.iter().map(|n| format!("- {}", n.name)).collect::<Vec<_>>().join("\n"),
    );

    match kind {
        "module" => format!(
            "Generate a module summary document from the project graph.\n\
             Prioritize factual description first, then inferred design rationale.\n\
             Output Markdown with sections: Overview, Key Modules, Decisions, Open Questions.\n\n{}",
            context
        ),
        "handoff" => format!(
            "Generate a handoff document for a new maintainer from the project graph.\n\
             Prioritize factual description first, then inferred design rationale.\n\
             Output Markdown with sections: What Matters Most, Important Decisions, Risk Areas, Open Questions.\n\n{}",
            context
        ),
        _ => format!(
            "Generate an onboarding guide for a new developer from the project graph.\n\
             Prioritize factual description first, then inferred design rationale.\n\
             Output Markdown with sections: Project Overview, Where to Start, Important Decisions, Core Concepts, Open Questions.\n\n{}",
            context
        ),
    }
}

async fn build_graph_local(project_path: &Path) -> Result<(usize, usize, usize, usize), String> {
    let index = CodebaseIndexer::index(project_path).map_err(|e| e.to_string())?;
    let store = GraphStore::new(&graph_db_path(project_path)).await.map_err(|e| e.to_string())?;
    store.clear().await.map_err(|e| e.to_string())?;

    let mut graph = KnowledgeGraph::default();
    let mut commit_nodes: HashMap<String, Uuid> = HashMap::new();
    let parsed_by_path: HashMap<String, &ParsedFile> = index.parsed_files.iter()
        .map(|pf| (pf.path.clone(), pf))
        .collect();

    for file in index.summary.files.iter().filter(|file| file.language.is_code()) {
        let file_path = file.path.to_string_lossy().to_string();
        let parsed = parsed_by_path.get(&file_path).copied();
        let file_node = Node {
            id: Uuid::new_v4(),
            kind: NodeKind::File,
            name: file.relative_path.clone(),
            file_path: Some(file_path.clone()),
            description: parsed.and_then(|pf| pf.doc_comment.clone()).or_else(|| Some(format!("{} lines", file.line_count))),
            raw_source: None,
            created_at: Utc::now(),
        };
        let file_id = graph.add_node(file_node.clone());
        store.save_node(&file_node).await.map_err(|e| e.to_string())?;

        if let Ok(history) = GitHistory::file_history(&project_path.to_path_buf(), &file_path, 3) {
            for commit in history.commits {
                let commit_id = if let Some(existing) = commit_nodes.get(&commit.hash) {
                    *existing
                } else {
                    let node = Node {
                        id: Uuid::new_v4(),
                        kind: NodeKind::Commit,
                        name: commit.hash.clone(),
                        file_path: Some(file_path.clone()),
                        description: Some(format!("{} — {}", commit.message, commit.author)),
                        raw_source: None,
                        created_at: chrono::DateTime::from_timestamp(commit.timestamp, 0).unwrap_or_else(Utc::now),
                    };
                    let id = graph.add_node(node.clone());
                    store.save_node(&node).await.map_err(|e| e.to_string())?;
                    commit_nodes.insert(commit.hash.clone(), id);
                    id
                };

                let edge = Edge {
                    id: Uuid::new_v4(),
                    from: file_id,
                    to: commit_id,
                    kind: EdgeKind::ChangedIn,
                    label: Some("recent file history".to_string()),
                };
                graph.add_edge(edge.clone());
                store.save_edge(&edge).await.map_err(|e| e.to_string())?;
            }
        }

        if let Some(parsed_file) = parsed {
            for symbol in &parsed_file.symbols {
                let kind = match symbol.kind {
                    SymbolKind::Struct => NodeKind::Struct,
                    SymbolKind::Enum => NodeKind::Enum,
                    SymbolKind::Trait => NodeKind::Trait,
                    SymbolKind::Module => NodeKind::Module,
                    _ => NodeKind::Function,
                };
                let symbol_node = Node {
                    id: Uuid::new_v4(),
                    kind,
                    name: symbol.name.clone(),
                    file_path: Some(file_path.clone()),
                    description: symbol.doc_comment.clone(),
                    raw_source: None,
                    created_at: Utc::now(),
                };
                let symbol_id = graph.add_node(symbol_node.clone());
                store.save_node(&symbol_node).await.map_err(|e| e.to_string())?;
                let edge = Edge {
                    id: Uuid::new_v4(),
                    from: file_id,
                    to: symbol_id,
                    kind: EdgeKind::Contains,
                    label: None,
                };
                graph.add_edge(edge.clone());
                store.save_edge(&edge).await.map_err(|e| e.to_string())?;
            }

            for (caller, callee) in &parsed_file.calls {
                let from_id = graph.nodes.iter()
                    .find(|node| &node.name == caller && node.file_path.as_deref() == Some(file_path.as_str()))
                    .map(|node| node.id);
                let to_id = graph.nodes.iter()
                    .find(|node| &node.name == callee && node.file_path.as_deref() == Some(file_path.as_str()))
                    .map(|node| node.id);
                if let (Some(from), Some(to)) = (from_id, to_id) {
                    if from != to {
                        let edge = Edge {
                            id: Uuid::new_v4(),
                            from,
                            to,
                            kind: EdgeKind::Calls,
                            label: None,
                        };
                        graph.add_edge(edge.clone());
                        store.save_edge(&edge).await.map_err(|e| e.to_string())?;
                    }
                }
            }
        }
    }

    let ts_file = bs_dir(project_path).join("last_index");
    let _ = std::fs::write(ts_file, Utc::now().timestamp().to_string());

    Ok((
        index.summary.files.len(),
        index.summary.total_lines,
        graph.nodes.len(),
        graph.edges.len(),
    ))
}

fn is_trace_question(question: &str) -> bool {
    let q = question.to_lowercase();
    [
        "why",
        "为什么",
        "原因",
        "设计",
        "决策",
        "history",
        "blame",
        "diff",
        "演进",
        "trace",
    ]
    .iter()
    .any(|needle| q.contains(needle))
}

async fn build_graph_context(
    question: &str,
    q_vec: &Option<Vec<f32>>,
    graph: &KnowledgeGraph,
    vector_index: &VectorIndex,
    has_embeddings: bool,
) -> String {
    if has_embeddings {
        if let Some(vector) = q_vec {
            if let Ok(hits) = vector_index.search(vector, 30).await {
                let trace_query = is_trace_question(question);
                let mut ranked_ids: Vec<Uuid> = if trace_query {
                    let mut preferred: Vec<Uuid> = hits.iter()
                        .filter_map(|(id, _)| graph.nodes.iter()
                            .find(|node| node.id == *id && matches!(node.kind, NodeKind::Decision | NodeKind::Commit))
                            .map(|node| node.id))
                        .collect();
                    let mut fallback: Vec<Uuid> = hits.iter()
                        .filter_map(|(id, _)| graph.nodes.iter()
                            .find(|node| node.id == *id && !matches!(node.kind, NodeKind::Decision | NodeKind::Commit))
                            .map(|node| node.id))
                        .collect();
                    preferred.append(&mut fallback);
                    preferred
                } else {
                    hits.into_iter().map(|(id, _)| id).collect()
                };

                ranked_ids.truncate(if trace_query { 18 } else { 20 });
                let mut ids: HashSet<Uuid> = ranked_ids.into_iter().collect();
                for id in ids.clone() {
                    for (_, neighbor) in graph.neighbors(id) {
                        ids.insert(neighbor.id);
                    }
                }
                let mut context = graph.to_context_str_filtered(&ids);
                if trace_query {
                    let decisions = graph.nodes.iter()
                        .filter(|node| ids.contains(&node.id) && node.kind == NodeKind::Decision)
                        .map(|node| format!("- {}: {}", node.name, node.description.as_deref().unwrap_or("")))
                        .collect::<Vec<_>>();
                    if !decisions.is_empty() {
                        context = format!("## Prior Decisions\n{}\n\n{}", decisions.join("\n"), context);
                    }
                }
                return context;
            }
        }
    }

    graph.to_context_str(None)
}

async fn build_memory_context(q_vec: &Option<Vec<f32>>, store: &MemoryStore) -> String {
    let memories = store.recall(q_vec.as_deref(), Some(MemoryScope::Session), None, 5).await.unwrap_or_default();
    if memories.is_empty() {
        return String::new();
    }
    format!(
        "\n## Past context\n{}",
        memories.iter().map(|memory| format!("- {}", memory.content)).collect::<Vec<_>>().join("\n")
    )
}

async fn build_kb_context(q_vec: &Option<Vec<f32>>, store: &KnowledgeStore) -> String {
    let items = store.search_external(q_vec.as_deref(), None, 3).await.unwrap_or_default();
    if items.is_empty() {
        return String::new();
    }
    format!(
        "\n## Knowledge base\n{}",
        items.iter()
            .map(|item| format!("---\n{}", &item.content[..item.content.len().min(800)]))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

async fn auto_extract_knowledge(
    question: &str,
    answer: &str,
    llm: &dyn LlmClient,
    graph: &KnowledgeGraph,
    graph_store: &GraphStore,
    vector_index: &VectorIndex,
) {
    let prompt = format!(
        "Does this Q&A contain a reusable technical insight, design decision, or explanation worth saving to the project graph? \
         If yes, extract it as a single concise paragraph. If no, reply with exactly: SKIP\n\nQ: {}\nA: {}",
        question,
        &answer[..answer.len().min(800)]
    );

    if let Ok(loci_llm::LlmResponse::Text(text)) = llm.chat(
        vec![Message { role: Role::User, content: prompt }],
        None,
    ).await {
        let text = text.trim().to_string();
        if text == "SKIP" || text.is_empty() {
            return;
        }

        let concept = Node {
            id: Uuid::new_v4(),
            kind: NodeKind::Concept,
            name: format!("Insight: {}", question.chars().take(80).collect::<String>()),
            file_path: None,
            description: Some(text.clone()),
            raw_source: Some(format!("Q: {}\nA: {}", question, answer)),
            created_at: Utc::now(),
        };

        if graph_store.save_node(&concept).await.is_ok() {
            if let Ok(vector) = llm.embed(&text).await {
                let _ = vector_index.upsert(concept.id, &vector).await;
                if let Ok(hits) = vector_index.search(&vector, 3).await {
                    for (node_id, _) in hits {
                        if node_id == concept.id {
                            continue;
                        }
                        if graph.nodes.iter().any(|node| node.id == node_id) {
                            let edge = Edge {
                                id: Uuid::new_v4(),
                                from: node_id,
                                to: concept.id,
                                kind: EdgeKind::ExplainedBy,
                                label: Some("auto-extracted insight".to_string()),
                            };
                            let _ = graph_store.save_edge(&edge).await;
                        }
                    }
                }
            }
        }
    }
}

async fn persist_answer_artifacts(
    question: &str,
    answer: &str,
    llm: &dyn LlmClient,
    graph: &KnowledgeGraph,
    graph_store: &GraphStore,
    vector_index: &VectorIndex,
    memory_store: &MemoryStore,
) {
    let memory_text = format!("Q: {}\nA: {}", question, &answer[..answer.len().min(500)]);
    let memory_vector = llm.embed(&memory_text).await.ok();
    let _ = remember(memory_store, &memory_text, MemoryScope::Session, None, memory_vector).await;
    auto_extract_knowledge(question, answer, llm, graph, graph_store, vector_index).await;
}

async fn ask_local(project_path: &Path, question: &str, provider: Option<&str>) -> Result<String, String> {
    let cfg = BsConfig::load(project_path).unwrap_or_default();
    let llm = require_llm(&cfg, provider)?;
    let store = GraphStore::new(&graph_db_path(project_path)).await.map_err(|e| e.to_string())?;
    let graph = store.load_graph().await.map_err(|e| e.to_string())?;
    if graph.nodes.is_empty() {
        return Err(format!("No index found. Run: loci index --path {}", project_path.display()));
    }

    let memory_store = MemoryStore::new(&memory_db_path(project_path)).await.map_err(|e| e.to_string())?;
    let knowledge_store = KnowledgeStore::new(&knowledge_db_path(project_path)).await.map_err(|e| e.to_string())?;
    let vector_index = VectorIndex::new(store.pool.clone()).await.map_err(|e| e.to_string())?;
    let has_embeddings = vector_index.count().await.map_err(|e| e.to_string())? > 0;
    let q_vec = llm.embed(question).await.ok();

    let graph_ctx = build_graph_context(question, &q_vec, &graph, &vector_index, has_embeddings).await;
    let memory_ctx = build_memory_context(&q_vec, &memory_store).await;
    let kb_ctx = build_kb_context(&q_vec, &knowledge_store).await;
    let system = format!(
        "You are a codebase understanding assistant.\n\n## Knowledge Graph\n{}{}{}\n\nAnswer accurately.",
        graph_ctx, memory_ctx, kb_ctx
    );

    let response = llm.chat(
        vec![
            Message { role: Role::System, content: system },
            Message { role: Role::User, content: question.to_string() },
        ],
        None,
    ).await.map_err(|e| e.to_string())?;

    let answer = match response {
        loci_llm::LlmResponse::Text(text) => text,
        _ => String::new(),
    };

    persist_answer_artifacts(question, &answer, &*llm, &graph, &store, &vector_index, &memory_store).await;
    Ok(answer)
}

async fn score_eval_answer(llm: &dyn LlmClient, sample: &EvalSample, answer: &str) -> Result<EvalScore, String> {
    let prompt = format!(
        "Score this codebase-understanding answer on a 0-5 scale.\n\
         Judge accuracy, specificity, use of design decisions/concepts, and usefulness to a developer.\n\
         Respond with JSON only: {{\"score\": <0-5>, \"rationale\": \"...\"}}\n\n\
         Category: {}\nPrompt: {}\nAnswer:\n{}",
        sample.category,
        sample.prompt,
        &answer[..answer.len().min(4000)]
    );

    let response = llm.chat(vec![Message { role: Role::User, content: prompt }], None).await.map_err(|e| e.to_string())?;
    match response {
        loci_llm::LlmResponse::Text(text) => Ok(serde_json::from_str::<EvalScore>(&text).unwrap_or_else(|_| EvalScore {
            score: 0,
            rationale: text,
        })),
        _ => Ok(EvalScore {
            score: 0,
            rationale: "Scorer returned non-text output.".to_string(),
        }),
    }
}

fn load_eval_samples(project_path: &Path) -> Result<Vec<EvalSample>, String> {
    let sample_path = project_path.join("docs/eval/samples.json");
    if sample_path.exists() {
        let text = std::fs::read_to_string(&sample_path).map_err(|e| e.to_string())?;
        return serde_json::from_str(&text).map_err(|e| e.to_string());
    }

    Ok(vec![
        EvalSample {
            category: "Architecture".to_string(),
            prompt: "Summarize the high-level architecture of this repository.".to_string(),
        },
        EvalSample {
            category: "Key Decisions".to_string(),
            prompt: "What are the most important design decisions in this codebase, and why do they matter?".to_string(),
        },
        EvalSample {
            category: "Traceability".to_string(),
            prompt: "Pick one important implementation area and explain how its recent history evolved.".to_string(),
        },
        EvalSample {
            category: "Getting Started".to_string(),
            prompt: "If a new developer joined today, where should they start and what should they understand first?".to_string(),
        },
    ])
}

async fn eval_local(project_path: &Path, provider: Option<&str>) -> Result<EvalData, String> {
    let cfg = BsConfig::load(project_path).unwrap_or_default();
    let llm = require_llm(&cfg, provider)?;
    let samples = load_eval_samples(project_path)?;
    let mut results = Vec::new();

    for sample in samples {
        let answer = ask_local(project_path, &sample.prompt, provider).await?;
        let score = score_eval_answer(&*llm, &sample, &answer).await.unwrap_or_else(|_| EvalScore {
            score: 0,
            rationale: "Scoring failed.".to_string(),
        });
        results.push(EvalResult {
            category: sample.category,
            prompt: sample.prompt,
            answer,
            score,
        });
    }

    let average_score = if results.is_empty() {
        0.0
    } else {
        results.iter().map(|result| result.score.score as f32).sum::<f32>() / results.len() as f32
    };

    Ok(EvalData {
        average_score,
        results,
        drift_check: vec![
            "Runs against the locally indexed graph without requiring an external HTTP service.".to_string(),
            "Reuses the same in-app graph, memory, knowledge, and decision context used by desktop ask.".to_string(),
        ],
    })
}

#[tauri::command]
pub async fn get_default_project_path() -> Result<String, String> {
    Ok(default_project_path())
}

#[tauri::command]
pub async fn index_project(project_path: String) -> Result<String, String> {
    let project_path = resolve_project_path(&project_path);
    let (files, lines, nodes, edges) = build_graph_local(&project_path).await?;
    Ok(format!("{} files, {} lines, {} nodes, {} edges", files, lines, nodes, edges))
}

#[tauri::command]
pub async fn ask(project_path: String, question: String) -> Result<String, String> {
    let project_path = resolve_project_path(&project_path);
    ask_local(&project_path, &question, None).await
}

#[tauri::command]
pub async fn get_graph(project_path: String) -> Result<GraphData, String> {
    let project_path = resolve_project_path(&project_path);
    let store = GraphStore::new(&graph_db_path(&project_path)).await.map_err(|e| e.to_string())?;
    let graph = store.load_graph().await.map_err(|e| e.to_string())?;

    Ok(GraphData {
        nodes: graph.nodes.iter().map(map_node).collect(),
        edges: graph.edges.iter().map(map_edge).collect(),
    })
}

#[tauri::command]
pub async fn get_trace(project_path: String, target: String) -> Result<TraceData, String> {
    let project_path = resolve_project_path(&project_path);
    let store = GraphStore::new(&graph_db_path(&project_path)).await.map_err(|e| e.to_string())?;
    let graph = store.load_graph().await.map_err(|e| e.to_string())?;
    let query = target.trim().to_lowercase();

    let anchor_ids: HashSet<Uuid> = graph.nodes.iter()
        .filter(|node| {
            if query.is_empty() {
                false
            } else {
                node.name.to_lowercase().contains(&query)
                    || node.file_path.as_ref().map(|path| path.to_lowercase().contains(&query)).unwrap_or(false)
            }
        })
        .map(|node| node.id)
        .collect();

    let mut decision_ids = HashSet::new();
    let mut commit_ids = HashSet::new();
    let mut evidence = Vec::new();
    let mut related_ids = HashSet::new();

    if anchor_ids.is_empty() {
        for node in graph.nodes.iter().filter(|node| node.kind == NodeKind::Decision).rev().take(8) {
            decision_ids.insert(node.id);
        }
    } else {
        for edge in &graph.edges {
            if edge.kind == EdgeKind::ExplainedBy {
                if anchor_ids.contains(&edge.from) {
                    decision_ids.insert(edge.to);
                }
                if anchor_ids.contains(&edge.to) {
                    decision_ids.insert(edge.from);
                }
            }
            if edge.kind == EdgeKind::ChangedIn {
                if anchor_ids.contains(&edge.from) {
                    commit_ids.insert(edge.to);
                }
                if anchor_ids.contains(&edge.to) {
                    commit_ids.insert(edge.from);
                }
            }
        }
    }

    for edge in &graph.edges {
        if is_evidence_edge(&edge.kind) && (decision_ids.contains(&edge.from) || decision_ids.contains(&edge.to)) {
            evidence.push(map_edge(edge));
            if !decision_ids.contains(&edge.from) {
                related_ids.insert(edge.from);
            }
            if !decision_ids.contains(&edge.to) {
                related_ids.insert(edge.to);
            }
        }
    }

    for edge in &graph.edges {
        if edge.kind == EdgeKind::ChangedIn && (decision_ids.contains(&edge.from) || decision_ids.contains(&edge.to)) {
            if !decision_ids.contains(&edge.from) {
                commit_ids.insert(edge.from);
            }
            if !decision_ids.contains(&edge.to) {
                commit_ids.insert(edge.to);
            }
        }
    }

    Ok(TraceData {
        anchors: graph.nodes.iter().filter(|node| anchor_ids.contains(&node.id)).map(map_node).collect(),
        decisions: graph.nodes.iter().filter(|node| decision_ids.contains(&node.id)).map(map_node).collect(),
        commits: graph.nodes.iter()
            .filter(|node| commit_ids.contains(&node.id) && node.kind == NodeKind::Commit)
            .map(map_node)
            .collect(),
        evidence,
        related: graph.nodes.iter().filter(|node| related_ids.contains(&node.id)).map(map_node).collect(),
    })
}

#[tauri::command]
pub async fn get_doc(project_path: String, kind: String, provider: Option<String>) -> Result<DocData, String> {
    let project_path = resolve_project_path(&project_path);
    let cfg = BsConfig::load(&project_path).unwrap_or_default();
    let llm = require_llm(&cfg, provider.as_deref())?;
    let store = GraphStore::new(&graph_db_path(&project_path)).await.map_err(|e| e.to_string())?;
    let graph = store.load_graph().await.map_err(|e| e.to_string())?;
    if graph.nodes.is_empty() {
        return Err("No index. Run `loci index` first.".to_string());
    }

    let prompt = build_doc_prompt(&graph, &kind);
    let response = llm.chat(vec![Message { role: Role::User, content: prompt }], None).await.map_err(|e| e.to_string())?;
    let content = match response {
        loci_llm::LlmResponse::Text(text) => text,
        _ => String::new(),
    };
    Ok(DocData { kind, content })
}

#[tauri::command]
pub async fn get_eval(project_path: String, provider: Option<String>) -> Result<EvalData, String> {
    let project_path = resolve_project_path(&project_path);
    eval_local(&project_path, provider.as_deref()).await
}

#[tauri::command]
pub async fn get_memories(project_path: String) -> Result<Vec<String>, String> {
    let project_path = resolve_project_path(&project_path);
    let store = MemoryStore::new(&memory_db_path(&project_path)).await.map_err(|e| e.to_string())?;
    let memories = store.recall(None, None, None, 20).await.map_err(|e| e.to_string())?;
    Ok(memories.into_iter().map(|memory| memory.content).collect())
}
