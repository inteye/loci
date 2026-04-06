// Tauri commands — called from the frontend via invoke()

use loci_graph::{EdgeKind, NodeKind};
use loci_llm::config::BsConfig;
use loci_core::types::{Message, Role};
use serde::Serialize;
use std::collections::HashSet;

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

#[derive(Serialize, serde::Deserialize)]
pub struct EvalScore {
    pub score: u8,
    pub rationale: String,
}

#[derive(Serialize, serde::Deserialize)]
pub struct EvalResult {
    pub category: String,
    pub prompt: String,
    pub answer: String,
    pub score: EvalScore,
}

#[derive(Serialize, serde::Deserialize)]
pub struct EvalData {
    pub average_score: f32,
    pub results: Vec<EvalResult>,
    pub drift_check: Vec<String>,
}

fn map_node(node: &loci_graph::Node) -> GraphNode {
    GraphNode {
        id: node.id.to_string(),
        label: node.name.clone(),
        kind: format!("{:?}", node.kind),
        description: node.description.clone(),
        file_path: node.file_path.clone(),
    }
}

fn map_edge(edge: &loci_graph::Edge) -> GraphEdge {
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

fn build_doc_prompt(graph: &loci_graph::KnowledgeGraph, kind: &str) -> String {
    let decisions: Vec<&loci_graph::Node> = graph.nodes.iter()
        .filter(|n| n.kind == NodeKind::Decision)
        .take(12)
        .collect();
    let concepts: Vec<&loci_graph::Node> = graph.nodes.iter()
        .filter(|n| n.kind == NodeKind::Concept)
        .take(12)
        .collect();
    let files: Vec<&loci_graph::Node> = graph.nodes.iter()
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

/// Load the knowledge graph for the given project path
#[tauri::command]
pub async fn get_graph(project_path: String) -> Result<GraphData, String> {
    let db = format!("{}/.bs/graph.db", project_path);
    let store = loci_graph::GraphStore::new(&db).await.map_err(|e| e.to_string())?;
    let graph = store.load_graph().await.map_err(|e| e.to_string())?;

    Ok(GraphData {
        nodes: graph.nodes.iter().map(|n| GraphNode {
            id: n.id.to_string(),
            label: n.name.clone(),
            kind: format!("{:?}", n.kind),
            description: n.description.clone(),
            file_path: n.file_path.clone(),
        }).collect(),
        edges: graph.edges.iter().map(|e| GraphEdge {
            from: e.from.to_string(),
            to: e.to.to_string(),
            kind: format!("{:?}", e.kind),
        }).collect(),
    })
}

/// Inspect trace-oriented nodes and edges for a file or symbol
#[tauri::command]
pub async fn get_trace(project_path: String, target: String) -> Result<TraceData, String> {
    let db = format!("{}/.bs/graph.db", project_path);
    let store = loci_graph::GraphStore::new(&db).await.map_err(|e| e.to_string())?;
    let graph = store.load_graph().await.map_err(|e| e.to_string())?;
    let query = target.trim().to_lowercase();

    let anchor_ids: HashSet<uuid::Uuid> = graph.nodes.iter()
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
        for node in graph.nodes.iter()
            .filter(|node| node.kind == NodeKind::Decision)
            .rev()
            .take(8)
        {
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
        anchors: graph.nodes.iter()
            .filter(|node| anchor_ids.contains(&node.id))
            .map(map_node)
            .collect(),
        decisions: graph.nodes.iter()
            .filter(|node| decision_ids.contains(&node.id))
            .map(map_node)
            .collect(),
        commits: graph.nodes.iter()
            .filter(|node| commit_ids.contains(&node.id) && node.kind == NodeKind::Commit)
            .map(map_node)
            .collect(),
        evidence,
        related: graph.nodes.iter()
            .filter(|node| related_ids.contains(&node.id))
            .map(map_node)
            .collect(),
    })
}

/// Generate a document from graph decisions, concepts, and files
#[tauri::command]
pub async fn get_doc(project_path: String, kind: String, provider: Option<String>) -> Result<DocData, String> {
    let cfg = BsConfig::load(std::path::Path::new(&project_path)).unwrap_or_default();
    let llm = cfg.build_client(provider.as_deref()).map_err(|e| e.to_string())?;

    let db = format!("{}/.bs/graph.db", project_path);
    let store = loci_graph::GraphStore::new(&db).await.map_err(|e| e.to_string())?;
    let graph = store.load_graph().await.map_err(|e| e.to_string())?;
    if graph.nodes.is_empty() {
        return Err("No index. Run `loci index` first.".to_string());
    }

    let prompt = build_doc_prompt(&graph, &kind);
    let response = llm.chat(vec![Message {
        role: Role::User,
        content: prompt,
    }], None).await.map_err(|e| e.to_string())?;

    let content = match response {
        loci_llm::LlmResponse::Text(text) => text,
        _ => String::new(),
    };

    Ok(DocData { kind, content })
}

/// Run evaluation through the local loci server
#[tauri::command]
pub async fn get_eval(_project_path: String, provider: Option<String>) -> Result<EvalData, String> {
    let url = "http://localhost:3000/eval";
    let client = reqwest::Client::new();
    let response = client.post(url)
        .json(&serde_json::json!({ "provider": provider }))
        .send().await
        .map_err(|_| "loci serve not running. Start it with: loci serve".to_string())?;

    response.json::<EvalData>().await.map_err(|e| e.to_string())
}

/// Ask a question — proxies to the loci HTTP server if running
#[tauri::command]
pub async fn ask(_project_path: String, question: String) -> Result<String, String> {
    // Try local loci serve first
    let url = "http://localhost:3000/ask";
    let client = reqwest::Client::new();
    if let Ok(resp) = client.post(url)
        .json(&serde_json::json!({ "question": question }))
        .send().await
    {
        if let Ok(body) = resp.json::<serde_json::Value>().await {
            if let Some(answer) = body["answer"].as_str() {
                return Ok(answer.to_string());
            }
        }
    }
    Err("loci serve not running. Start it with: loci serve".to_string())
}

/// Index the project
#[tauri::command]
pub async fn index_project(project_path: String) -> Result<String, String> {
    let path = std::path::Path::new(&project_path);
    let index = loci_codebase::CodebaseIndexer::index(path).map_err(|e| e.to_string())?;
    Ok(format!("{} files, {} lines", index.summary.files.len(), index.summary.total_lines))
}

/// Get recent memories
#[tauri::command]
pub async fn get_memories(project_path: String) -> Result<Vec<String>, String> {
    let db = format!("{}/.bs/memory.db", project_path);
    let store = loci_memory::MemoryStore::new(&db).await.map_err(|e| e.to_string())?;
    let mems = store.recall(None, None, None, 20).await.map_err(|e| e.to_string())?;
    Ok(mems.into_iter().map(|m| m.content).collect())
}
