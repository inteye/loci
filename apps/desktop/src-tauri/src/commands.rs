use chrono::Utc;
use loci_agent::{TraceAgent, TraceReport};
use loci_codebase::{CodebaseIndexer, GitHistory, ParsedFile, SymbolKind};
use loci_core::types::{MemoryScope, Message, Role};
use loci_graph::{Edge, EdgeKind, GraphStore, KnowledgeGraph, Node, NodeKind, VectorIndex};
use loci_llm::{
    config::{BsConfig, ProviderConfig, ProviderProtocol},
    LlmClient,
};
use loci_memory::{remember, MemoryStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSettingsData {
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    pub api_key: String,
    pub api_key_env: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelSettingsData {
    pub config_path: String,
    pub default_provider: Option<String>,
    pub providers: Vec<ProviderSettingsData>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderModelsData {
    pub models: Vec<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProjectEntry {
    name: String,
    path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProjectRegistry {
    projects: Vec<ProjectEntry>,
    active: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedProjectData {
    pub url: String,
    pub project_name: String,
    pub project_path: String,
    pub reused_existing: bool,
    pub message: String,
}

fn normalize_project_path(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    let raw = path.to_string_lossy();
    let expanded = if raw == "~" || raw.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            let suffix = raw.strip_prefix("~/").unwrap_or("");
            PathBuf::from(home).join(suffix)
        } else {
            path.to_path_buf()
        }
    } else {
        path.to_path_buf()
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(expanded)
    };

    std::fs::canonicalize(&absolute)
        .unwrap_or(absolute)
        .to_string_lossy()
        .to_string()
}

fn registry_path() -> PathBuf {
    let dir = config_root_dir();
    std::fs::create_dir_all(&dir).ok();
    dir.join("projects.json")
}

fn config_root_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata).join("bs");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config/bs");
    }
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        return PathBuf::from(user_profile).join(".config/bs");
    }
    match (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        (Ok(drive), Ok(path)) => PathBuf::from(format!("{drive}{path}")).join(".config/bs"),
        _ => PathBuf::from(".bs"),
    }
}

fn load_project_registry() -> ProjectRegistry {
    std::fs::read_to_string(registry_path())
        .ok()
        .and_then(|content| serde_json::from_str::<ProjectRegistry>(&content).ok())
        .unwrap_or_default()
}

fn save_project_registry(registry: &ProjectRegistry) -> Result<(), String> {
    let content = serde_json::to_string_pretty(registry).map_err(|e| e.to_string())?;
    std::fs::write(registry_path(), content).map_err(|e| e.to_string())
}

fn remember_active_project(
    project_path: &Path,
    preferred_name: Option<&str>,
) -> Result<(), String> {
    let normalized = normalize_project_path(project_path);
    let mut registry = load_project_registry();
    let name = preferred_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .or_else(|| {
            Path::new(&normalized)
                .file_name()
                .and_then(|value| value.to_str())
                .map(|value| value.to_string())
        })
        .unwrap_or_else(|| "project".to_string());

    registry
        .projects
        .retain(|project| project.path != normalized);
    registry.projects.retain(|project| project.name != name);
    registry.projects.push(ProjectEntry {
        name: name.clone(),
        path: normalized,
    });
    registry.active = Some(name);
    save_project_registry(&registry)
}

fn default_project_path() -> String {
    let active = {
        let registry = load_project_registry();
        registry.active.as_ref().and_then(|active| {
            registry
                .projects
                .iter()
                .find(|project| project.name == *active)
                .map(|project| project.path.clone())
        })
    };

    active.unwrap_or_else(|| normalize_project_path("."))
}

fn github_import_root() -> PathBuf {
    let base = if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
    } else if let Ok(user_profile) = std::env::var("USERPROFILE") {
        PathBuf::from(user_profile)
    } else if let (Ok(drive), Ok(path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        PathBuf::from(format!("{drive}{path}"))
    } else {
        PathBuf::from(".")
    };
    base.join(".loci").join("projects")
}

fn parse_github_project_url(url: &str) -> Result<(String, String, String), String> {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("请输入 GitHub 仓库地址。".to_string());
    }

    let (slug, clone_protocol) = if let Some(rest) = trimmed.strip_prefix("git@github.com:") {
        (rest, "ssh")
    } else if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        (rest, "https")
    } else if let Some(rest) = trimmed.strip_prefix("http://github.com/") {
        (rest, "https")
    } else if let Some(rest) = trimmed.strip_prefix("github.com/") {
        (rest, "https")
    } else {
        return Err("目前只支持 GitHub 仓库地址，例如 https://github.com/owner/repo".to_string());
    };

    let slug = slug
        .split(['?', '#'])
        .next()
        .unwrap_or(slug)
        .trim_end_matches(".git");
    let parts = slug
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 2 {
        return Err("GitHub 仓库地址格式不完整，应包含 owner/repo。".to_string());
    }

    let owner = parts[0].to_string();
    let repo = parts[1].to_string();
    let clone_url = match clone_protocol {
        "ssh" => format!("git@github.com:{owner}/{repo}.git"),
        _ => format!("https://github.com/{owner}/{repo}.git"),
    };
    Ok((clone_url, owner, repo))
}

fn choose_clone_destination(root: &Path, folder_name: &str) -> (PathBuf, bool) {
    let preferred = root.join(folder_name);
    if preferred.join(".git").exists() {
        return (preferred, true);
    }
    if !preferred.exists() {
        return (preferred, false);
    }

    for index in 2..1000 {
        let candidate = root.join(format!("{folder_name}-{index}"));
        if !candidate.exists() {
            return (candidate, false);
        }
    }

    (
        root.join(format!("{folder_name}-{}", Uuid::new_v4())),
        false,
    )
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
    cfg.build_client(provider).map_err(|_| {
        "No LLM configured. Create .bs/config.toml and configure a LiteLLM, OpenAI-compatible, or Anthropic provider."
            .to_string()
    })
}

fn provider_protocol_to_string(protocol: &ProviderProtocol) -> String {
    match protocol {
        ProviderProtocol::OpenAi => "openai".to_string(),
        ProviderProtocol::LiteLlm => "litellm".to_string(),
        ProviderProtocol::Anthropic => "anthropic".to_string(),
    }
}

fn provider_protocol_from_string(value: &str) -> ProviderProtocol {
    match value.trim().to_lowercase().as_str() {
        "litellm" => ProviderProtocol::LiteLlm,
        "anthropic" => ProviderProtocol::Anthropic,
        _ => ProviderProtocol::OpenAi,
    }
}

async fn test_provider_connection_local(
    project_path: &Path,
    provider: Option<&str>,
) -> Result<String, String> {
    let cfg = BsConfig::load(project_path).unwrap_or_default();
    let llm = require_llm(&cfg, provider)?;
    let provider_name = provider
        .map(str::to_string)
        .or(cfg.default_provider.clone())
        .unwrap_or_else(|| "default".to_string());
    let response = llm
        .chat(
            vec![Message {
                role: Role::User,
                content: "请用简体中文简短确认模型连接可用。".to_string(),
            }],
            None,
        )
        .await
        .map_err(|e| e.to_string())?;

    let text = match response {
        loci_llm::LlmResponse::Text(text) => text,
        _ => "Received a non-text response from the model.".to_string(),
    };

    Ok(format!(
        "连接成功：provider={} model={}\n\n{}",
        provider_name,
        llm.model(),
        text
    ))
}

fn effective_base_url(provider: &ProviderSettingsData) -> String {
    let url = provider.base_url.trim();
    if !url.is_empty() {
        return url.to_string();
    }
    match provider.protocol.as_str() {
        "anthropic" => "https://api.anthropic.com/v1".to_string(),
        _ => "https://api.openai.com/v1".to_string(),
    }
}

fn build_auth_headers(
    provider: &ProviderSettingsData,
    request: reqwest::RequestBuilder,
) -> reqwest::RequestBuilder {
    let mut request = request;
    let key = provider.api_key.trim();
    if !key.is_empty() {
        match provider.protocol.as_str() {
            "anthropic" => {
                request = request
                    .header("x-api-key", key)
                    .header("anthropic-version", "2023-06-01");
            }
            _ => {
                request = request.bearer_auth(key);
            }
        }
    }
    request
}

async fn fetch_models_from_openai_compatible(
    client: &reqwest::Client,
    provider: &ProviderSettingsData,
) -> Result<Vec<String>, String> {
    let base_url = effective_base_url(provider);
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let response = build_auth_headers(provider, client.get(url))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !response.status().is_success() {
        return Err(format!("model list request failed: {}", response.status()));
    }

    let body: Value = response.json().await.map_err(|e| e.to_string())?;
    let mut models = body
        .get("data")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(|id| id.as_str()).map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if models.is_empty() && base_url.contains("11434") {
        let fallback_url = format!(
            "{}/api/tags",
            base_url.trim_end_matches("/v1").trim_end_matches('/')
        );
        let response = client
            .get(fallback_url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if response.status().is_success() {
            let body: Value = response.json().await.map_err(|e| e.to_string())?;
            models = body
                .get("models")
                .and_then(|value| value.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            item.get("name")
                                .and_then(|name| name.as_str())
                                .map(String::from)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
        }
    }

    if models.is_empty() {
        return Err("no models returned from provider".to_string());
    }

    models.sort();
    models.dedup();
    Ok(models)
}

async fn fetch_models_from_anthropic(
    client: &reqwest::Client,
    provider: &ProviderSettingsData,
) -> Result<Vec<String>, String> {
    let base_url = effective_base_url(provider);
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let response = build_auth_headers(provider, client.get(url))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !response.status().is_success() {
        return Err(format!("model list request failed: {}", response.status()));
    }

    let body: Value = response.json().await.map_err(|e| e.to_string())?;
    let mut models = body
        .get("data")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(|id| id.as_str()).map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if models.is_empty() {
        return Err("no models returned from provider".to_string());
    }

    models.sort();
    models.dedup();
    Ok(models)
}

fn settings_from_config(project_path: &Path, cfg: BsConfig) -> ModelSettingsData {
    ModelSettingsData {
        config_path: project_path
            .join(".bs/config.toml")
            .to_string_lossy()
            .to_string(),
        default_provider: cfg.default_provider,
        providers: cfg
            .providers
            .into_iter()
            .map(|provider| ProviderSettingsData {
                name: provider.name,
                protocol: provider_protocol_to_string(&provider.protocol),
                base_url: provider.base_url.unwrap_or_default(),
                api_key: provider.api_key.unwrap_or_default(),
                api_key_env: provider.api_key_env.unwrap_or_default(),
                model: provider.model,
            })
            .collect(),
    }
}

fn config_from_settings(settings: ModelSettingsData) -> Result<BsConfig, String> {
    let providers = settings
        .providers
        .into_iter()
        .map(|provider| {
            let name = provider.name.trim().to_string();
            let model = provider.model.trim().to_string();
            if name.is_empty() {
                return Err("Provider name cannot be empty.".to_string());
            }
            if model.is_empty() {
                return Err(format!("Provider '{}' is missing a model.", name));
            }

            Ok(ProviderConfig {
                name,
                protocol: provider_protocol_from_string(&provider.protocol),
                base_url: (!provider.base_url.trim().is_empty())
                    .then(|| provider.base_url.trim().to_string()),
                api_key: (!provider.api_key.trim().is_empty())
                    .then(|| provider.api_key.trim().to_string()),
                api_key_env: (!provider.api_key_env.trim().is_empty())
                    .then(|| provider.api_key_env.trim().to_string()),
                model,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    if providers.is_empty() {
        return Err("At least one provider is required.".to_string());
    }

    if let Some(default_provider) = settings.default_provider.as_deref() {
        if !default_provider.trim().is_empty()
            && !providers
                .iter()
                .any(|provider| provider.name == default_provider)
        {
            return Err(format!(
                "Default provider '{}' does not exist.",
                default_provider
            ));
        }
    }

    Ok(BsConfig {
        default_provider: settings.default_provider.and_then(|value| {
            let trimmed = value.trim().to_string();
            (!trimmed.is_empty()).then_some(trimmed)
        }),
        providers,
    })
}

fn build_doc_prompt(graph: &KnowledgeGraph, kind: &str) -> String {
    let decisions: Vec<&Node> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Decision)
        .take(12)
        .collect();
    let concepts: Vec<&Node> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Concept)
        .take(12)
        .collect();
    let files: Vec<&Node> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::File)
        .take(12)
        .collect();

    let context = format!(
        "## Decisions\n{}\n\n## Concepts\n{}\n\n## Files\n{}",
        decisions
            .iter()
            .map(|n| format!("- {}: {}", n.name, n.description.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n"),
        concepts
            .iter()
            .map(|n| format!("- {}: {}", n.name, n.description.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n"),
        files
            .iter()
            .map(|n| format!("- {}", n.name))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    match kind {
        "module" => format!(
            "请根据项目图谱生成一份模块概览文档。\n\
             默认使用简体中文输出。\n\
             先写事实，再写推断出的设计原因。\n\
             输出 Markdown，并包含这些章节：概览、关键模块、重要决策、开放问题。\n\n{}",
            context
        ),
        "handoff" => format!(
            "请根据项目图谱生成一份交接文档，面向新维护者。\n\
             默认使用简体中文输出。\n\
             先写事实，再写推断出的设计原因。\n\
             输出 Markdown，并包含这些章节：最重要的内容、关键决策、风险区域、开放问题。\n\n{}",
            context
        ),
        _ => format!(
            "请根据项目图谱生成一份给新同事的入门指南。\n\
             默认使用简体中文输出。\n\
             先写事实，再写推断出的设计原因。\n\
             输出 Markdown，并包含这些章节：项目概览、从哪里开始、重要决策、核心概念、开放问题。\n\n{}",
            context
        ),
    }
}

async fn build_graph_local(project_path: &Path) -> Result<(usize, usize, usize, usize), String> {
    let index = CodebaseIndexer::index(project_path).map_err(|e| e.to_string())?;
    let store = GraphStore::new(&graph_db_path(project_path))
        .await
        .map_err(|e| e.to_string())?;
    store.clear().await.map_err(|e| e.to_string())?;

    let mut graph = KnowledgeGraph::default();
    let mut commit_nodes: HashMap<String, Uuid> = HashMap::new();
    let parsed_by_path: HashMap<String, &ParsedFile> = index
        .parsed_files
        .iter()
        .map(|pf| (pf.path.clone(), pf))
        .collect();

    for file in index
        .summary
        .files
        .iter()
        .filter(|file| file.language.is_graph_source())
    {
        let file_path = file.path.to_string_lossy().to_string();
        let parsed = parsed_by_path.get(&file_path).copied();
        let file_node = Node {
            id: Uuid::new_v4(),
            kind: NodeKind::File,
            name: file.relative_path.clone(),
            file_path: Some(file_path.clone()),
            description: parsed.and_then(|pf| pf.doc_comment.clone()).or_else(|| {
                Some(format!(
                    "{:?} file, {} lines",
                    file.language, file.line_count
                ))
            }),
            raw_source: None,
            created_at: Utc::now(),
        };
        let file_id = graph.add_node(file_node.clone());
        store
            .save_node(&file_node)
            .await
            .map_err(|e| e.to_string())?;

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
                        created_at: chrono::DateTime::from_timestamp(commit.timestamp, 0)
                            .unwrap_or_else(Utc::now),
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
                store
                    .save_node(&symbol_node)
                    .await
                    .map_err(|e| e.to_string())?;
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
                let from_id = graph
                    .nodes
                    .iter()
                    .find(|node| {
                        &node.name == caller
                            && node.file_path.as_deref() == Some(file_path.as_str())
                    })
                    .map(|node| node.id);
                let to_id = graph
                    .nodes
                    .iter()
                    .find(|node| {
                        &node.name == callee
                            && node.file_path.as_deref() == Some(file_path.as_str())
                    })
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

fn is_decision_question(question: &str) -> bool {
    let q = question.to_lowercase();
    [
        "why",
        "为什么",
        "原因",
        "设计",
        "决策",
        "tradeoff",
        "权衡",
        "rationale",
        "architecture",
        "架构",
        "handoff",
        "onboarding",
        "演进",
        "history",
        "blame",
        "trace",
    ]
    .iter()
    .any(|needle| q.contains(needle))
}

fn node_priority(kind: &NodeKind, decision_query: bool, trace_query: bool) -> usize {
    match (trace_query, decision_query, kind) {
        (true, _, NodeKind::Decision) => 0,
        (true, _, NodeKind::Commit) => 1,
        (true, _, NodeKind::Concept) => 2,
        (true, _, NodeKind::File) => 3,
        (true, _, _) => 4,
        (false, true, NodeKind::Decision) => 0,
        (false, true, NodeKind::Concept) => 1,
        (false, true, NodeKind::File) => 2,
        (false, true, NodeKind::Function) => 3,
        (false, true, NodeKind::Module) => 3,
        (false, true, _) => 4,
        (false, false, NodeKind::File) => 0,
        (false, false, NodeKind::Module) => 1,
        (false, false, NodeKind::Function) => 2,
        (false, false, NodeKind::Decision) => 3,
        (false, false, NodeKind::Concept) => 4,
        (false, false, _) => 5,
    }
}

fn default_context_ids(
    graph: &KnowledgeGraph,
    decision_query: bool,
    trace_query: bool,
) -> HashSet<Uuid> {
    let mut ids = HashSet::new();
    let mut ranked = graph.nodes.iter().collect::<Vec<_>>();
    ranked.sort_by_key(|node| node_priority(&node.kind, decision_query, trace_query));

    for node in ranked.into_iter().take(if trace_query { 18 } else { 20 }) {
        ids.insert(node.id);
    }

    graph.expand_ids_with_neighbors(&ids, 1)
}

fn build_ranked_context_ids(
    question: &str,
    graph: &KnowledgeGraph,
    hits: Vec<(Uuid, f32)>,
) -> HashSet<Uuid> {
    let trace_query = is_trace_question(question);
    let decision_query = is_decision_question(question);
    let mut ranked = hits
        .into_iter()
        .filter_map(|(id, score)| {
            graph.node_by_id(id).map(|node| {
                (
                    id,
                    node_priority(&node.kind, decision_query, trace_query),
                    score,
                )
            })
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|left, right| {
        left.1.cmp(&right.1).then_with(|| {
            right
                .2
                .partial_cmp(&left.2)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });

    let mut ids = HashSet::new();
    for (id, _, _) in ranked.into_iter().take(if trace_query { 18 } else { 20 }) {
        ids.insert(id);
    }

    graph.expand_ids_with_neighbors(&ids, if trace_query { 2 } else { 1 })
}

fn render_graph_context(
    graph: &KnowledgeGraph,
    ids: &HashSet<Uuid>,
    decision_query: bool,
    trace_query: bool,
) -> String {
    let mut sections = Vec::new();

    let decisions = graph
        .nodes
        .iter()
        .filter(|node| ids.contains(&node.id) && node.kind == NodeKind::Decision)
        .map(|node| {
            format!(
                "- {}: {}",
                node.name,
                node.description.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>();
    if !decisions.is_empty() {
        sections.push(format!("## Prior Decisions\n{}", decisions.join("\n")));
    }

    let concepts = graph
        .nodes
        .iter()
        .filter(|node| ids.contains(&node.id) && node.kind == NodeKind::Concept)
        .map(|node| {
            format!(
                "- {}: {}",
                node.name,
                node.description.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>();
    if decision_query && !concepts.is_empty() {
        sections.push(format!("## Relevant Concepts\n{}", concepts.join("\n")));
    }

    if trace_query {
        let commits = graph
            .nodes
            .iter()
            .filter(|node| ids.contains(&node.id) && node.kind == NodeKind::Commit)
            .map(|node| {
                format!(
                    "- {}: {}",
                    node.name,
                    node.description.as_deref().unwrap_or("")
                )
            })
            .collect::<Vec<_>>();
        if !commits.is_empty() {
            sections.push(format!("## Relevant Commits\n{}", commits.join("\n")));
        }
    }

    sections.push(format!(
        "## Graph Context\n{}",
        graph.to_context_str_filtered(ids)
    ));

    sections.join("\n\n")
}

async fn build_graph_context(
    question: &str,
    q_vec: &Option<Vec<f32>>,
    graph: &KnowledgeGraph,
    vector_index: &VectorIndex,
    has_embeddings: bool,
) -> String {
    let trace_query = is_trace_question(question);
    let decision_query = is_decision_question(question);

    let ids = if has_embeddings {
        if let Some(vector) = q_vec {
            if let Ok(hits) = vector_index.search(vector, 40).await {
                build_ranked_context_ids(question, graph, hits)
            } else {
                default_context_ids(graph, decision_query, trace_query)
            }
        } else {
            default_context_ids(graph, decision_query, trace_query)
        }
    } else {
        default_context_ids(graph, decision_query, trace_query)
    };

    render_graph_context(graph, &ids, decision_query, trace_query)
}

async fn build_memory_context(q_vec: &Option<Vec<f32>>, store: &MemoryStore) -> String {
    let memories = store
        .recall(q_vec.as_deref(), Some(MemoryScope::Session), None, 5)
        .await
        .unwrap_or_default();
    if memories.is_empty() {
        return String::new();
    }
    format!(
        "\n## Past context\n{}",
        memories
            .iter()
            .map(|memory| format!("- {}", memory.content))
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

    if let Ok(loci_llm::LlmResponse::Text(text)) = llm
        .chat(
            vec![Message {
                role: Role::User,
                content: prompt,
            }],
            None,
        )
        .await
    {
        let text = text.trim().to_string();
        if text == "SKIP" || text.is_empty() {
            return;
        }

        let node_kind = if is_decision_question(question) {
            NodeKind::Decision
        } else {
            NodeKind::Concept
        };
        let concept = Node {
            id: Uuid::new_v4(),
            kind: node_kind.clone(),
            name: format!(
                "{}: {}",
                if node_kind == NodeKind::Decision {
                    "Decision"
                } else {
                    "Insight"
                },
                question.chars().take(80).collect::<String>()
            ),
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
    let _ = remember(
        memory_store,
        &memory_text,
        MemoryScope::Session,
        None,
        memory_vector,
    )
    .await;
    auto_extract_knowledge(question, answer, llm, graph, graph_store, vector_index).await;
}

async fn ask_local(
    project_path: &Path,
    question: &str,
    provider: Option<&str>,
) -> Result<String, String> {
    let cfg = BsConfig::load(project_path).unwrap_or_default();
    let llm = require_llm(&cfg, provider)?;
    let store = GraphStore::new(&graph_db_path(project_path))
        .await
        .map_err(|e| e.to_string())?;
    let graph = store.load_graph().await.map_err(|e| e.to_string())?;
    if graph.nodes.is_empty() {
        return Err(format!(
            "No index found. Run: loci index --path {}",
            project_path.display()
        ));
    }

    let memory_store = MemoryStore::new(&memory_db_path(project_path))
        .await
        .map_err(|e| e.to_string())?;
    let vector_index = VectorIndex::new(store.pool.clone())
        .await
        .map_err(|e| e.to_string())?;
    let has_embeddings = vector_index.count().await.map_err(|e| e.to_string())? > 0;
    let q_vec = llm.embed(question).await.ok();

    let graph_ctx =
        build_graph_context(question, &q_vec, &graph, &vector_index, has_embeddings).await;
    let memory_ctx = build_memory_context(&q_vec, &memory_store).await;
    let system = format!(
        "你是一个代码库理解助手。默认使用简体中文回答，除非用户明确要求其他语言。\n\
         回答要准确、直接，并把知识图谱视为项目事实主存。\n\
         Session memory 只用于保留对话上下文，不应覆盖图谱事实。\n\n## Knowledge Graph\n{}{}\n\n请给出清晰回答。",
        graph_ctx, memory_ctx
    );

    let response = llm
        .chat(
            vec![
                Message {
                    role: Role::System,
                    content: system,
                },
                Message {
                    role: Role::User,
                    content: question.to_string(),
                },
            ],
            None,
        )
        .await
        .map_err(|e| e.to_string())?;

    let answer = match response {
        loci_llm::LlmResponse::Text(text) => text,
        _ => String::new(),
    };

    persist_answer_artifacts(
        question,
        &answer,
        &*llm,
        &graph,
        &store,
        &vector_index,
        &memory_store,
    )
    .await;
    Ok(answer)
}

async fn score_eval_answer(
    llm: &dyn LlmClient,
    sample: &EvalSample,
    answer: &str,
) -> Result<EvalScore, String> {
    let prompt = format!(
        "Score this codebase-understanding answer on a 0-5 scale.\n\
         Judge accuracy, specificity, use of design decisions/concepts, and usefulness to a developer.\n\
         Respond with JSON only: {{\"score\": <0-5>, \"rationale\": \"...\"}}\n\n\
         Category: {}\nPrompt: {}\nAnswer:\n{}",
        sample.category,
        sample.prompt,
        &answer[..answer.len().min(4000)]
    );

    let response = llm
        .chat(
            vec![Message {
                role: Role::User,
                content: prompt,
            }],
            None,
        )
        .await
        .map_err(|e| e.to_string())?;
    match response {
        loci_llm::LlmResponse::Text(text) => Ok(serde_json::from_str::<EvalScore>(&text)
            .unwrap_or_else(|_| EvalScore {
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
        let score = score_eval_answer(&*llm, &sample, &answer)
            .await
            .unwrap_or_else(|_| EvalScore {
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
        results
            .iter()
            .map(|result| result.score.score as f32)
            .sum::<f32>()
            / results.len() as f32
    };

    Ok(EvalData {
        average_score,
        results,
        drift_check: vec![
            "评测直接基于本地索引后的图谱运行，不依赖额外 HTTP 服务。".to_string(),
            "评测复用了桌面端问答使用的图谱、记忆、知识和决策上下文。".to_string(),
        ],
    })
}

fn extract_commit_hashes(detail: &str) -> Vec<String> {
    let mut hashes = Vec::new();
    let mut current = String::new();

    for ch in detail.chars() {
        if ch.is_ascii_hexdigit() {
            current.push(ch);
        } else {
            if (7..=40).contains(&current.len()) {
                hashes.push(current.clone());
            }
            current.clear();
        }
    }

    if (7..=40).contains(&current.len()) {
        hashes.push(current);
    }

    hashes
}

fn extract_changed_files_from_diff(diff: &str) -> Vec<String> {
    let mut files = diff
        .lines()
        .filter_map(|line| {
            line.strip_prefix("+++ b/")
                .or_else(|| line.strip_prefix("--- a/"))
        })
        .filter(|line| *line != "/dev/null")
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    files
}

fn is_symbol_like_kind(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Function | NodeKind::Struct | NodeKind::Enum | NodeKind::Trait | NodeKind::Module
    )
}

fn collect_anchor_scope_ids(graph: &KnowledgeGraph, anchor_ids: &[Uuid]) -> HashSet<Uuid> {
    let seeds = anchor_ids.iter().copied().collect::<HashSet<_>>();
    graph.expand_ids_with_neighbors(&seeds, 2)
}

fn match_file_or_symbol_ids(
    graph: &KnowledgeGraph,
    detail: &str,
    anchor_scope_ids: &HashSet<Uuid>,
) -> Vec<Uuid> {
    let detail_lower = detail.to_lowercase();
    graph
        .nodes
        .iter()
        .filter(|node| {
            if !anchor_scope_ids.is_empty() && !anchor_scope_ids.contains(&node.id) {
                return false;
            }
            if node.kind == NodeKind::File {
                return detail_lower.contains(&node.name.to_lowercase())
                    || node
                        .file_path
                        .as_deref()
                        .map(|path| detail_lower.contains(&path.to_lowercase()))
                        .unwrap_or(false);
            }
            is_symbol_like_kind(&node.kind) && detail_lower.contains(&node.name.to_lowercase())
        })
        .map(|node| node.id)
        .collect()
}

async fn persist_trace_decision(
    store: &GraphStore,
    vector_index: &VectorIndex,
    llm: &dyn LlmClient,
    graph: &KnowledgeGraph,
    anchor_ids: &[Uuid],
    title: &str,
    report: &TraceReport,
) -> Result<Uuid, String> {
    let decision = Node {
        id: Uuid::new_v4(),
        kind: NodeKind::Decision,
        name: title.to_string(),
        file_path: None,
        description: Some(report.summary.clone()),
        raw_source: Some(serde_json::to_string_pretty(report).map_err(|e| e.to_string())?),
        created_at: Utc::now(),
    };
    store
        .save_node(&decision)
        .await
        .map_err(|e| e.to_string())?;

    if let Ok(vector) = llm.embed(&format!("{}\n{}", title, report.summary)).await {
        let _ = vector_index.upsert(decision.id, &vector).await;
    }

    for anchor_id in anchor_ids {
        if graph.nodes.iter().any(|node| node.id == *anchor_id) {
            let edge = Edge {
                id: Uuid::new_v4(),
                from: *anchor_id,
                to: decision.id,
                kind: EdgeKind::ExplainedBy,
                label: Some("trace report".to_string()),
            };
            store.save_edge(&edge).await.map_err(|e| e.to_string())?;
        }
    }

    persist_trace_evidence_edges(store, graph, decision.id, anchor_ids, report).await?;
    Ok(decision.id)
}

async fn persist_trace_evidence_edges(
    store: &GraphStore,
    graph: &KnowledgeGraph,
    decision_id: Uuid,
    anchor_ids: &[Uuid],
    report: &TraceReport,
) -> Result<(), String> {
    let anchor_scope_ids = collect_anchor_scope_ids(graph, anchor_ids);
    for evidence in &report.evidence {
        let source = evidence.source.to_lowercase();
        let mut matched_ids: Vec<Uuid> = Vec::new();
        let mut edge_kind: Option<EdgeKind> = None;

        if source.contains("commit") || source.contains("blame") || source.contains("diff") {
            edge_kind = Some(EdgeKind::EvidenceFromCommit);
            for hash in extract_commit_hashes(&evidence.detail) {
                matched_ids.extend(
                    graph
                        .nodes
                        .iter()
                        .filter(|node| {
                            node.kind == NodeKind::Commit
                                && (node.name == hash || node.name.starts_with(&hash))
                        })
                        .map(|node| node.id),
                );
            }
        }

        if source.contains("code") || source.contains("file") {
            edge_kind.get_or_insert(EdgeKind::EvidenceFromFile);
            matched_ids.extend(anchor_ids.iter().copied());
            matched_ids.extend(match_file_or_symbol_ids(
                graph,
                &evidence.detail,
                &anchor_scope_ids,
            ));
        }

        if source.contains("diff") {
            edge_kind.get_or_insert(EdgeKind::EvidenceFromFile);
            for changed in extract_changed_files_from_diff(&evidence.detail) {
                matched_ids.extend(
                    graph
                        .nodes
                        .iter()
                        .filter(|node| {
                            node.kind == NodeKind::File
                                && (node.name == changed
                                    || node.file_path.as_deref() == Some(changed.as_str()))
                        })
                        .map(|node| node.id),
                );
            }
        }

        if source.contains("decision") || source.contains("concept") {
            edge_kind.get_or_insert(if source.contains("decision") {
                EdgeKind::EvidenceFromDecision
            } else {
                EdgeKind::EvidenceFromConcept
            });
            matched_ids.extend(
                graph
                    .nodes
                    .iter()
                    .filter(|node| matches!(node.kind, NodeKind::Concept | NodeKind::Decision))
                    .filter(|node| {
                        let name = node.name.to_lowercase();
                        let desc = node.description.as_deref().unwrap_or("").to_lowercase();
                        let detail = evidence.detail.to_lowercase();
                        detail.contains(&name) || (!desc.is_empty() && detail.contains(&desc))
                    })
                    .map(|node| node.id),
            );
        }

        matched_ids.sort_unstable();
        matched_ids.dedup();

        for matched_id in matched_ids {
            let edge = Edge {
                id: Uuid::new_v4(),
                from: decision_id,
                to: matched_id,
                kind: edge_kind.clone().unwrap_or(EdgeKind::RelatedTo),
                label: Some(format!("trace evidence: {}", evidence.source)),
            };
            store.save_edge(&edge).await.map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn get_default_project_path() -> Result<String, String> {
    Ok(default_project_path())
}

#[tauri::command]
pub async fn pick_project_directory(app: AppHandle) -> Result<Option<String>, String> {
    let selected = app.dialog().file().blocking_pick_folder();
    let normalized = selected
        .and_then(|path| path.into_path().ok())
        .map(normalize_project_path);
    if let Some(path) = normalized.as_deref() {
        remember_active_project(Path::new(path), None)?;
    }
    Ok(normalized)
}

#[tauri::command]
pub async fn import_github_project(url: String) -> Result<ImportedProjectData, String> {
    let (clone_url, owner, repo) = parse_github_project_url(&url)?;
    let clone_root = github_import_root();
    std::fs::create_dir_all(&clone_root).map_err(|e| e.to_string())?;

    let folder_name = format!("{owner}-{repo}");
    let (destination, reused_existing) = choose_clone_destination(&clone_root, &folder_name);

    if !reused_existing {
        let destination_text = destination.to_string_lossy().to_string();
        let output = std::process::Command::new("git")
            .args(["clone", &clone_url, &destination_text])
            .output()
            .map_err(|e| format!("执行 git clone 失败：{e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return Err(if !stderr.is_empty() {
                format!("GitHub 导入失败：{stderr}")
            } else if !stdout.is_empty() {
                format!("GitHub 导入失败：{stdout}")
            } else {
                "GitHub 导入失败：git clone 未成功完成。".to_string()
            });
        }
    }

    remember_active_project(&destination, Some(&folder_name))?;
    let normalized_path = normalize_project_path(&destination);
    let message = if reused_existing {
        format!("已复用本地仓库：{}", normalized_path)
    } else {
        format!("已克隆到本地：{}", normalized_path)
    };

    Ok(ImportedProjectData {
        url: clone_url,
        project_name: folder_name,
        project_path: normalized_path,
        reused_existing,
        message,
    })
}

#[tauri::command]
pub async fn get_model_settings(project_path: String) -> Result<ModelSettingsData, String> {
    let project_path = resolve_project_path(&project_path);
    let cfg = BsConfig::load(&project_path).unwrap_or_default();
    Ok(settings_from_config(&project_path, cfg))
}

#[tauri::command]
pub async fn save_model_settings(
    project_path: String,
    settings: ModelSettingsData,
) -> Result<String, String> {
    let project_path = resolve_project_path(&project_path);
    let config = config_from_settings(settings)?;
    let path = config
        .save_project(&project_path)
        .map_err(|e| e.to_string())?;
    Ok(format!("模型设置已保存到 {}", path.display()))
}

#[tauri::command]
pub async fn test_model_connection(
    project_path: String,
    provider: Option<String>,
) -> Result<String, String> {
    let project_path = resolve_project_path(&project_path);
    test_provider_connection_local(&project_path, provider.as_deref()).await
}

#[tauri::command]
pub async fn list_provider_models(
    provider: ProviderSettingsData,
) -> Result<ProviderModelsData, String> {
    let client = reqwest::Client::new();
    let models = match provider.protocol.as_str() {
        "anthropic" => fetch_models_from_anthropic(&client, &provider).await?,
        _ => fetch_models_from_openai_compatible(&client, &provider).await?,
    };
    Ok(ProviderModelsData { models })
}

#[tauri::command]
pub async fn index_project(project_path: String) -> Result<String, String> {
    let project_path = resolve_project_path(&project_path);
    let (files, lines, nodes, edges) = build_graph_local(&project_path).await?;
    if nodes == 0 {
        return Err(format!(
            "Indexed '{}' but found 0 graph nodes. Check that this is the repository root and that it contains supported code files before generating docs.",
            project_path.display()
        ));
    }
    Ok(format!(
        "{} files, {} lines, {} nodes, {} edges",
        files, lines, nodes, edges
    ))
}

#[tauri::command]
pub async fn ask(project_path: String, question: String) -> Result<String, String> {
    let project_path = resolve_project_path(&project_path);
    ask_local(&project_path, &question, None).await
}

#[tauri::command]
pub async fn explain_target(project_path: String, target: String) -> Result<String, String> {
    let project_path = resolve_project_path(&project_path);
    let cfg = BsConfig::load(&project_path).unwrap_or_default();
    let llm = require_llm(&cfg, None)?;
    let store = GraphStore::new(&graph_db_path(&project_path))
        .await
        .map_err(|e| e.to_string())?;
    let graph = store.load_graph().await.map_err(|e| e.to_string())?;
    let vector_index = VectorIndex::new(store.pool.clone())
        .await
        .map_err(|e| e.to_string())?;

    if std::path::Path::new(&target).exists() {
        let src = std::fs::read_to_string(&target).map_err(|e| e.to_string())?;
        let llm: std::sync::Arc<dyn LlmClient> = std::sync::Arc::from(llm);
        let trace = TraceAgent::new(llm.clone());
        let report = trace
            .explain_file(&project_path, &target, &src)
            .await
            .map_err(|e| e.to_string())?;
        let anchor_ids = graph
            .nodes_in_file(&target)
            .into_iter()
            .map(|node| node.id)
            .collect::<Vec<_>>();
        let _ = persist_trace_decision(
            &store,
            &vector_index,
            &*llm,
            &graph,
            &anchor_ids,
            &format!("Decision: {}", target),
            &report,
        )
        .await;
        return Ok(report.to_markdown(&format!("Trace Report: {}", target)));
    }

    let node = graph.find_node_by_name(&target).ok_or_else(|| {
        format!(
            "'{}' not found as file or indexed symbol. Run `loci index --path {}` first.",
            target,
            project_path.display()
        )
    })?;

    let neighbors = graph.neighbors(node.id);
    let prompt = format!(
        "Explain this symbol to a developer who is new to the codebase.\n\
         Cover: what it does, why it likely exists, and what related nodes matter.\n\
         Output Markdown.\n\n\
         Symbol: {} ({:?})\nFile: {}\nRelated: {}\nDescription: {}",
        node.name,
        node.kind,
        node.file_path.as_deref().unwrap_or("unknown"),
        neighbors
            .iter()
            .map(|(_, n)| n.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        node.description.as_deref().unwrap_or("")
    );

    let response = llm
        .chat(
            vec![Message {
                role: Role::User,
                content: prompt,
            }],
            None,
        )
        .await
        .map_err(|e| e.to_string())?;
    let answer = match response {
        loci_llm::LlmResponse::Text(text) => text,
        _ => String::new(),
    };

    if let Some(file_path) = node.file_path.as_deref() {
        if let Ok(src) = std::fs::read_to_string(file_path) {
            let llm: std::sync::Arc<dyn LlmClient> = std::sync::Arc::from(llm);
            let trace = TraceAgent::new(llm.clone());
            if let Ok(report) = trace.explain_file(&project_path, file_path, &src).await {
                let mut anchor_ids = vec![node.id];
                if let Some(file_node) = graph.find_file_node(file_path) {
                    anchor_ids.push(file_node.id);
                }
                let _ = persist_trace_decision(
                    &store,
                    &vector_index,
                    &*llm,
                    &graph,
                    &anchor_ids,
                    &format!("Decision: {}", target),
                    &report,
                )
                .await;
            }
        }
    }

    Ok(answer)
}

#[tauri::command]
pub async fn analyze_recent_diff(
    project_path: String,
    commit: Option<String>,
) -> Result<String, String> {
    let project_path = resolve_project_path(&project_path);
    let commit = commit.unwrap_or_else(|| "HEAD".to_string());
    let cfg = BsConfig::load(&project_path).unwrap_or_default();
    let llm = require_llm(&cfg, None)?;

    let diff_output = if commit == "HEAD" {
        std::process::Command::new("git")
            .args(["diff", "HEAD"])
            .current_dir(&project_path)
            .output()
            .map_err(|e| e.to_string())?
    } else {
        std::process::Command::new("git")
            .args(["diff", &format!("{}^", commit), &commit])
            .current_dir(&project_path)
            .output()
            .map_err(|e| e.to_string())?
    };

    let diff = String::from_utf8_lossy(&diff_output.stdout).to_string();
    if diff.trim().is_empty() {
        return Ok(format!("No changes found for '{}'.", commit));
    }

    let llm: std::sync::Arc<dyn LlmClient> = std::sync::Arc::from(llm);
    let trace = TraceAgent::new(llm.clone());
    let report = trace
        .analyze_diff(&commit, &diff)
        .await
        .map_err(|e| e.to_string())?;

    let store = GraphStore::new(&graph_db_path(&project_path))
        .await
        .map_err(|e| e.to_string())?;
    let graph = store.load_graph().await.map_err(|e| e.to_string())?;
    let vector_index = VectorIndex::new(store.pool.clone())
        .await
        .map_err(|e| e.to_string())?;
    let mut anchor_ids = graph
        .nodes
        .iter()
        .filter(|node| {
            node.kind == NodeKind::Commit && (node.name == commit || node.name.starts_with(&commit))
        })
        .map(|node| node.id)
        .collect::<Vec<_>>();
    let changed_files = extract_changed_files_from_diff(&diff);
    anchor_ids.extend(
        graph
            .nodes
            .iter()
            .filter(|node| {
                node.kind == NodeKind::File
                    && changed_files.iter().any(|changed| {
                        node.name == *changed || node.file_path.as_deref() == Some(changed.as_str())
                    })
            })
            .map(|node| node.id),
    );
    anchor_ids.sort_unstable();
    anchor_ids.dedup();

    let _ = persist_trace_decision(
        &store,
        &vector_index,
        &*llm,
        &graph,
        &anchor_ids,
        &format!("Decision: diff {}", commit),
        &report,
    )
    .await;

    Ok(report.to_markdown(&format!("Trace Report: diff {}", commit)))
}

#[tauri::command]
pub async fn get_graph(project_path: String) -> Result<GraphData, String> {
    let project_path = resolve_project_path(&project_path);
    let store = GraphStore::new(&graph_db_path(&project_path))
        .await
        .map_err(|e| e.to_string())?;
    let graph = store.load_graph().await.map_err(|e| e.to_string())?;

    Ok(GraphData {
        nodes: graph.nodes.iter().map(map_node).collect(),
        edges: graph.edges.iter().map(map_edge).collect(),
    })
}

#[tauri::command]
pub async fn get_trace(project_path: String, target: String) -> Result<TraceData, String> {
    let project_path = resolve_project_path(&project_path);
    let store = GraphStore::new(&graph_db_path(&project_path))
        .await
        .map_err(|e| e.to_string())?;
    let graph = store.load_graph().await.map_err(|e| e.to_string())?;
    let query = target.trim().to_lowercase();

    let anchor_ids: HashSet<Uuid> = graph
        .nodes
        .iter()
        .filter(|node| {
            if query.is_empty() {
                false
            } else {
                node.name.to_lowercase().contains(&query)
                    || node
                        .file_path
                        .as_ref()
                        .map(|path| path.to_lowercase().contains(&query))
                        .unwrap_or(false)
            }
        })
        .map(|node| node.id)
        .collect();

    let mut decision_ids = HashSet::new();
    let mut commit_ids = HashSet::new();
    let mut evidence = Vec::new();
    let mut related_ids = HashSet::new();

    if anchor_ids.is_empty() {
        for node in graph
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Decision)
            .rev()
            .take(8)
        {
            decision_ids.insert(node.id);
        }
    } else {
        let trace_scope = graph.expand_ids_with_neighbors(&anchor_ids, 2);
        for node in &graph.nodes {
            if trace_scope.contains(&node.id) {
                if node.kind == NodeKind::Decision {
                    decision_ids.insert(node.id);
                }
                if node.kind == NodeKind::Commit {
                    commit_ids.insert(node.id);
                }
                if !anchor_ids.contains(&node.id) {
                    related_ids.insert(node.id);
                }
            }
        }
    }

    for edge in &graph.edges {
        if is_evidence_edge(&edge.kind)
            && (decision_ids.contains(&edge.from) || decision_ids.contains(&edge.to))
        {
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
        if edge.kind == EdgeKind::ChangedIn
            && (decision_ids.contains(&edge.from) || decision_ids.contains(&edge.to))
        {
            if !decision_ids.contains(&edge.from) {
                commit_ids.insert(edge.from);
            }
            if !decision_ids.contains(&edge.to) {
                commit_ids.insert(edge.to);
            }
        }
    }

    Ok(TraceData {
        anchors: graph
            .nodes
            .iter()
            .filter(|node| anchor_ids.contains(&node.id))
            .map(map_node)
            .collect(),
        decisions: graph
            .nodes
            .iter()
            .filter(|node| decision_ids.contains(&node.id))
            .map(map_node)
            .collect(),
        commits: graph
            .nodes
            .iter()
            .filter(|node| commit_ids.contains(&node.id) && node.kind == NodeKind::Commit)
            .map(map_node)
            .collect(),
        evidence,
        related: graph
            .nodes
            .iter()
            .filter(|node| related_ids.contains(&node.id))
            .map(map_node)
            .collect(),
    })
}

#[tauri::command]
pub async fn get_doc(
    project_path: String,
    kind: String,
    provider: Option<String>,
) -> Result<DocData, String> {
    let project_path = resolve_project_path(&project_path);
    let cfg = BsConfig::load(&project_path).unwrap_or_default();
    let llm = require_llm(&cfg, provider.as_deref())?;
    let store = GraphStore::new(&graph_db_path(&project_path))
        .await
        .map_err(|e| e.to_string())?;
    let graph = store.load_graph().await.map_err(|e| e.to_string())?;
    if graph.nodes.is_empty() {
        return Err(format!(
            "No index found for '{}'. Re-run indexing for this exact project path and make sure the index result contains non-zero nodes.",
            project_path.display()
        ));
    }

    let prompt = build_doc_prompt(&graph, &kind);
    let response = llm
        .chat(
            vec![Message {
                role: Role::User,
                content: prompt,
            }],
            None,
        )
        .await
        .map_err(|e| e.to_string())?;
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
    let store = MemoryStore::new(&memory_db_path(&project_path))
        .await
        .map_err(|e| e.to_string())?;
    let memories = store
        .recall(None, None, None, 20)
        .await
        .map_err(|e| e.to_string())?;
    Ok(memories.into_iter().map(|memory| memory.content).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_github_url() {
        let (clone_url, owner, repo) =
            parse_github_project_url("https://github.com/openai/openai-cookbook").unwrap();
        assert_eq!(clone_url, "https://github.com/openai/openai-cookbook.git");
        assert_eq!(owner, "openai");
        assert_eq!(repo, "openai-cookbook");
    }

    #[test]
    fn parses_ssh_github_url() {
        let (clone_url, owner, repo) =
            parse_github_project_url("git@github.com:owner/repo.git").unwrap();
        assert_eq!(clone_url, "git@github.com:owner/repo.git");
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn chooses_suffix_when_plain_directory_exists() {
        let root = std::env::temp_dir().join(format!("loci-clone-test-{}", Uuid::new_v4()));
        let existing = root.join("demo-repo");
        std::fs::create_dir_all(&existing).unwrap();

        let (candidate, reused_existing) = choose_clone_destination(&root, "demo-repo");
        assert_eq!(candidate, root.join("demo-repo-2"));
        assert!(!reused_existing);

        std::fs::remove_dir_all(&root).unwrap();
    }
}
