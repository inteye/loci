use axum::{Router, routing::{post, get}, Json, extract::State};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use loci_agent::Agent;
use loci_llm::openai::OpenAiClient;
use loci_llm::config::BsConfig;
use loci_tools::{ToolRegistry, ToolContext, shell::ShellExec, file::{FileRead, FileWrite}, http::HttpRequest};
use loci_graph::{GraphStore, KnowledgeGraph, VectorIndex};
use loci_memory::MemoryStore;
use loci_knowledge::KnowledgeStore;
use loci_core::types::{Message, Role};

#[derive(Clone)]
struct AppState {
    agent: Arc<Agent>,
    project_path: String,
}

// ── request / response types ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct RunRequest {
    goal: String,
    working_dir: Option<String>,
}

#[derive(Deserialize)]
struct AskRequest {
    question: String,
    provider: Option<String>,
}

#[derive(Serialize)]
struct AskResponse {
    answer: String,
}

#[derive(Serialize)]
struct GraphResponse {
    nodes: usize,
    edges: usize,
    context: String,
}

#[derive(Serialize)]
struct MemoryResponse {
    memories: Vec<String>,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

// ── handlers ──────────────────────────────────────────────────────────────────

async fn handle_health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok", version: env!("CARGO_PKG_VERSION") })
}

async fn handle_run(
    State(state): State<AppState>,
    Json(req): Json<RunRequest>,
) -> Json<serde_json::Value> {
    let ctx = ToolContext {
        working_dir: req.working_dir,
        ..Default::default()
    };
    let result = state.agent.run(&req.goal, ctx).await
        .unwrap_or_else(|e| format!("Error: {}", e));
    Json(serde_json::json!({ "result": result }))
}

async fn handle_ask(
    State(state): State<AppState>,
    Json(req): Json<AskRequest>,
) -> Json<AskResponse> {
    let path = std::path::PathBuf::from(&state.project_path);
    let cfg = BsConfig::load(&path).unwrap_or_default();
    let answer = match cfg.build_client(req.provider.as_deref()) {
        Err(_) => "LLM not configured".to_string(),
        Ok(llm) => {
            let db = path.join(".bs/graph.db").to_string_lossy().to_string();
            let store = match GraphStore::new(&db).await { Ok(s) => s, Err(e) => return Json(AskResponse { answer: e.to_string() }) };
            let graph = store.load_graph().await.unwrap_or_default();
            let vi = VectorIndex::new(store.pool.clone()).await.unwrap();
            let has_emb = vi.count().await.unwrap_or(0) > 0;
            let mem_store = MemoryStore::new(&path.join(".bs/memory.db").to_string_lossy()).await.unwrap();
            let kb_store  = KnowledgeStore::new(&path.join(".bs/knowledge.db").to_string_lossy()).await.unwrap();

            let q_vec = llm.embed(&req.question).await.ok();
            let graph_ctx = if has_emb {
                if let Some(ref qv) = q_vec {
                    let hits = vi.search(qv, 20).await.unwrap_or_default();
                    let mut ids: std::collections::HashSet<uuid::Uuid> = hits.into_iter().map(|(id,_)| id).collect();
                    for id in ids.clone() { for (_, n) in graph.neighbors(id) { ids.insert(n.id); } }
                    graph.to_context_str_filtered(&ids)
                } else { graph.to_context_str(None) }
            } else { graph.to_context_str(None) };

            let mems = mem_store.recall(q_vec.as_deref(), None, None, 5).await.unwrap_or_default();
            let mem_ctx = if mems.is_empty() { String::new() } else {
                format!("\n## Past context\n{}", mems.iter().map(|m| format!("- {}", m.content)).collect::<Vec<_>>().join("\n"))
            };

            let system = format!("You are a codebase understanding assistant.\n\n## Knowledge Graph\n{}{}\n\nAnswer accurately.", graph_ctx, mem_ctx);
            match llm.chat(vec![
                Message { role: Role::System, content: system },
                Message { role: Role::User, content: req.question.clone() },
            ], None).await {
                Ok(loci_llm::LlmResponse::Text(t)) => t,
                _ => "No response".to_string(),
            }
        }
    };
    Json(AskResponse { answer })
}

async fn handle_graph(State(state): State<AppState>) -> Json<GraphResponse> {
    let path = std::path::PathBuf::from(&state.project_path);
    let db = path.join(".bs/graph.db").to_string_lossy().to_string();
    match GraphStore::new(&db).await {
        Ok(store) => {
            let graph = store.load_graph().await.unwrap_or_default();
            Json(GraphResponse {
                nodes: graph.nodes.len(),
                edges: graph.edges.len(),
                context: graph.to_context_str(None),
            })
        }
        Err(e) => Json(GraphResponse { nodes: 0, edges: 0, context: e.to_string() }),
    }
}

async fn handle_memories(State(state): State<AppState>) -> Json<MemoryResponse> {
    let path = std::path::PathBuf::from(&state.project_path);
    let db = path.join(".bs/memory.db").to_string_lossy().to_string();
    match MemoryStore::new(&db).await {
        Ok(store) => {
            let mems = store.recall(None, None, None, 20).await.unwrap_or_default();
            Json(MemoryResponse { memories: mems.into_iter().map(|m| m.content).collect() })
        }
        Err(_) => Json(MemoryResponse { memories: vec![] }),
    }
}

// ── server entry ──────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let port: u16 = std::env::var("BS_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(3000);
    let project = std::env::var("BS_PROJECT").unwrap_or_else(|_| ".".to_string());
    run_server(port, &project).await
}

pub async fn run_server(port: u16, project_path: &str) -> anyhow::Result<()> {
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "no-key".to_string());
    let base_url = std::env::var("LLM_BASE_URL").ok();
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());

    let llm = Arc::new(OpenAiClient::new(&api_key, base_url.as_deref(), &model));
    let mut registry = ToolRegistry::default();
    registry.register(ShellExec);
    registry.register(FileRead);
    registry.register(FileWrite);
    registry.register(HttpRequest);
    let tools = Arc::new(registry);
    let agent = Arc::new(Agent::new(llm, tools));

    let state = AppState { agent, project_path: project_path.to_string() };

    let app = Router::new()
        .route("/health",   get(handle_health))
        .route("/run",      post(handle_run))
        .route("/ask",      post(handle_ask))
        .route("/graph",    get(handle_graph))
        .route("/memories", get(handle_memories))
        .with_state(state);

    let addr = format!("127.0.0.1:{}", port);
    tracing::info!("loci-server listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
