#![recursion_limit = "512"]

use axum::{Router, routing::{post, get}, Json, extract::State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;
use loci_agent::Agent;
use loci_codebase::GitHistory;
use loci_llm::openai::OpenAiClient;
use loci_llm::config::BsConfig;
use loci_tools::{ToolRegistry, ToolContext, shell::ShellExec, file::{FileRead, FileWrite}, http::HttpRequest};
use loci_graph::{EdgeKind, GraphStore, NodeKind, VectorIndex};
use loci_memory::MemoryStore;
use loci_knowledge::{KnowledgeStore, ingest_file, ingest_url};
use loci_core::types::{Message, Role};

#[derive(Clone)]
struct AppState {
    agent: Arc<Agent>,
    project_path: Arc<RwLock<String>>,
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

#[derive(Deserialize)]
struct ProjectAddRequest {
    name: String,
    path: String,
}

#[derive(Deserialize)]
struct ProjectUseRequest {
    name: String,
}

#[derive(Deserialize)]
struct ProjectRemoveRequest {
    name: String,
}

#[derive(Deserialize)]
struct KnowledgeAddRequest {
    source: String,
    provider: Option<String>,
}

#[derive(Deserialize)]
struct KnowledgeSearchRequest {
    query: String,
    provider: Option<String>,
}

#[derive(Deserialize)]
struct HistoryRequest {
    file: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct ExplainRequest {
    target: String,
    selected_text: Option<String>,
    provider: Option<String>,
}

#[derive(Deserialize)]
struct DiffRequest {
    provider: Option<String>,
}

#[derive(Deserialize)]
struct EvalRequest {
    provider: Option<String>,
}

#[derive(Deserialize)]
struct DocRequest {
    kind: Option<String>,
    provider: Option<String>,
}

#[derive(Deserialize)]
struct TraceRequest {
    target: Option<String>,
}

#[derive(Serialize)]
struct AskResponse {
    answer: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct ProjectEntry {
    name: String,
    path: String,
}

#[derive(Serialize, Deserialize, Default)]
struct ProjectRegistry {
    projects: Vec<ProjectEntry>,
    active: Option<String>,
}

#[derive(Serialize)]
struct ProjectListResponse {
    active: Option<String>,
    current_path: String,
    projects: Vec<ProjectEntry>,
}

#[derive(Serialize)]
struct KnowledgeItemResponse {
    source: String,
    content: String,
    created_at: String,
}

#[derive(Serialize)]
struct KnowledgeListResponse {
    items: Vec<KnowledgeItemResponse>,
}

#[derive(Serialize)]
struct KnowledgeAddResponse {
    source: String,
    chars: usize,
}

#[derive(Serialize)]
struct HistoryCommitResponse {
    hash: String,
    message: String,
    author: String,
    timestamp: i64,
}

#[derive(Serialize)]
struct HistoryResponse {
    scope: String,
    path: Option<String>,
    commits: Vec<HistoryCommitResponse>,
    blame_summary: Vec<(String, String)>,
}

#[derive(Serialize)]
struct ExplainResponse {
    answer: String,
    trace: TraceResponse,
}

#[derive(Serialize)]
struct DiffResponse {
    answer: String,
    trace: TraceResponse,
}

#[derive(Serialize)]
struct EvalResponse {
    average_score: f32,
    results: Vec<EvalResult>,
    drift_check: Vec<String>,
}

#[derive(Serialize)]
struct DocResponse {
    kind: String,
    content: String,
}

#[derive(Serialize)]
struct TraceNode {
    id: String,
    label: String,
    kind: String,
    description: Option<String>,
    file_path: Option<String>,
}

#[derive(Serialize)]
struct TraceEdge {
    from: String,
    to: String,
    kind: String,
}

#[derive(Serialize)]
struct TraceResponse {
    anchors: Vec<TraceNode>,
    decisions: Vec<TraceNode>,
    commits: Vec<TraceNode>,
    evidence: Vec<TraceEdge>,
    related: Vec<TraceNode>,
}

#[derive(Serialize, Deserialize)]
struct EvalSample {
    category: String,
    prompt: String,
}

#[derive(Serialize, Deserialize, Default)]
struct EvalScore {
    score: u8,
    rationale: String,
}

#[derive(Serialize)]
struct EvalResult {
    category: String,
    prompt: String,
    answer: String,
    score: EvalScore,
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

#[derive(Serialize)]
struct ApiErrorBody {
    code: &'static str,
    message: String,
}

#[derive(Serialize)]
struct ApiEnvelope<T> {
    ok: bool,
    api_version: &'static str,
    data: Option<T>,
    error: Option<ApiErrorBody>,
}

#[derive(Serialize)]
struct ApiMetaResponse {
    service: &'static str,
    version: &'static str,
    api_version: &'static str,
    legacy_routes: Vec<&'static str>,
    versioned_base: &'static str,
    error_codes: Vec<&'static str>,
}

type ApiResult<T> = (StatusCode, Json<ApiEnvelope<T>>);

fn api_ok<T: Serialize>(data: T) -> ApiResult<T> {
    (
        StatusCode::OK,
        Json(ApiEnvelope {
            ok: true,
            api_version: "v1",
            data: Some(data),
            error: None,
        }),
    )
}

fn api_err_status<T>(status: StatusCode, code: &'static str, message: impl Into<String>) -> ApiResult<T> {
    (
        status,
        Json(ApiEnvelope {
            ok: false,
            api_version: "v1",
            data: None,
            error: Some(ApiErrorBody {
                code,
                message: message.into(),
            }),
        }),
    )
}

fn api_err<T>(code: &'static str, message: impl Into<String>) -> ApiResult<T> {
    let status = match code {
        "empty_question" | "empty_target" | "empty_query" | "invalid_kind" | "invalid_path" => StatusCode::BAD_REQUEST,
        "project_not_found" => StatusCode::NOT_FOUND,
        "project_already_exists" => StatusCode::CONFLICT,
        "index_missing" => StatusCode::PRECONDITION_FAILED,
        "llm_not_configured" => StatusCode::SERVICE_UNAVAILABLE,
        "knowledge_ingest_failed" => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::BAD_REQUEST,
    };
    api_err_status(status, code, message)
}

// ── handlers ──────────────────────────────────────────────────────────────────

async fn handle_health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok", version: env!("CARGO_PKG_VERSION") })
}

async fn handle_api_meta() -> ApiResult<ApiMetaResponse> {
    api_ok(ApiMetaResponse {
        service: "loci-server",
        version: env!("CARGO_PKG_VERSION"),
        api_version: "v1",
        legacy_routes: vec![
            "/health",
            "/run",
            "/projects",
            "/knowledge",
            "/history",
            "/ask",
            "/explain",
            "/diff",
            "/doc",
            "/eval",
            "/trace",
            "/graph",
            "/memories",
        ],
        versioned_base: "/api/v1",
        error_codes: vec![
            "empty_question",
            "empty_target",
            "invalid_kind",
            "invalid_path",
            "project_not_found",
            "project_already_exists",
            "knowledge_ingest_failed",
            "empty_query",
            "index_missing",
            "llm_not_configured",
        ],
    })
}

async fn handle_openapi() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "loci server",
            "version": env!("CARGO_PKG_VERSION")
        },
        "tags": [
            { "name": "meta" },
            { "name": "projects" },
            { "name": "knowledge" },
            { "name": "history" },
            { "name": "qa" },
            { "name": "trace" },
            { "name": "graph" },
            { "name": "memory" },
            { "name": "agent" }
        ],
        "components": {
            "schemas": {
                "ApiErrorBody": {
                    "type": "object",
                    "required": ["code", "message"],
                    "properties": {
                        "code": { "type": "string" },
                        "message": { "type": "string" }
                    }
                },
                "ApiEnvelope": {
                    "type": "object",
                    "properties": {
                        "ok": { "type": "boolean" },
                        "api_version": { "type": "string" },
                        "data": { "type": ["object", "null"] },
                        "error": {
                            "oneOf": [
                                { "$ref": "#/components/schemas/ApiErrorBody" },
                                { "type": "null" }
                            ]
                        }
                    }
                },
                "ProjectEntry": {
                    "type": "object",
                    "required": ["name", "path"],
                    "properties": {
                        "name": { "type": "string" },
                        "path": { "type": "string" }
                    }
                },
                "ProjectListResponse": {
                    "type": "object",
                    "properties": {
                        "active": { "type": ["string", "null"] },
                        "current_path": { "type": "string" },
                        "projects": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/ProjectEntry" }
                        }
                    }
                },
                "KnowledgeItemResponse": {
                    "type": "object",
                    "properties": {
                        "source": { "type": "string" },
                        "content": { "type": "string" },
                        "created_at": { "type": "string" }
                    }
                },
                "KnowledgeListResponse": {
                    "type": "object",
                    "properties": {
                        "items": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/KnowledgeItemResponse" }
                        }
                    }
                },
                "KnowledgeAddResponse": {
                    "type": "object",
                    "properties": {
                        "source": { "type": "string" },
                        "chars": { "type": "integer" }
                    }
                },
                "HistoryCommitResponse": {
                    "type": "object",
                    "properties": {
                        "hash": { "type": "string" },
                        "message": { "type": "string" },
                        "author": { "type": "string" },
                        "timestamp": { "type": "integer" }
                    }
                },
                "HistoryResponse": {
                    "type": "object",
                    "properties": {
                        "scope": { "type": "string" },
                        "path": { "type": ["string", "null"] },
                        "commits": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/HistoryCommitResponse" }
                        },
                        "blame_summary": {
                            "type": "array",
                            "items": {
                                "type": "array",
                                "items": [{ "type": "string" }, { "type": "string" }],
                                "minItems": 2,
                                "maxItems": 2
                            }
                        }
                    }
                },
                "TraceNode": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "label": { "type": "string" },
                        "kind": { "type": "string" },
                        "description": { "type": ["string", "null"] },
                        "file_path": { "type": ["string", "null"] }
                    }
                },
                "TraceEdge": {
                    "type": "object",
                    "properties": {
                        "from": { "type": "string" },
                        "to": { "type": "string" },
                        "kind": { "type": "string" }
                    }
                },
                "TraceResponse": {
                    "type": "object",
                    "properties": {
                        "anchors": { "type": "array", "items": { "$ref": "#/components/schemas/TraceNode" } },
                        "decisions": { "type": "array", "items": { "$ref": "#/components/schemas/TraceNode" } },
                        "commits": { "type": "array", "items": { "$ref": "#/components/schemas/TraceNode" } },
                        "evidence": { "type": "array", "items": { "$ref": "#/components/schemas/TraceEdge" } },
                        "related": { "type": "array", "items": { "$ref": "#/components/schemas/TraceNode" } }
                    }
                },
                "AskResponse": {
                    "type": "object",
                    "properties": {
                        "answer": { "type": "string" }
                    }
                },
                "ExplainResponse": {
                    "type": "object",
                    "properties": {
                        "answer": { "type": "string" },
                        "trace": { "$ref": "#/components/schemas/TraceResponse" }
                    }
                },
                "DiffResponse": {
                    "type": "object",
                    "properties": {
                        "answer": { "type": "string" },
                        "trace": { "$ref": "#/components/schemas/TraceResponse" }
                    }
                },
                "DocResponse": {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string" },
                        "content": { "type": "string" }
                    }
                },
                "EvalScore": {
                    "type": "object",
                    "properties": {
                        "score": { "type": "integer" },
                        "rationale": { "type": "string" }
                    }
                },
                "EvalResult": {
                    "type": "object",
                    "properties": {
                        "category": { "type": "string" },
                        "prompt": { "type": "string" },
                        "answer": { "type": "string" },
                        "score": { "$ref": "#/components/schemas/EvalScore" }
                    }
                },
                "EvalResponse": {
                    "type": "object",
                    "properties": {
                        "average_score": { "type": "number" },
                        "results": { "type": "array", "items": { "$ref": "#/components/schemas/EvalResult" } },
                        "drift_check": { "type": "array", "items": { "type": "string" } }
                    }
                },
                "GraphResponse": {
                    "type": "object",
                    "properties": {
                        "nodes": { "type": "integer" },
                        "edges": { "type": "integer" },
                        "context": { "type": "string" }
                    }
                },
                "MemoryResponse": {
                    "type": "object",
                    "properties": {
                        "memories": { "type": "array", "items": { "type": "string" } }
                    }
                },
                "RunResponse": {
                    "type": "object",
                    "properties": {
                        "result": {}
                    }
                },
                "HealthResponse": {
                    "type": "object",
                    "properties": {
                        "status": { "type": "string" },
                        "version": { "type": "string" }
                    }
                },
                "RunRequest": {
                    "type": "object",
                    "properties": {
                        "goal": { "type": "string" },
                        "working_dir": { "type": ["string", "null"] }
                    }
                },
                "AskRequest": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string" },
                        "provider": { "type": ["string", "null"] }
                    }
                },
                "ExplainRequest": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string" },
                        "selected_text": { "type": ["string", "null"] },
                        "provider": { "type": ["string", "null"] }
                    }
                },
                "DiffRequest": {
                    "type": "object",
                    "properties": {
                        "provider": { "type": ["string", "null"] }
                    }
                },
                "DocRequest": {
                    "type": "object",
                    "properties": {
                        "kind": { "type": ["string", "null"] },
                        "provider": { "type": ["string", "null"] }
                    }
                },
                "EvalRequest": {
                    "type": "object",
                    "properties": {
                        "provider": { "type": ["string", "null"] }
                    }
                },
                "TraceRequest": {
                    "type": "object",
                    "properties": {
                        "target": { "type": ["string", "null"] }
                    }
                },
                "HistoryRequest": {
                    "type": "object",
                    "properties": {
                        "file": { "type": ["string", "null"] },
                        "limit": { "type": ["integer", "null"] }
                    }
                },
                "KnowledgeSearchRequest": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "provider": { "type": ["string", "null"] }
                    }
                },
                "KnowledgeAddRequest": {
                    "type": "object",
                    "properties": {
                        "source": { "type": "string" },
                        "provider": { "type": ["string", "null"] }
                    }
                },
                "ProjectAddRequest": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "path": { "type": "string" }
                    }
                },
                "ProjectUseRequest": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" }
                    }
                },
                "ProjectRemoveRequest": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" }
                    }
                }
            }
        },
        "servers": [
            { "url": "http://127.0.0.1:3000/api/v1" }
        ],
        "paths": {
            "/meta": { "get": { "tags": ["meta"], "summary": "API metadata", "responses": { "200": { "description": "Metadata envelope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/projects": { "get": { "tags": ["projects"], "summary": "List registered projects and active project", "responses": { "200": { "description": "Project list", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/projects/add": { "post": { "tags": ["projects"], "summary": "Register a project", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ProjectAddRequest" }}}}, "responses": { "200": { "description": "Project added", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/projects/use": { "post": { "tags": ["projects"], "summary": "Switch active project", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ProjectUseRequest" }}}}, "responses": { "200": { "description": "Project switched", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/projects/remove": { "post": { "tags": ["projects"], "summary": "Remove a project", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ProjectRemoveRequest" }}}}, "responses": { "200": { "description": "Project removed", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/knowledge": { "get": { "tags": ["knowledge"], "summary": "List external knowledge items", "responses": { "200": { "description": "Knowledge items", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/knowledge/add": { "post": { "tags": ["knowledge"], "summary": "Add file or URL knowledge", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/KnowledgeAddRequest" }}}}, "responses": { "200": { "description": "Knowledge added", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/knowledge/search": { "post": { "tags": ["knowledge"], "summary": "Search external knowledge", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/KnowledgeSearchRequest" }}}}, "responses": { "200": { "description": "Knowledge search results", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/history": { "post": { "tags": ["history"], "summary": "Get repo or file history", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/HistoryRequest" }}}}, "responses": { "200": { "description": "History data", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/ask": { "post": { "tags": ["qa"], "summary": "Ask a question about the active project", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/AskRequest" }}}}, "responses": { "200": { "description": "Answer", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/explain": { "post": { "tags": ["qa"], "summary": "Explain a file or selected code with trace context", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ExplainRequest" }}}}, "responses": { "200": { "description": "Explanation with trace", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/diff": { "post": { "tags": ["qa"], "summary": "Explain recent changes with trace context", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/DiffRequest" }}}}, "responses": { "200": { "description": "Diff analysis with trace", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/doc": { "post": { "tags": ["qa"], "summary": "Generate onboarding/module/handoff docs", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/DocRequest" }}}}, "responses": { "200": { "description": "Generated document", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/eval": { "post": { "tags": ["qa"], "summary": "Run eval samples against the active project", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/EvalRequest" }}}}, "responses": { "200": { "description": "Eval report", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/trace": { "post": { "tags": ["trace"], "summary": "Inspect decisions, commits, and evidence", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/TraceRequest" }}}}, "responses": { "200": { "description": "Trace graph slice", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/graph": { "get": { "tags": ["graph"], "summary": "Get graph summary and context", "responses": { "200": { "description": "Graph summary", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/memories": { "get": { "tags": ["memory"], "summary": "List recent session memories", "responses": { "200": { "description": "Memory items", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/run": { "post": { "tags": ["agent"], "summary": "Run the generic agent loop", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RunRequest" }}}}, "responses": { "200": { "description": "Agent result", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}},
            "/health": { "get": { "tags": ["meta"], "summary": "Service health", "responses": { "200": { "description": "Health status", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiEnvelope" }}}}}}}
        }
    }))
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
    Json(AskResponse {
        answer: answer_question(&state, &req.question, req.provider.as_deref()).await,
    })
}

fn registry_path() -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/bs");
    std::fs::create_dir_all(&dir).ok();
    dir.join("projects.json")
}

fn load_registry() -> ProjectRegistry {
    std::fs::read_to_string(registry_path())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_registry(registry: &ProjectRegistry) {
    if let Ok(content) = serde_json::to_string_pretty(registry) {
        let _ = std::fs::write(registry_path(), content);
    }
}

fn normalize_project_path(path: impl AsRef<std::path::Path>) -> String {
    std::fs::canonicalize(path.as_ref())
        .unwrap_or_else(|_| path.as_ref().to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn active_project_path_from_registry() -> Option<String> {
    let registry = load_registry();
    let active = registry.active?;
    registry.projects.into_iter()
        .find(|project| project.name == active)
        .map(|project| project.path)
}

fn default_project_path() -> String {
    active_project_path_from_registry().unwrap_or_else(|| {
        normalize_project_path(std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
    })
}

fn resolve_server_project_path(project_path: &str) -> String {
    let trimmed = project_path.trim();
    if trimmed.is_empty() || trimmed == "." {
        default_project_path()
    } else {
        normalize_project_path(trimmed)
    }
}

async fn current_project_path(state: &AppState) -> String {
    state.project_path.read().await.clone()
}

async fn project_has_index(project_path: &std::path::Path) -> bool {
    let db = project_path.join(".bs/graph.db");
    if let Ok(store) = GraphStore::new(&db.to_string_lossy()).await {
        if let Ok(graph) = store.load_graph().await {
            return !graph.nodes.is_empty();
        }
    }
    false
}

async fn handle_projects(State(state): State<AppState>) -> Json<ProjectListResponse> {
    let registry = load_registry();
    Json(ProjectListResponse {
        active: registry.active,
        current_path: current_project_path(&state).await,
        projects: registry.projects,
    })
}

async fn handle_project_add(
    State(state): State<AppState>,
    Json(req): Json<ProjectAddRequest>,
) -> Json<ProjectListResponse> {
    let mut registry = load_registry();
    let abs = std::fs::canonicalize(&req.path)
        .unwrap_or_else(|_| std::path::PathBuf::from(&req.path))
        .to_string_lossy()
        .to_string();
    registry.projects.retain(|project| project.name != req.name);
    registry.projects.push(ProjectEntry { name: req.name, path: abs });
    save_registry(&registry);

    Json(ProjectListResponse {
        active: registry.active,
        current_path: current_project_path(&state).await,
        projects: registry.projects,
    })
}

async fn handle_project_use(
    State(state): State<AppState>,
    Json(req): Json<ProjectUseRequest>,
) -> Json<ProjectListResponse> {
    let mut registry = load_registry();
    if let Some(project) = registry.projects.iter().find(|project| project.name == req.name).cloned() {
        registry.active = Some(project.name.clone());
        save_registry(&registry);
        *state.project_path.write().await = project.path;
    }

    Json(ProjectListResponse {
        active: registry.active,
        current_path: current_project_path(&state).await,
        projects: registry.projects,
    })
}

async fn handle_project_remove(
    State(state): State<AppState>,
    Json(req): Json<ProjectRemoveRequest>,
) -> Json<ProjectListResponse> {
    let mut registry = load_registry();
    let removed_active = registry.active.as_deref() == Some(req.name.as_str());
    registry.projects.retain(|project| project.name != req.name);
    if removed_active {
        registry.active = None;
        *state.project_path.write().await = default_project_path();
    }
    save_registry(&registry);

    Json(ProjectListResponse {
        active: registry.active,
        current_path: current_project_path(&state).await,
        projects: registry.projects,
    })
}

fn render_knowledge_source(source: &loci_core::types::KnowledgeSource) -> String {
    match source {
        loci_core::types::KnowledgeSource::File { path } => path.clone(),
        loci_core::types::KnowledgeSource::Url { url } => url.clone(),
        loci_core::types::KnowledgeSource::Conversation { .. } => "[conversation]".to_string(),
        loci_core::types::KnowledgeSource::Auto => "[auto]".to_string(),
    }
}

async fn handle_knowledge_list(State(state): State<AppState>) -> Json<KnowledgeListResponse> {
    let path = std::path::PathBuf::from(current_project_path(&state).await);
    let kb = match KnowledgeStore::new(&path.join(".bs/knowledge.db").to_string_lossy()).await {
        Ok(store) => store,
        Err(_) => {
            return Json(KnowledgeListResponse { items: Vec::new() });
        }
    };

    let items = kb.list_external(20).await.unwrap_or_default()
        .into_iter()
        .map(|item| KnowledgeItemResponse {
            source: render_knowledge_source(&item.source),
            content: item.content,
            created_at: item.created_at.to_rfc3339(),
        })
        .collect();

    Json(KnowledgeListResponse { items })
}

async fn handle_knowledge_search(
    State(state): State<AppState>,
    Json(req): Json<KnowledgeSearchRequest>,
) -> Json<KnowledgeListResponse> {
    let path = std::path::PathBuf::from(current_project_path(&state).await);
    let cfg = BsConfig::load(&path).unwrap_or_default();
    let kb = match KnowledgeStore::new(&path.join(".bs/knowledge.db").to_string_lossy()).await {
        Ok(store) => store,
        Err(_) => {
            return Json(KnowledgeListResponse { items: Vec::new() });
        }
    };

    let query_embedding = match cfg.build_client(req.provider.as_deref()) {
        Ok(llm) => llm.embed(&req.query).await.ok(),
        Err(_) => None,
    };

    let items = kb.search_external(query_embedding.as_deref(), Some(&req.query), 10).await.unwrap_or_default()
        .into_iter()
        .map(|item| KnowledgeItemResponse {
            source: render_knowledge_source(&item.source),
            content: item.content,
            created_at: item.created_at.to_rfc3339(),
        })
        .collect();

    Json(KnowledgeListResponse { items })
}

async fn handle_knowledge_add(
    State(state): State<AppState>,
    Json(req): Json<KnowledgeAddRequest>,
) -> Json<KnowledgeAddResponse> {
    let path = std::path::PathBuf::from(current_project_path(&state).await);
    let cfg = BsConfig::load(&path).unwrap_or_default();
    let kb = match KnowledgeStore::new(&path.join(".bs/knowledge.db").to_string_lossy()).await {
        Ok(store) => store,
        Err(_) => {
            return Json(KnowledgeAddResponse { source: req.source, chars: 0 });
        }
    };

    let llm = cfg.build_client(req.provider.as_deref()).ok();
    let source = req.source;
    let knowledge = if source.starts_with("http://") || source.starts_with("https://") {
        let embedding = if let Some(ref llm) = llm {
            llm.embed(&source).await.ok()
        } else {
            None
        };
        ingest_url(&kb, &source, embedding, None).await.ok()
    } else {
        let file_path = std::path::Path::new(&source);
        let file_content = std::fs::read_to_string(file_path).ok();
        let embedding = if let (Some(ref llm), Some(content)) = (llm.as_ref(), file_content.as_ref()) {
            llm.embed(&content[..content.len().min(2000)]).await.ok()
        } else {
            None
        };
        ingest_file(&kb, file_path, embedding, None).await.ok()
    };

    Json(KnowledgeAddResponse {
        source,
        chars: knowledge.as_ref().map(|item| item.content.len()).unwrap_or(0),
    })
}

async fn handle_history(
    State(state): State<AppState>,
    Json(req): Json<HistoryRequest>,
) -> Json<HistoryResponse> {
    let path = std::path::PathBuf::from(current_project_path(&state).await);
    let limit = req.limit.unwrap_or(10).max(1).min(50);

    if let Some(file) = req.file {
        let history = GitHistory::file_history(&path, &file, limit);
        return match history {
            Ok(history) => Json(HistoryResponse {
                scope: "file".to_string(),
                path: Some(history.path),
                commits: history.commits.into_iter().map(|commit| HistoryCommitResponse {
                    hash: commit.hash,
                    message: commit.message,
                    author: commit.author,
                    timestamp: commit.timestamp,
                }).collect(),
                blame_summary: history.blame_summary,
            }),
            Err(_) => Json(HistoryResponse {
                scope: "file".to_string(),
                path: Some(file),
                commits: Vec::new(),
                blame_summary: Vec::new(),
            }),
        };
    }

    let commits = GitHistory::recent_commits(&path, limit).unwrap_or_default();
    Json(HistoryResponse {
        scope: "repo".to_string(),
        path: None,
        commits: commits.into_iter().map(|commit| HistoryCommitResponse {
            hash: commit.hash,
            message: commit.message,
            author: commit.author,
            timestamp: commit.timestamp,
        }).collect(),
        blame_summary: Vec::new(),
    })
}

async fn handle_explain(
    State(state): State<AppState>,
    Json(req): Json<ExplainRequest>,
) -> Json<ExplainResponse> {
    let question = if let Some(selected) = req.selected_text.filter(|text| !text.trim().is_empty()) {
        format!(
            "Explain this code in the context of the repository.\n\
             Separate factual description from inferred rationale.\n\n```text\n{}\n```",
            selected.chars().take(4000).collect::<String>()
        )
    } else {
        format!(
            "Explain the file or symbol `{}` in the context of this repository.\n\
             Separate factual description from inferred rationale.",
            req.target
        )
    };

    let project_path = current_project_path(&state).await;
    Json(ExplainResponse {
        answer: answer_question(&state, &question, req.provider.as_deref()).await,
        trace: load_trace_response(&project_path, Some(&req.target)).await,
    })
}

async fn handle_diff(
    State(state): State<AppState>,
    Json(req): Json<DiffRequest>,
) -> Json<DiffResponse> {
    let question = "Analyze the recent git changes in this project. What changed, why did it change, and what should a maintainer pay attention to?".to_string();
    let project_path = current_project_path(&state).await;
    Json(DiffResponse {
        answer: answer_question(&state, &question, req.provider.as_deref()).await,
        trace: load_trace_response(&project_path, None).await,
    })
}

async fn answer_question(state: &AppState, question: &str, provider: Option<&str>) -> String {
    let path = std::path::PathBuf::from(current_project_path(state).await);
    let cfg = BsConfig::load(&path).unwrap_or_default();
    match cfg.build_client(provider) {
        Err(_) => "LLM not configured".to_string(),
        Ok(llm) => {
            let db = path.join(".bs/graph.db").to_string_lossy().to_string();
            let store = match GraphStore::new(&db).await { Ok(s) => s, Err(e) => return e.to_string() };
            let graph = store.load_graph().await.unwrap_or_default();
            let vi = VectorIndex::new(store.pool.clone()).await.unwrap();
            let has_emb = vi.count().await.unwrap_or(0) > 0;
            let mem_store = MemoryStore::new(&path.join(".bs/memory.db").to_string_lossy()).await.unwrap();
            let _kb_store = KnowledgeStore::new(&path.join(".bs/knowledge.db").to_string_lossy()).await.unwrap();

            let q_vec = llm.embed(question).await.ok();
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
                Message { role: Role::User, content: question.to_string() },
            ], None).await {
                Ok(loci_llm::LlmResponse::Text(t)) => t,
                _ => "No response".to_string(),
            }
        }
    }
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

async fn handle_doc(
    State(state): State<AppState>,
    Json(req): Json<DocRequest>,
) -> Json<DocResponse> {
    let kind = req.kind.unwrap_or_else(|| "onboarding".to_string());
    let path = std::path::PathBuf::from(current_project_path(&state).await);
    let cfg = BsConfig::load(&path).unwrap_or_default();

    let content = match cfg.build_client(req.provider.as_deref()) {
        Err(_) => "LLM not configured".to_string(),
        Ok(llm) => {
            let db = path.join(".bs/graph.db").to_string_lossy().to_string();
            match GraphStore::new(&db).await {
                Ok(store) => {
                    let graph = store.load_graph().await.unwrap_or_default();
                    if graph.nodes.is_empty() {
                        "No index. Run `loci index` first.".to_string()
                    } else {
                        let prompt = build_doc_prompt(&graph, &kind);
                        match llm.chat(vec![Message {
                            role: Role::User,
                            content: prompt,
                        }], None).await {
                            Ok(loci_llm::LlmResponse::Text(text)) => text,
                            _ => "No response".to_string(),
                        }
                    }
                }
                Err(e) => e.to_string(),
            }
        }
    };

    Json(DocResponse { kind, content })
}

fn load_eval_samples(project_path: &std::path::Path) -> Vec<EvalSample> {
    let sample_path = project_path.join("docs/eval/samples.json");
    if let Ok(text) = std::fs::read_to_string(&sample_path) {
        if let Ok(samples) = serde_json::from_str::<Vec<EvalSample>>(&text) {
            return samples;
        }
    }

    vec![
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
    ]
}

async fn score_eval_answer(llm: &dyn loci_llm::LlmClient, sample: &EvalSample, answer: &str) -> EvalScore {
    let prompt = format!(
        "Score this codebase-understanding answer on a 0-5 scale.\n\
         Judge accuracy, specificity, use of design decisions/concepts, and usefulness to a developer.\n\
         Respond with JSON only: {{\"score\": <0-5>, \"rationale\": \"...\"}}\n\n\
         Category: {}\nPrompt: {}\nAnswer:\n{}",
        sample.category,
        sample.prompt,
        &answer[..answer.len().min(4000)]
    );

    match llm.chat(vec![Message { role: Role::User, content: prompt }], None).await {
        Ok(loci_llm::LlmResponse::Text(text)) => serde_json::from_str::<EvalScore>(&text).unwrap_or_else(|_| EvalScore {
            score: 0,
            rationale: text,
        }),
        _ => EvalScore {
            score: 0,
            rationale: "Scorer returned non-text output.".to_string(),
        },
    }
}

async fn handle_eval(
    State(state): State<AppState>,
    Json(req): Json<EvalRequest>,
) -> Json<EvalResponse> {
    let path = std::path::PathBuf::from(current_project_path(&state).await);
    let cfg = BsConfig::load(&path).unwrap_or_default();
    let samples = load_eval_samples(&path);

    let response = match cfg.build_client(req.provider.as_deref()) {
        Err(_) => EvalResponse {
            average_score: 0.0,
            results: vec![],
            drift_check: vec!["LLM not configured.".to_string()],
        },
        Ok(llm) => {
            let mut results = Vec::new();
            for sample in samples {
                let answer = answer_question(&state, &sample.prompt, req.provider.as_deref()).await;
                let score = score_eval_answer(&*llm, &sample, &answer).await;
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
                results.iter().map(|r| r.score.score as f32).sum::<f32>() / results.len() as f32
            };

            EvalResponse {
                average_score,
                results,
                drift_check: vec![
                    "Validates codebase understanding quality on a real indexed project.".to_string(),
                    "Reuses the existing ask pipeline and project graph instead of introducing a generic framework.".to_string(),
                ],
            }
        }
    };

    Json(response)
}

fn trace_node(node: &loci_graph::Node) -> TraceNode {
    TraceNode {
        id: node.id.to_string(),
        label: node.name.clone(),
        kind: format!("{:?}", node.kind),
        description: node.description.clone(),
        file_path: node.file_path.clone(),
    }
}

fn trace_edge(edge: &loci_graph::Edge) -> TraceEdge {
    TraceEdge {
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

async fn load_trace_response(project_path: &str, target: Option<&str>) -> TraceResponse {
    let db = std::path::PathBuf::from(project_path).join(".bs/graph.db");
    let query = target.unwrap_or_default().trim().to_lowercase();

    match GraphStore::new(&db.to_string_lossy()).await {
        Ok(store) => {
            let graph = store.load_graph().await.unwrap_or_default();
            let anchor_ids: HashSet<uuid::Uuid> = graph.nodes.iter()
                .filter(|node| {
                    if query.is_empty() {
                        false
                    } else {
                        node.name.to_lowercase().contains(&query)
                            || node.file_path.as_ref().map(|value| value.to_lowercase().contains(&query)).unwrap_or(false)
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
                    evidence.push(trace_edge(edge));
                    if !decision_ids.contains(&edge.from) {
                        related_ids.insert(edge.from);
                    }
                    if !decision_ids.contains(&edge.to) {
                        related_ids.insert(edge.to);
                    }
                }
                if edge.kind == EdgeKind::ChangedIn && (decision_ids.contains(&edge.from) || decision_ids.contains(&edge.to)) {
                    if !decision_ids.contains(&edge.from) {
                        commit_ids.insert(edge.from);
                    }
                    if !decision_ids.contains(&edge.to) {
                        commit_ids.insert(edge.to);
                    }
                }
            }

            TraceResponse {
                anchors: graph.nodes.iter().filter(|node| anchor_ids.contains(&node.id)).map(trace_node).collect(),
                decisions: graph.nodes.iter().filter(|node| decision_ids.contains(&node.id)).map(trace_node).collect(),
                commits: graph.nodes.iter().filter(|node| commit_ids.contains(&node.id) && node.kind == NodeKind::Commit).map(trace_node).collect(),
                evidence,
                related: graph.nodes.iter().filter(|node| related_ids.contains(&node.id)).map(trace_node).collect(),
            }
        }
        Err(_) => TraceResponse {
            anchors: Vec::new(),
            decisions: Vec::new(),
            commits: Vec::new(),
            evidence: Vec::new(),
            related: Vec::new(),
        },
    }
}

async fn handle_trace(
    State(state): State<AppState>,
    Json(req): Json<TraceRequest>,
) -> Json<TraceResponse> {
    let project_path = current_project_path(&state).await;
    Json(load_trace_response(&project_path, req.target.as_deref()).await)
}

async fn handle_graph(State(state): State<AppState>) -> Json<GraphResponse> {
    let path = std::path::PathBuf::from(current_project_path(&state).await);
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
    let path = std::path::PathBuf::from(current_project_path(&state).await);
    let db = path.join(".bs/memory.db").to_string_lossy().to_string();
    match MemoryStore::new(&db).await {
        Ok(store) => {
            let mems = store.recall(None, None, None, 20).await.unwrap_or_default();
            Json(MemoryResponse { memories: mems.into_iter().map(|m| m.content).collect() })
        }
        Err(_) => Json(MemoryResponse { memories: vec![] }),
    }
}

async fn handle_health_v1() -> ApiResult<HealthResponse> {
    api_ok(HealthResponse { status: "ok", version: env!("CARGO_PKG_VERSION") })
}

async fn handle_run_v1(
    State(state): State<AppState>,
    Json(req): Json<RunRequest>,
) -> ApiResult<serde_json::Value> {
    let ctx = ToolContext {
        working_dir: req.working_dir,
        ..Default::default()
    };
    let result = state.agent.run(&req.goal, ctx).await
        .unwrap_or_else(|e| format!("Error: {}", e));
    api_ok(serde_json::json!({ "result": result }))
}

async fn handle_projects_v1(State(state): State<AppState>) -> ApiResult<ProjectListResponse> {
    let registry = load_registry();
    api_ok(ProjectListResponse {
        active: registry.active,
        current_path: current_project_path(&state).await,
        projects: registry.projects,
    })
}

async fn handle_project_add_v1(
    State(state): State<AppState>,
    Json(req): Json<ProjectAddRequest>,
) -> ApiResult<ProjectListResponse> {
    if req.name.trim().is_empty() || req.path.trim().is_empty() {
        return api_err("invalid_path", "Project name and path must not be empty");
    }
    let mut registry = load_registry();
    if registry.projects.iter().any(|project| project.name == req.name) {
        return api_err("project_already_exists", format!("Project '{}' already exists", req.name));
    }
    let abs = std::fs::canonicalize(&req.path)
        .unwrap_or_else(|_| std::path::PathBuf::from(&req.path))
        .to_string_lossy()
        .to_string();
    registry.projects.retain(|project| project.name != req.name);
    registry.projects.push(ProjectEntry { name: req.name, path: abs });
    save_registry(&registry);
    api_ok(ProjectListResponse {
        active: registry.active,
        current_path: current_project_path(&state).await,
        projects: registry.projects,
    })
}

async fn handle_project_use_v1(
    State(state): State<AppState>,
    Json(req): Json<ProjectUseRequest>,
) -> ApiResult<ProjectListResponse> {
    let mut registry = load_registry();
    if let Some(project) = registry.projects.iter().find(|project| project.name == req.name).cloned() {
        registry.active = Some(project.name.clone());
        save_registry(&registry);
        *state.project_path.write().await = project.path;
        return api_ok(ProjectListResponse {
            active: registry.active,
            current_path: current_project_path(&state).await,
            projects: registry.projects,
        });
    }
    api_err("project_not_found", format!("Project '{}' not found", req.name))
}

async fn handle_project_remove_v1(
    State(state): State<AppState>,
    Json(req): Json<ProjectRemoveRequest>,
) -> ApiResult<ProjectListResponse> {
    let mut registry = load_registry();
    if !registry.projects.iter().any(|project| project.name == req.name) {
        return api_err("project_not_found", format!("Project '{}' not found", req.name));
    }
    let removed_active = registry.active.as_deref() == Some(req.name.as_str());
    registry.projects.retain(|project| project.name != req.name);
    if removed_active {
        registry.active = None;
        *state.project_path.write().await = default_project_path();
    }
    save_registry(&registry);
    api_ok(ProjectListResponse {
        active: registry.active,
        current_path: current_project_path(&state).await,
        projects: registry.projects,
    })
}

async fn handle_knowledge_list_v1(State(state): State<AppState>) -> ApiResult<KnowledgeListResponse> {
    api_ok(handle_knowledge_list(State(state)).await.0)
}

async fn handle_knowledge_search_v1(
    State(state): State<AppState>,
    Json(req): Json<KnowledgeSearchRequest>,
) -> ApiResult<KnowledgeListResponse> {
    if req.query.trim().is_empty() {
        return api_err("empty_query", "Query must not be empty");
    }
    api_ok(handle_knowledge_search(State(state), Json(req)).await.0)
}

async fn handle_knowledge_add_v1(
    State(state): State<AppState>,
    Json(req): Json<KnowledgeAddRequest>,
) -> ApiResult<KnowledgeAddResponse> {
    if req.source.trim().is_empty() {
        return api_err("invalid_path", "Knowledge source must not be empty");
    }
    let response = handle_knowledge_add(State(state), Json(req)).await.0;
    if response.chars == 0 {
        return api_err("knowledge_ingest_failed", format!("Failed to ingest '{}'", response.source));
    }
    api_ok(response)
}

async fn handle_history_v1(
    State(state): State<AppState>,
    Json(req): Json<HistoryRequest>,
) -> ApiResult<HistoryResponse> {
    if req.file.as_deref().map(|file| file.trim().is_empty()).unwrap_or(false) {
        return api_err("invalid_path", "History file path must not be empty");
    }
    api_ok(handle_history(State(state), Json(req)).await.0)
}

async fn handle_ask_v1(
    State(state): State<AppState>,
    Json(req): Json<AskRequest>,
) -> ApiResult<AskResponse> {
    if req.question.trim().is_empty() {
        return api_err("empty_question", "Question must not be empty");
    }
    api_ok(handle_ask(State(state), Json(req)).await.0)
}

async fn handle_explain_v1(
    State(state): State<AppState>,
    Json(req): Json<ExplainRequest>,
) -> ApiResult<ExplainResponse> {
    if req.target.trim().is_empty() {
        return api_err("empty_target", "Target must not be empty");
    }
    api_ok(handle_explain(State(state), Json(req)).await.0)
}

async fn handle_diff_v1(
    State(state): State<AppState>,
    Json(req): Json<DiffRequest>,
) -> ApiResult<DiffResponse> {
    api_ok(handle_diff(State(state), Json(req)).await.0)
}

async fn handle_doc_v1(
    State(state): State<AppState>,
    Json(req): Json<DocRequest>,
) -> ApiResult<DocResponse> {
    if let Some(kind) = req.kind.as_deref() {
        if !matches!(kind, "onboarding" | "module" | "handoff") {
            return api_err("invalid_kind", format!("Unsupported doc kind '{}'", kind));
        }
    }
    let response = handle_doc(State(state), Json(req)).await.0;
    if response.content == "LLM not configured" {
        return api_err("llm_not_configured", response.content);
    }
    if response.content == "No index. Run `loci index` first." {
        return api_err("index_missing", response.content);
    }
    api_ok(response)
}

async fn handle_eval_v1(
    State(state): State<AppState>,
    Json(req): Json<EvalRequest>,
) -> ApiResult<EvalResponse> {
    let project_path = std::path::PathBuf::from(current_project_path(&state).await);
    if !project_has_index(&project_path).await {
        return api_err("index_missing", "No index. Run `loci index` first.");
    }
    let response = handle_eval(State(state), Json(req)).await.0;
    if response.results.is_empty() && response.drift_check.iter().any(|line| line == "LLM not configured.") {
        return api_err("llm_not_configured", "LLM not configured");
    }
    api_ok(response)
}

async fn handle_trace_v1(
    State(state): State<AppState>,
    Json(req): Json<TraceRequest>,
) -> ApiResult<TraceResponse> {
    let project_path = std::path::PathBuf::from(current_project_path(&state).await);
    if !project_has_index(&project_path).await {
        return api_err("index_missing", "No index. Run `loci index` first.");
    }
    api_ok(handle_trace(State(state), Json(req)).await.0)
}

async fn handle_graph_v1(State(state): State<AppState>) -> ApiResult<GraphResponse> {
    let project_path = std::path::PathBuf::from(current_project_path(&state).await);
    if !project_has_index(&project_path).await {
        return api_err("index_missing", "No index. Run `loci index` first.");
    }
    api_ok(handle_graph(State(state)).await.0)
}

async fn handle_memories_v1(State(state): State<AppState>) -> ApiResult<MemoryResponse> {
    api_ok(handle_memories(State(state)).await.0)
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

    let state = AppState {
        agent,
        project_path: Arc::new(RwLock::new(resolve_server_project_path(project_path))),
    };

    let api_v1 = Router::new()
        .route("/meta", get(handle_api_meta))
        .route("/health", get(handle_health_v1))
        .route("/openapi.json", get(handle_openapi))
        .route("/run", post(handle_run_v1))
        .route("/projects", get(handle_projects_v1))
        .route("/projects/add", post(handle_project_add_v1))
        .route("/projects/use", post(handle_project_use_v1))
        .route("/projects/remove", post(handle_project_remove_v1))
        .route("/knowledge", get(handle_knowledge_list_v1))
        .route("/knowledge/add", post(handle_knowledge_add_v1))
        .route("/knowledge/search", post(handle_knowledge_search_v1))
        .route("/history", post(handle_history_v1))
        .route("/ask", post(handle_ask_v1))
        .route("/explain", post(handle_explain_v1))
        .route("/diff", post(handle_diff_v1))
        .route("/doc", post(handle_doc_v1))
        .route("/eval", post(handle_eval_v1))
        .route("/trace", post(handle_trace_v1))
        .route("/graph", get(handle_graph_v1))
        .route("/memories", get(handle_memories_v1));

    let app = Router::new()
        .route("/meta",     get(handle_api_meta))
        .route("/openapi.json", get(handle_openapi))
        .route("/health",   get(handle_health))
        .route("/run",      post(handle_run))
        .route("/projects", get(handle_projects))
        .route("/projects/add", post(handle_project_add))
        .route("/projects/use", post(handle_project_use))
        .route("/projects/remove", post(handle_project_remove))
        .route("/knowledge", get(handle_knowledge_list))
        .route("/knowledge/add", post(handle_knowledge_add))
        .route("/knowledge/search", post(handle_knowledge_search))
        .route("/history",   post(handle_history))
        .route("/ask",      post(handle_ask))
        .route("/explain",  post(handle_explain))
        .route("/diff",     post(handle_diff))
        .route("/doc",      post(handle_doc))
        .route("/eval",     post(handle_eval))
        .route("/trace",    post(handle_trace))
        .route("/graph",    get(handle_graph))
        .route("/memories", get(handle_memories))
        .nest("/api/v1", api_v1)
        .with_state(state);

    let addr = format!("127.0.0.1:{}", port);
    tracing::info!("loci-server listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
