// Tauri commands — called from the frontend via invoke()

use serde::{Deserialize, Serialize};

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
}

#[derive(Serialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
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
        }).collect(),
        edges: graph.edges.iter().map(|e| GraphEdge {
            from: e.from.to_string(),
            to: e.to.to_string(),
            kind: format!("{:?}", e.kind),
        }).collect(),
    })
}

/// Ask a question — proxies to the bs HTTP server if running, else calls LLM directly
#[tauri::command]
pub async fn ask(project_path: String, question: String) -> Result<String, String> {
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
