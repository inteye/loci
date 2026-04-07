use std::path::PathBuf;
use clap::{Parser, Subcommand};
use anyhow::Result;
use rustyline::{DefaultEditor, error::ReadlineError};
use std::sync::Arc;
use loci_codebase::{CodebaseIndexer, GitHistory};
use loci_graph::{KnowledgeGraph, GraphStore, Node, Edge, NodeKind, EdgeKind, VectorIndex};
use loci_llm::{LlmClient, config::BsConfig};
use loci_agent::{TraceAgent, TraceReport};
use loci_memory::{MemoryStore, remember};
use loci_knowledge::{KnowledgeStore, ingest_file, ingest_url};
use loci_skills::SkillRegistry;
use loci_core::types::{Message, Role, MemoryScope};
use uuid::Uuid;
use chrono::Utc;
use serde::{Serialize, Deserialize};

#[derive(Parser)]
#[command(name = "loci", about = "loci — codebase understanding agent")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan and index a codebase
    Index {
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Only re-parse files changed since last index
        #[arg(long)]
        incremental: bool,
    },
    /// Generate embeddings + LLM descriptions for all graph nodes
    Embed {
        #[arg(short, long)]
        path: Option<PathBuf>,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Ask a question about the codebase
    Ask {
        /// Question (omit for interactive chat mode)
        question: Option<String>,
        #[arg(short, long)]
        path: Option<PathBuf>,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Explain a file or symbol in plain language
    Explain {
        /// File path or symbol name
        target: String,
        #[arg(short, long)]
        path: Option<PathBuf>,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Explain recent git changes
    Diff {
        /// Commit ref or range (default: HEAD uncommitted changes)
        #[arg(default_value = "HEAD")]
        commit: String,
        #[arg(short, long)]
        path: Option<PathBuf>,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Show the stored decision chain and evidence graph for a file or commit
    Trace {
        /// File path or commit hash prefix
        target: String,
        #[arg(short, long)]
        path: Option<PathBuf>,
    },
    /// Generate project documentation from the graph, decisions, and concepts
    Doc {
        /// Document kind: onboarding, module, handoff
        #[arg(default_value = "onboarding")]
        kind: String,
        #[arg(short, long)]
        path: Option<PathBuf>,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Run a lightweight real-project evaluation against the indexed codebase
    Eval {
        #[arg(short, long)]
        path: Option<PathBuf>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Run as background server (watches files + serves HTTP API)
    Serve {
        #[arg(short, long)]
        path: Option<PathBuf>,
        #[arg(long, default_value = "3000")]
        port: u16,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Run a built-in skill (code_review, commit_message, doc_generate, pr_description)
    Skill {
        /// Skill name (omit to list available skills)
        name: Option<String>,
        /// Input: file path, or '-' to read from stdin
        #[arg(short, long)]
        input: Option<String>,
        #[arg(short, long)]
        path: Option<PathBuf>,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Manage projects
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
    /// Show the knowledge graph
    Graph {
        #[arg(short, long)]
        path: Option<PathBuf>,
    },
    /// Show git history for a file
    History {
        file: String,
        #[arg(short, long)]
        path: Option<PathBuf>,
    },
    /// Manage memories
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
        #[arg(short, long)]
        path: Option<PathBuf>,
    },
    /// Manage knowledge base
    Knowledge {
        #[command(subcommand)]
        action: KnowledgeAction,
        #[arg(short, long)]
        path: Option<PathBuf>,
        #[arg(long)]
        provider: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProjectAction {
    /// Register current directory as a named project
    Add { name: String, #[arg(short, long)] path: Option<PathBuf> },
    /// List all registered projects
    List,
    /// Switch active project (sets default path for all commands)
    Use { name: String },
    /// Remove a project from the registry
    Remove { name: String },
}

#[derive(Subcommand)]
enum MemoryAction {
    /// List recent memories
    List,
    /// Clear all memories
    Clear,
}

#[derive(Subcommand)]
enum KnowledgeAction {
    /// Add a file to the knowledge base
    Add { source: String },
    /// List knowledge entries
    List,
    /// Search the knowledge base
    Search { query: String },
    /// Watch a directory and auto-ingest new/changed files
    Watch { dir: PathBuf },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match cli.command {
        Command::Index { path, incremental }                    => cmd_index(&resolve_command_path(path)?, incremental).await,
        Command::Embed { path, provider }                       => cmd_embed(&resolve_command_path(path)?, provider.as_deref()).await,
        Command::Ask { question, path, provider }               => cmd_ask(question.as_deref(), &resolve_command_path(path)?, provider.as_deref()).await,
        Command::Graph { path }                                 => cmd_graph(&resolve_command_path(path)?).await,
        Command::History { file, path }                         => cmd_history(&file, &resolve_command_path(path)?).await,
        Command::Explain { target, path, provider }             => cmd_explain(&target, &resolve_command_path(path)?, provider.as_deref()).await,
        Command::Diff { commit, path, provider }                => cmd_diff(&commit, &resolve_command_path(path)?, provider.as_deref()).await,
        Command::Trace { target, path }                         => cmd_trace(&target, &resolve_command_path(path)?).await,
        Command::Doc { kind, path, provider }                   => cmd_doc(&kind, &resolve_command_path(path)?, provider.as_deref()).await,
        Command::Eval { path, provider, output }                => cmd_eval(&resolve_command_path(path)?, provider.as_deref(), output.as_ref()).await,
        Command::Serve { path, port, provider }                 => cmd_serve(&resolve_command_path(path)?, port, provider.as_deref()).await,
        Command::Project { action }                             => cmd_project(action).await,
        Command::Skill { name, input, path, provider }          => cmd_skill(name.as_deref(), input.as_deref(), &resolve_command_path(path)?, provider.as_deref()).await,
        Command::Memory { action, path }                        => cmd_memory(action, &resolve_command_path(path)?).await,
        Command::Knowledge { action, path, provider }           => cmd_knowledge(action, &resolve_command_path(path)?, provider.as_deref()).await,
    }
}

// ── index ─────────────────────────────────────────────────────────────────────

async fn cmd_index(path: &PathBuf, incremental: bool) -> Result<()> {
    println!("Scanning {}...", path.display());

    let ts_file = bs_dir(path).join("last_index");
    let last_ts: i64 = if incremental {
        std::fs::read_to_string(&ts_file).ok()
            .and_then(|s| s.trim().parse().ok()).unwrap_or(0)
    } else { 0 };

    let mut index = CodebaseIndexer::index(path)?;
    println!("Found {} files, {} lines", index.summary.files.len(), index.summary.total_lines);
    for (lang, count) in &index.summary.language_breakdown {
        println!("  {}: {} files", lang, count);
    }

    if incremental && last_ts > 0 {
        let updated = loci_codebase::CodebaseIndexer::index_incremental(path, &mut index, last_ts)?;
        println!("Incremental: {} files updated", updated);
    }

    build_graph(path, &index).await?;
    std::fs::write(&ts_file, Utc::now().timestamp().to_string())?;
    println!("Run `loci embed` to generate embeddings + descriptions");
    Ok(())
}

async fn build_graph(path: &PathBuf, index: &loci_codebase::CodebaseIndex) -> Result<()> {
    let store = GraphStore::new(&graph_db_path(path)).await?;
    store.clear().await?;  // avoid duplicate nodes on re-index
    let mut graph = KnowledgeGraph::default();
    let mut commit_nodes: std::collections::HashMap<String, Uuid> = std::collections::HashMap::new();
    let parsed_by_path: std::collections::HashMap<String, &loci_codebase::ParsedFile> = index.parsed_files.iter()
        .map(|pf| (pf.path.clone(), pf))
        .collect();

    for file in index.summary.files.iter().filter(|file| file.language.is_code()) {
        let parsed = parsed_by_path.get(&file.path.to_string_lossy().to_string()).copied();
        let file_node = Node {
            id: Uuid::new_v4(), kind: NodeKind::File,
            name: file.relative_path.clone(), file_path: Some(file.path.to_string_lossy().to_string()),
            description: parsed.and_then(|pf| pf.doc_comment.clone()), raw_source: None, created_at: Utc::now(),
        };
        let file_id = graph.add_node(file_node.clone());
        store.save_node(&file_node).await?;

        let file_path = file.path.to_string_lossy().to_string();
        if let Ok(history) = GitHistory::file_history(path, &file_path, 3) {
            for commit in history.commits {
                let commit_id = if let Some(id) = commit_nodes.get(&commit.hash) {
                    *id
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
                    store.save_node(&node).await?;
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
                store.save_edge(&edge).await?;
            }
        }

        if let Some(pf) = parsed {
            for sym in &pf.symbols {
                let kind = match sym.kind {
                    loci_codebase::SymbolKind::Struct => NodeKind::Struct,
                    loci_codebase::SymbolKind::Enum   => NodeKind::Enum,
                    loci_codebase::SymbolKind::Trait  => NodeKind::Trait,
                    loci_codebase::SymbolKind::Module => NodeKind::Module,
                    _ => NodeKind::Function,
                };
                let sym_node = Node {
                    id: Uuid::new_v4(), kind,
                    name: sym.name.clone(), file_path: Some(file_path.clone()),
                    description: sym.doc_comment.clone(), raw_source: None, created_at: Utc::now(),
                };
                let sym_id = graph.add_node(sym_node.clone());
                store.save_node(&sym_node).await?;
                let edge = Edge { id: Uuid::new_v4(), from: file_id, to: sym_id, kind: EdgeKind::Contains, label: None };
                graph.add_edge(edge.clone());
                store.save_edge(&edge).await?;
            }

            // Add Calls edges between symbols in this file
            for (caller, callee) in &pf.calls {
                let from_id = graph.nodes.iter().find(|n| &n.name == caller && n.file_path.as_deref() == Some(file_path.as_str())).map(|n| n.id);
                let to_id   = graph.nodes.iter().find(|n| &n.name == callee && n.file_path.as_deref() == Some(file_path.as_str())).map(|n| n.id);
                if let (Some(from), Some(to)) = (from_id, to_id) {
                    if from != to {
                        let edge = Edge { id: Uuid::new_v4(), from, to, kind: EdgeKind::Calls, label: None };
                        graph.add_edge(edge.clone());
                        store.save_edge(&edge).await?;
                    }
                }
            }
        } else {
            let raw_source = format!("parse failed or unsupported language-specific extraction");
            store.update_node_description(file_id, &raw_source).await?;
            if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == file_id) {
                node.description = Some(raw_source);
            }
        }
    }

    println!("Graph: {} nodes, {} edges → {}", graph.nodes.len(), graph.edges.len(), graph_db_path(path));
    Ok(())
}

// ── embed ─────────────────────────────────────────────────────────────────────

async fn cmd_embed(path: &PathBuf, provider: Option<&str>) -> Result<()> {
    let cfg = BsConfig::load(path).unwrap_or_default();
    let llm = cfg.build_client(provider)?;
    let store = GraphStore::new(&graph_db_path(path)).await?;
    let vector_index = VectorIndex::new(store.pool.clone()).await?;
    let graph = store.load_graph().await?;

    if graph.nodes.is_empty() {
        println!("No index. Run `loci index` first."); return Ok(());
    }

    println!("Processing {} nodes...", graph.nodes.len());
    let mut done = 0usize;

    for node in &graph.nodes {
        let description = if node.description.is_none() && !matches!(node.kind, NodeKind::File) {
            let prompt = format!("One sentence: what does `{}` ({:?}) do in a Rust codebase?", node.name, node.kind);
            match llm.chat(vec![Message { role: Role::User, content: prompt }], None).await {
                Ok(loci_llm::LlmResponse::Text(t)) => {
                    store.update_node_description(node.id, t.trim()).await?;
                    Some(t.trim().to_string())
                }
                _ => None,
            }
        } else { node.description.clone() };

        let text = format!("{} {:?} {} {}", node.name, node.kind,
            description.as_deref().unwrap_or(""),
            node.file_path.as_deref().unwrap_or(""));

        if let Ok(vec) = llm.embed(&text).await {
            vector_index.upsert(node.id, &vec).await?;
            done += 1;
        }
    }
    println!("Embedded {}/{} nodes", done, graph.nodes.len());
    Ok(())
}

// ── ask ───────────────────────────────────────────────────────────────────────

async fn cmd_ask(question: Option<&str>, path: &PathBuf, provider: Option<&str>) -> Result<()> {
    let cfg = BsConfig::load(path).unwrap_or_default();
    let llm = require_llm(&cfg, provider)?;

    let store = GraphStore::new(&graph_db_path(path)).await?;
    let graph = store.load_graph().await?;

    if graph.nodes.is_empty() {
        eprintln!("No index found. Run: loci index {}", path.display());
        std::process::exit(1);
    }

    let mem_store = MemoryStore::new(&memory_db_path(path)).await?;
    let kb_store  = KnowledgeStore::new(&knowledge_db_path(path)).await?;
    let vector_index = VectorIndex::new(store.pool.clone()).await?;
    let has_embeddings = vector_index.count().await? > 0;

    // Single question mode
    if let Some(q) = question {
        let answer = do_ask(q, &*llm, &graph, &store, &vector_index, has_embeddings, &mem_store, &kb_store).await?;
        println!("{}", answer);
        return Ok(());
    }

    // Interactive chat mode
    println!("loci chat (type 'exit' or Ctrl+D to quit)\n");
    let mut rl = DefaultEditor::new()?;
    let mut session_history: Vec<Message> = Vec::new();

    loop {
        match rl.readline("you> ") {
            Ok(line) => {
                let q = line.trim().to_string();
                if q.is_empty() { continue; }
                if q == "exit" || q == "quit" { break; }
                rl.add_history_entry(&q)?;

                // Build context once per question
                let q_vec = llm.embed(&q).await.ok();
                let graph_ctx = build_graph_context(&q, &q_vec, &graph, &vector_index, has_embeddings).await;
                let mem_ctx   = build_memory_context(&q_vec, &mem_store).await;
                let kb_ctx    = build_kb_context(&q_vec, &kb_store).await;

                // System prompt (only first turn)
                if session_history.is_empty() {
                    session_history.push(Message {
                        role: Role::System,
                        content: format!(
                            "You are a codebase understanding assistant.\n\n## Knowledge Graph\n{}{}{}\n\nAnswer accurately.",
                            graph_ctx, mem_ctx, kb_ctx
                        ),
                    });
                }

                session_history.push(Message { role: Role::User, content: q.clone() });

                match llm.chat(session_history.clone(), None).await {
                    Ok(loci_llm::LlmResponse::Text(answer)) => {
                        println!("\nbs> {}\n", answer);
                        session_history.push(Message { role: Role::Assistant, content: answer.clone() });

                        persist_answer_artifacts(
                            &q,
                            &answer,
                            &*llm,
                            &graph,
                            &store,
                            &vector_index,
                            &mem_store,
                            &kb_store,
                        ).await;
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("Error: {}", e),
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(e) => { eprintln!("Error: {}", e); break; }
        }
    }
    Ok(())
}

async fn do_ask(
    question: &str,
    llm: &dyn LlmClient,
    graph: &KnowledgeGraph,
    graph_store: &GraphStore,
    vector_index: &VectorIndex,
    has_embeddings: bool,
    mem_store: &MemoryStore,
    kb_store: &KnowledgeStore,
) -> Result<String> {
    let q_vec = llm.embed(question).await.ok();
    let graph_ctx = build_graph_context(question, &q_vec, graph, vector_index, has_embeddings).await;
    let mem_ctx   = build_memory_context(&q_vec, mem_store).await;
    let kb_ctx    = build_kb_context(&q_vec, kb_store).await;

    let system = format!(
        "You are a codebase understanding assistant.\n\n## Knowledge Graph\n{}{}{}\n\nAnswer accurately.",
        graph_ctx, mem_ctx, kb_ctx
    );
    let response = llm.chat(vec![
        Message { role: Role::System, content: system },
        Message { role: Role::User,   content: question.to_string() },
    ], None).await?;

    let answer = match response { loci_llm::LlmResponse::Text(t) => t, _ => String::new() };

    persist_answer_artifacts(
        question,
        &answer,
        llm,
        graph,
        graph_store,
        vector_index,
        mem_store,
        kb_store,
    ).await;

    Ok(answer)
}

async fn persist_answer_artifacts(
    question: &str,
    answer: &str,
    llm: &dyn LlmClient,
    graph: &KnowledgeGraph,
    graph_store: &GraphStore,
    vector_index: &VectorIndex,
    mem_store: &MemoryStore,
    kb_store: &KnowledgeStore,
) {
    let mem_text = format!("Q: {}\nA: {}", question, &answer[..answer.len().min(500)]);
    let mem_vec = llm.embed(&mem_text).await.ok();
    let _ = remember(mem_store, &mem_text, MemoryScope::Session, None, mem_vec).await;

    auto_extract_knowledge(question, answer, llm, kb_store, graph, graph_store, vector_index).await;
}

async fn auto_extract_knowledge(
    question: &str,
    answer: &str,
    llm: &dyn LlmClient,
    _kb: &KnowledgeStore,
    graph: &KnowledgeGraph,
    graph_store: &GraphStore,
    vector_index: &VectorIndex,
) {
    let prompt = format!(
        "Does this Q&A contain a reusable technical insight, design decision, or explanation \
         worth saving to the project graph? If yes, extract it as a single concise paragraph. \
         If no, reply with exactly: SKIP\n\nQ: {}\nA: {}",
        question, &answer[..answer.len().min(800)]
    );
    if let Ok(loci_llm::LlmResponse::Text(t)) = llm.chat(
        vec![Message { role: Role::User, content: prompt }], None
    ).await {
        let t = t.trim().to_string();
        if t != "SKIP" && !t.is_empty() {
            let concept = Node {
                id: Uuid::new_v4(),
                kind: NodeKind::Concept,
                name: format!("Insight: {}", question.chars().take(80).collect::<String>()),
                file_path: None,
                description: Some(t.clone()),
                raw_source: Some(format!("Q: {}\nA: {}", question, answer)),
                created_at: Utc::now(),
            };

            if graph_store.save_node(&concept).await.is_ok() {
                if let Ok(vec) = llm.embed(&t).await {
                    let _ = vector_index.upsert(concept.id, &vec).await;

                    if let Ok(hits) = vector_index.search(&vec, 3).await {
                        for (node_id, _) in hits {
                            if node_id == concept.id { continue; }
                            if graph.nodes.iter().any(|n| n.id == node_id) {
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
}

async fn build_graph_context(
    question: &str,
    q_vec: &Option<Vec<f32>>,
    graph: &KnowledgeGraph,
    vi: &VectorIndex,
    has_emb: bool,
) -> String {
    if has_emb {
        if let Some(ref qv) = q_vec {
            if let Ok(hits) = vi.search(qv, 30).await {
                let trace_query = is_trace_question(question);
                let mut ranked_ids: Vec<Uuid> = if trace_query {
                    let mut preferred: Vec<Uuid> = hits.iter()
                        .filter_map(|(id, _)| graph.nodes.iter()
                            .find(|n| n.id == *id && matches!(n.kind, NodeKind::Decision | NodeKind::Commit))
                            .map(|n| n.id))
                        .collect();
                    let mut fallback: Vec<Uuid> = hits.iter()
                        .filter_map(|(id, _)| graph.nodes.iter()
                            .find(|n| n.id == *id && !matches!(n.kind, NodeKind::Decision | NodeKind::Commit))
                            .map(|n| n.id))
                        .collect();
                    preferred.append(&mut fallback);
                    preferred
                } else {
                    hits.into_iter().map(|(id, _)| id).collect()
                };

                ranked_ids.truncate(if trace_query { 18 } else { 20 });
                let mut ids: std::collections::HashSet<Uuid> = ranked_ids.into_iter().collect();
                for id in ids.clone() { for (_, n) in graph.neighbors(id) { ids.insert(n.id); } }
                let mut context = graph.to_context_str_filtered(&ids);
                if trace_query {
                    let decision_section = graph.nodes.iter()
                        .filter(|n| ids.contains(&n.id) && n.kind == NodeKind::Decision)
                        .map(|n| format!("- {}: {}", n.name, n.description.as_deref().unwrap_or("")))
                        .collect::<Vec<_>>();
                    if !decision_section.is_empty() {
                        context = format!("## Prior Decisions\n{}\n\n{}", decision_section.join("\n"), context);
                    }
                }
                return context;
            }
        }
    }
    graph.to_context_str(None)
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
    ].iter().any(|needle| q.contains(needle))
}

async fn build_memory_context(q_vec: &Option<Vec<f32>>, store: &MemoryStore) -> String {
    let mems = store.recall(q_vec.as_deref(), Some(MemoryScope::Session), None, 5).await.unwrap_or_default();
    if mems.is_empty() { return String::new(); }
    format!("\n## Past context\n{}", mems.iter().map(|m| format!("- {}", m.content)).collect::<Vec<_>>().join("\n"))
}

async fn build_kb_context(q_vec: &Option<Vec<f32>>, store: &KnowledgeStore) -> String {
    let items = store.search_external(q_vec.as_deref(), None, 3).await.unwrap_or_default();
    if items.is_empty() { return String::new(); }
    format!("\n## Knowledge base\n{}", items.iter().map(|k| format!("---\n{}", &k.content[..k.content.len().min(800)])).collect::<Vec<_>>().join("\n"))
}

// ── memory ────────────────────────────────────────────────────────────────────

async fn cmd_memory(action: MemoryAction, path: &PathBuf) -> Result<()> {
    let store = MemoryStore::new(&memory_db_path(path)).await?;
    match action {
        MemoryAction::List => {
            let mems = store.recall(None, None, None, 20).await?;
            if mems.is_empty() { println!("No memories yet."); return Ok(()); }
            for m in &mems {
                println!("[{}] {}", m.created_at.format("%m-%d %H:%M"), &m.content[..m.content.len().min(120)]);
            }
            println!("\n{} total", store.count().await?);
        }
        MemoryAction::Clear => {
            // Recreate empty DB
            let db_path = memory_db_path(path);
            std::fs::remove_file(&db_path).ok();
            MemoryStore::new(&db_path).await?;
            println!("Memories cleared.");
        }
    }
    Ok(())
}

// ── knowledge ─────────────────────────────────────────────────────────────────

async fn cmd_knowledge(action: KnowledgeAction, path: &PathBuf, provider: Option<&str>) -> Result<()> {
    let kb = KnowledgeStore::new(&knowledge_db_path(path)).await?;

    match action {
        KnowledgeAction::Add { source } => {
            let cfg = BsConfig::load(path).unwrap_or_default();
            let llm = cfg.build_client(provider).ok();
            let graph_store = GraphStore::new(&graph_db_path(path)).await.ok();
            let graph = if let Some(store) = &graph_store {
                store.load_graph().await.ok()
            } else {
                None
            };
            let vector_index = if let Some(store) = &graph_store {
                VectorIndex::new(store.pool.clone()).await.ok()
            } else {
                None
            };

            let content_for_embed;
            let k = if source.starts_with("http://") || source.starts_with("https://") {
                println!("Fetching {}...", source);
                let k = ingest_url(&kb, &source, None, None).await?;
                content_for_embed = k.content.clone();
                k
            } else {
                let file_path = std::path::Path::new(&source);
                println!("Ingesting {}...", source);
                let k = ingest_file(&kb, file_path, None, None).await?;
                content_for_embed = k.content.clone();
                k
            };

            // Generate embedding if LLM available
            if let Some(ref llm) = llm {
                let snippet = &content_for_embed[..content_for_embed.len().min(2000)];
                if let Ok(vec) = llm.embed(snippet).await {
                    let mut k2 = k.clone();
                    k2.embedding = Some(vec);
                    kb.save(&k2).await?;
                }
            }

            if let (Some(llm), Some(store), Some(graph), Some(vi)) = (llm.as_deref(), graph_store.as_ref(), graph.as_ref(), vector_index.as_ref()) {
                let _ = persist_external_knowledge_to_graph(store, vi, llm, graph, &k).await;
            }

            println!("Added: {} chars from {}", content_for_embed.len(), source);
        }

        KnowledgeAction::List => {
            let items = kb.list_external(20).await?;
            if items.is_empty() { println!("Knowledge base is empty."); return Ok(()); }
            for item in &items {
                let src = match &item.source {
                    loci_core::types::KnowledgeSource::File { path } => path.clone(),
                    loci_core::types::KnowledgeSource::Url { url } => url.clone(),
                    loci_core::types::KnowledgeSource::Conversation { .. } => "[conversation]".to_string(),
                    loci_core::types::KnowledgeSource::Auto => "[auto]".to_string(),
                };
                println!("[{}] {} ({} chars)", item.created_at.format("%m-%d"), src, item.content.len());
            }
            println!("\n{} total", kb.count().await?);
        }

        KnowledgeAction::Search { query } => {
            let cfg = BsConfig::load(path).unwrap_or_default();
            let q_vec = cfg.build_client(provider).ok()
                .and_then(|llm| tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(llm.embed(&query)).ok()
                }));
            let results = kb.search_external(q_vec.as_deref(), Some(&query), 5).await?;
            if results.is_empty() { println!("No results."); return Ok(()); }
            for (i, item) in results.iter().enumerate() {
                println!("\n[{}] {}", i+1, &item.content[..item.content.len().min(300)]);
            }
        }

        KnowledgeAction::Watch { dir } => {
            use notify::{Watcher, RecursiveMode, recommended_watcher, Event, EventKind};
            use std::sync::mpsc;

            println!("Watching {} for changes (Ctrl+C to stop)...", dir.display());
            let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
            let mut watcher = recommended_watcher(tx)?;
            watcher.watch(&dir, RecursiveMode::Recursive)?;

            let cfg = BsConfig::load(path).unwrap_or_default();

            for res in rx {
                if let Ok(event) = res {
                    if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                        for p in event.paths {
                            if p.is_file() {
                                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                                if matches!(ext, "md" | "txt" | "rs" | "py" | "ts" | "toml") {
                                    print!("  Ingesting {}... ", p.display());
                                    let llm = cfg.build_client(None).ok();
                                    let embedding = if let Some(ref llm) = llm {
                                        std::fs::read_to_string(&p).ok()
                                            .and_then(|s| tokio::task::block_in_place(|| {
                                                tokio::runtime::Handle::current()
                                                    .block_on(llm.embed(&s[..s.len().min(2000)])).ok()
                                            }))
                                    } else { None };
                                    match ingest_file(&kb, &p, embedding, None).await {
                                        Ok(k) => {
                                            if let Some(ref llm) = llm {
                                                if let Ok(store) = GraphStore::new(&graph_db_path(path)).await {
                                                    if let Ok(graph) = store.load_graph().await {
                                                        if let Ok(vi) = VectorIndex::new(store.pool.clone()).await {
                                                            let _ = persist_external_knowledge_to_graph(&store, &vi, &**llm, &graph, &k).await;
                                                        }
                                                    }
                                                }
                                            }
                                            println!("{} chars", k.content.len())
                                        }
                                        Err(e) => println!("error: {}", e),
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

// ── skill ─────────────────────────────────────────────────────────────────────

async fn cmd_skill(name: Option<&str>, input: Option<&str>, path: &PathBuf, provider: Option<&str>) -> Result<()> {
    let registry = SkillRegistry::default();

    // List skills if no name given
    let skill_name = match name {
        None => {
            println!("Available skills:");
            for (n, d) in registry.list() { println!("  {:20} {}", n, d); }
            return Ok(());
        }
        Some(n) => n,
    };

    let skill = registry.get(skill_name)
        .ok_or_else(|| anyhow::anyhow!("Unknown skill '{}'. Run `loci skill` to list.", skill_name))?;

    // Read input
    let content = match input {
        Some("-") => { use std::io::Read; let mut s = String::new(); std::io::stdin().read_to_string(&mut s)?; s }
        Some(file) => std::fs::read_to_string(file)?,
        None => {
            // For commit_message / pr_description, default to git diff
            if matches!(skill_name, "commit_message" | "pr_description") {
                let out = std::process::Command::new("git").args(["diff", "HEAD"]).current_dir(path).output()?;
                String::from_utf8_lossy(&out.stdout).to_string()
            } else {
                anyhow::bail!("Provide --input <file> or pipe content via stdin");
            }
        }
    };

    let cfg = BsConfig::load(path).unwrap_or_default();
    let llm = require_llm(&cfg, provider)?;
    let ctx = loci_tools::ToolContext { working_dir: Some(path.to_string_lossy().to_string()), ..Default::default() };

    let result = skill.run(&content, &*llm, &ctx).await?;
    println!("{}", result);
    Ok(())
}

// ── project ───────────────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct ProjectRegistry {
    projects: Vec<ProjectEntry>,
    active: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct ProjectEntry { name: String, path: String }

fn registry_path() -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/bs");
    std::fs::create_dir_all(&dir).ok();
    dir.join("projects.json")
}

fn load_registry() -> ProjectRegistry {
    std::fs::read_to_string(registry_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_registry(r: &ProjectRegistry) {
    if let Ok(s) = serde_json::to_string_pretty(r) {
        let _ = std::fs::write(registry_path(), s);
    }
}

async fn cmd_project(action: ProjectAction) -> Result<()> {
    let mut reg = load_registry();
    match action {
        ProjectAction::Add { name, path } => {
            let path = resolve_command_path(path)?;
            let abs = std::fs::canonicalize(&path)
                .unwrap_or(path.clone())
                .to_string_lossy().to_string();
            reg.projects.retain(|p| p.name != name);
            reg.projects.push(ProjectEntry { name: name.clone(), path: abs.clone() });
            save_registry(&reg);
            println!("Added project '{}' → {}", name, abs);
        }
        ProjectAction::List => {
            if reg.projects.is_empty() { println!("No projects registered."); return Ok(()); }
            for p in &reg.projects {
                let active = reg.active.as_deref() == Some(&p.name);
                println!("{} {} → {}", if active { "▶" } else { " " }, p.name, p.path);
            }
        }
        ProjectAction::Use { name } => {
            if !reg.projects.iter().any(|p| p.name == name) {
                anyhow::bail!("Project '{}' not found. Run `loci project add {}`", name, name);
            }
            reg.active = Some(name.clone());
            save_registry(&reg);
            println!("Active project: {}", name);
        }
        ProjectAction::Remove { name } => {
            reg.projects.retain(|p| p.name != name);
            if reg.active.as_deref() == Some(&name) { reg.active = None; }
            save_registry(&reg);
            println!("Removed project '{}'", name);
        }
    }
    Ok(())
}

// ── serve ─────────────────────────────────────────────────────────────────────

async fn cmd_serve(path: &PathBuf, port: u16, provider: Option<&str>) -> Result<()> {
    use axum::{Router, routing::{post, get}, Json, extract::State};
    use std::sync::Arc;
    #[derive(serde::Deserialize)]
    struct AskReq { question: String, provider: Option<String> }
    #[derive(serde::Serialize)]
    struct AskResp { answer: String }

    #[derive(Clone)]
    struct Srv { path: PathBuf, provider: Option<String> }

    let srv = Arc::new(Srv { path: path.clone(), provider: provider.map(String::from) });

    // File watcher — auto re-index on changes
    let watch_path = path.clone();
    tokio::spawn(async move {
        use notify::{Watcher, RecursiveMode, recommended_watcher, Event, EventKind};
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher = match recommended_watcher(tx) {
            Ok(w) => w, Err(_) => return,
        };
        let _ = watcher.watch(&watch_path, RecursiveMode::Recursive);
        let mut last_reindex = std::time::Instant::now();
        for res in rx {
            if let Ok(event) = res {
                if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                    // Debounce: only re-index if >5s since last
                    if last_reindex.elapsed().as_secs() > 5 {
                        last_reindex = std::time::Instant::now();
                        if let Ok(mut index) = loci_codebase::CodebaseIndexer::index(&watch_path) {
                            let ts = std::fs::read_to_string(bs_dir(&watch_path).join("last_index"))
                                .ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
                            let _ = loci_codebase::CodebaseIndexer::index_incremental(&watch_path, &mut index, ts);
                            // Rebuild graph in background (fire and forget)
                            let wp = watch_path.clone();
                            let idx = index;
                            tokio::spawn(async move {
                                let _ = build_graph_static(&wp, &idx).await;
                                eprintln!("[serve] index updated");
                            });
                        }
                    }
                }
            }
        }
    });

    // HTTP handlers
    async fn handle_ask(
        State(srv): State<Arc<Srv>>,
        Json(req): Json<AskReq>,
    ) -> Json<AskResp> {
        let provider = req.provider.as_deref().or(srv.provider.as_deref());
        let cfg = BsConfig::load(&srv.path).unwrap_or_default();
        let answer = match cfg.build_client(provider) {
            Err(_) => "LLM not configured".to_string(),
            Ok(llm) => {
                let store = GraphStore::new(&graph_db_path(&srv.path)).await.unwrap();
                let graph = store.load_graph().await.unwrap_or_default();
                let mem_store = MemoryStore::new(&memory_db_path(&srv.path)).await.unwrap();
                let kb_store  = KnowledgeStore::new(&knowledge_db_path(&srv.path)).await.unwrap();
                let vi = VectorIndex::new(store.pool.clone()).await.unwrap();
                let has_emb = vi.count().await.unwrap_or(0) > 0;
                do_ask(&req.question, &*llm, &graph, &store, &vi, has_emb, &mem_store, &kb_store)
                    .await.unwrap_or_else(|e| e.to_string())
            }
        };
        Json(AskResp { answer })
    }

    async fn handle_health() -> &'static str { "ok" }

    let app = Router::new()
        .route("/ask",    post(handle_ask))
        .route("/health", get(handle_health))
        .with_state(srv);

    let addr = format!("127.0.0.1:{}", port);
    println!("loci serve listening on http://{}", addr);
    println!("  POST /ask  {{\"question\": \"...\"}}");
    println!("  GET  /health");
    println!("  Watching {} for changes...", path.display());

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn build_graph_static(path: &PathBuf, index: &loci_codebase::CodebaseIndex) -> Result<()> {
    build_graph(path, index).await
}

async fn cmd_explain(target: &str, path: &PathBuf, provider: Option<&str>) -> Result<()> {
    let cfg = BsConfig::load(path).unwrap_or_default();
    let llm = require_llm(&cfg, provider)?;

    // Try to read as file first, then fall back to symbol lookup in graph
    if std::path::Path::new(target).exists() {
        let src = std::fs::read_to_string(target)?;
        let llm: Arc<dyn LlmClient> = Arc::from(llm);
        let trace = TraceAgent::new(llm.clone());
        let report = trace.explain_file(path, target, &src).await?;
        let store = GraphStore::new(&graph_db_path(path)).await?;
        let graph = store.load_graph().await?;
        let vi = VectorIndex::new(store.pool.clone()).await?;
        let anchors: Vec<Uuid> = graph.nodes.iter()
            .filter(|n| n.kind == NodeKind::File && n.file_path.as_deref() == Some(target))
            .map(|n| n.id)
            .collect();
        let _ = persist_trace_decision(
            &store,
            &vi,
            &*llm,
            &graph,
            &anchors,
            &format!("Decision: {}", target),
            &report,
        ).await;
        println!("{}", report.to_markdown(&format!("Trace Report: {}", target)));
        return Ok(());
    }

    // Symbol lookup from graph remains as a lightweight explore path.
    let store = GraphStore::new(&graph_db_path(path)).await?;
    let graph = store.load_graph().await?;
    let node = graph.find_node_by_name(target)
        .ok_or_else(|| anyhow::anyhow!("'{}' not found as file or indexed symbol. Run `loci index` first.", target))?;

    let neighbors = graph.neighbors(node.id);
    let prompt = format!(
        "Explain this symbol to a developer who is new to the codebase.\n\
         Cover: what it does, why it likely exists, and what related nodes matter.\n\
         Output Markdown.\n\n\
         Symbol: {} ({:?})\nFile: {}\nRelated: {}\nDescription: {}",
        node.name,
        node.kind,
        node.file_path.as_deref().unwrap_or("unknown"),
        neighbors.iter().map(|(_, n)| n.name.as_str()).collect::<Vec<_>>().join(", "),
        node.description.as_deref().unwrap_or("")
    );

    let response = llm.chat(vec![Message { role: Role::User, content: prompt }], None).await?;
    if let loci_llm::LlmResponse::Text(t) = response { println!("{}", t); }
    Ok(())
}

// ── diff ──────────────────────────────────────────────────────────────────────

async fn cmd_diff(commit: &str, path: &PathBuf, provider: Option<&str>) -> Result<()> {
    let cfg = BsConfig::load(path).unwrap_or_default();
    let llm = require_llm(&cfg, provider)?;

    // Get diff via git command
    let diff_output = if commit == "HEAD" {
        // Uncommitted changes
        std::process::Command::new("git")
            .args(["diff", "HEAD"])
            .current_dir(path)
            .output()?
    } else {
        // Specific commit or range
        std::process::Command::new("git")
            .args(["diff", &format!("{}^", commit), commit])
            .current_dir(path)
            .output()?
    };

    let diff = String::from_utf8_lossy(&diff_output.stdout);
    if diff.trim().is_empty() {
        println!("No changes found for '{}'.", commit);
        return Ok(());
    }

    let llm: Arc<dyn LlmClient> = Arc::from(llm);
    let trace = TraceAgent::new(llm.clone());
    let report = trace.analyze_diff(commit, &diff).await?;
    let store = GraphStore::new(&graph_db_path(path)).await?;
    let graph = store.load_graph().await?;
    let vi = VectorIndex::new(store.pool.clone()).await?;
    let anchors: Vec<Uuid> = graph.nodes.iter()
        .filter(|n| n.kind == NodeKind::Commit && (n.name == commit || n.name.starts_with(commit)))
        .map(|n| n.id)
        .collect();
    let _ = persist_trace_decision(
        &store,
        &vi,
        &*llm,
        &graph,
        &anchors,
        &format!("Decision: diff {}", commit),
        &report,
    ).await;
    println!("{}", report.to_markdown(&format!("Trace Report: diff {}", commit)));
    Ok(())
}

// ── graph / history ───────────────────────────────────────────────────────────

async fn cmd_graph(path: &PathBuf) -> Result<()> {
    let store = GraphStore::new(&graph_db_path(path)).await?;
    let graph = store.load_graph().await?;
    if graph.nodes.is_empty() { println!("No index. Run `loci index` first."); return Ok(()); }
    println!("{}", graph.to_context_str(None));
    Ok(())
}

async fn cmd_history(file: &str, path: &PathBuf) -> Result<()> {
    let history = GitHistory::file_history(path, file, 10)?;
    println!("Git history for {}:", file);
    for c in &history.commits {
        println!("  [{}] {} — {}", c.hash, c.message, c.author);
    }
    if !history.blame_summary.is_empty() {
        println!("\nBlame highlights:");
        for (hash, line) in &history.blame_summary {
            println!("  [{}] {}", hash, line);
        }
    }
    Ok(())
}

async fn cmd_trace(target: &str, path: &PathBuf) -> Result<()> {
    let store = GraphStore::new(&graph_db_path(path)).await?;
    let graph = store.load_graph().await?;
    if graph.nodes.is_empty() {
        println!("No index. Run `loci index` first.");
        return Ok(());
    }

    let anchor = if std::path::Path::new(target).exists() {
        graph.nodes.iter().find(|n| n.kind == NodeKind::File && n.file_path.as_deref() == Some(target))
    } else {
        graph.nodes.iter().find(|n|
            (n.kind == NodeKind::Commit && (n.name == target || n.name.starts_with(target))) ||
            n.name.eq_ignore_ascii_case(target)
        )
    };

    let Some(anchor) = anchor else {
        println!("No trace target found for '{}'. Run `loci index`, `loci explain`, or `loci diff` first.", target);
        return Ok(());
    };

    let mut ids: std::collections::HashSet<Uuid> = std::collections::HashSet::from([anchor.id]);
    for (_, node) in graph.neighbors(anchor.id) {
        ids.insert(node.id);
    }

    let decisions: Vec<&Node> = graph.nodes.iter()
        .filter(|n| ids.contains(&n.id) && n.kind == NodeKind::Decision)
        .collect();
    let commits: Vec<&Node> = graph.nodes.iter()
        .filter(|n| ids.contains(&n.id) && n.kind == NodeKind::Commit)
        .collect();
    let related: Vec<&Node> = graph.nodes.iter()
        .filter(|n| ids.contains(&n.id) && !matches!(n.kind, NodeKind::Decision | NodeKind::Commit))
        .collect();

    println!("# Trace: {}\n", target);
    println!("## Anchor");
    println!("- {:?}: {}", anchor.kind, anchor.name);

    if !decisions.is_empty() {
        println!("\n## Decisions");
        for decision in decisions {
            println!("- {}", decision.name);
            if let Some(desc) = &decision.description {
                println!("  {}", desc);
            }

            let evidence_edges: Vec<&Edge> = graph.edges.iter()
                .filter(|e| e.from == decision.id && is_evidence_edge(&e.kind))
                .collect();
            if !evidence_edges.is_empty() {
                println!("  Evidence:");
                for edge in evidence_edges {
                    if let Some(node) = graph.nodes.iter().find(|n| n.id == edge.to) {
                        println!(
                            "  - {} ({:?}) [{}]",
                            node.name,
                            node.kind,
                            edge_kind_label(&edge.kind, edge.label.as_deref().unwrap_or("evidence"))
                        );
                    }
                }
            }
        }
    }

    if !commits.is_empty() {
        println!("\n## Commits");
        for commit in commits {
            println!("- {}", commit.name);
            if let Some(desc) = &commit.description {
                println!("  {}", desc);
            }
        }
    }

    if !related.is_empty() {
        println!("\n## Related Nodes");
        for node in related {
            if node.id != anchor.id {
                println!("- {:?}: {}", node.kind, node.name);
            }
        }
    }

    Ok(())
}

async fn cmd_doc(kind: &str, path: &PathBuf, provider: Option<&str>) -> Result<()> {
    let cfg = BsConfig::load(path).unwrap_or_default();
    let llm = require_llm(&cfg, provider)?;
    let store = GraphStore::new(&graph_db_path(path)).await?;
    let graph = store.load_graph().await?;

    if graph.nodes.is_empty() {
        println!("No index. Run `loci index` first.");
        return Ok(());
    }

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

    let prompt = match kind {
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
    };

    let response = llm.chat(vec![Message { role: Role::User, content: prompt }], None).await?;
    if let loci_llm::LlmResponse::Text(t) = response {
        println!("{}", t);
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvalSample {
    category: String,
    prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct EvalScore {
    score: u8,
    rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvalResult {
    category: String,
    prompt: String,
    answer: String,
    score: EvalScore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvalReport {
    generated_at: String,
    project_path: String,
    average_score: f32,
    results: Vec<EvalResult>,
    drift_check: Vec<String>,
}

async fn cmd_eval(path: &PathBuf, provider: Option<&str>, output: Option<&PathBuf>) -> Result<()> {
    let cfg = BsConfig::load(path).unwrap_or_default();
    let llm = require_llm(&cfg, provider)?;
    let store = GraphStore::new(&graph_db_path(path)).await?;
    let graph = store.load_graph().await?;

    if graph.nodes.is_empty() {
        println!("No index. Run `loci index` first.");
        return Ok(());
    }

    let mem_store = MemoryStore::new(&memory_db_path(path)).await?;
    let kb_store = KnowledgeStore::new(&knowledge_db_path(path)).await?;
    let vector_index = VectorIndex::new(store.pool.clone()).await?;
    let has_embeddings = vector_index.count().await? > 0;

    let prompts = load_eval_samples(path)?;

    println!("# Evaluation Report\n");
    println!("- Project: {}", path.display());
    println!("- Questions: {}", prompts.len());
    println!("- Uses graph, decisions, concepts, memories, and knowledge base\n");

    let mut results = Vec::new();
    for sample in prompts {
        let answer = do_ask(
            &sample.prompt,
            &*llm,
            &graph,
            &store,
            &vector_index,
            has_embeddings,
            &mem_store,
            &kb_store,
        ).await?;

        let score = score_eval_answer(&*llm, &sample, &answer).await.unwrap_or_else(|_| EvalScore {
            score: 0,
            rationale: "Scoring failed.".to_string(),
        });

        println!("## {}\n", sample.category);
        println!("**Prompt:** {}\n", sample.prompt);
        println!("**Score:** {}/5\n", score.score);
        println!("{}\n", answer);

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
    let previous = load_previous_eval_report(path);

    println!("## Drift Check\n");
    println!("- This evaluation stays on the roadmap path: it validates codebase understanding quality on a real indexed project.");
    println!("- It does not add a new generic framework; it reuses the existing ask pipeline and current project graph.");
    if let Some(prev) = &previous {
        println!("- Previous average score: {:.2}/5", prev.average_score);
        println!("- Score delta: {:+.2}", average_score - prev.average_score);
    }

    let report = EvalReport {
        generated_at: Utc::now().to_rfc3339(),
        project_path: path.display().to_string(),
        average_score,
        results,
        drift_check: vec![
            "Validates codebase understanding quality on a real indexed project.".to_string(),
            "Reuses the existing ask pipeline and project graph instead of introducing a generic framework.".to_string(),
        ],
    };

    let out_path = output.cloned().unwrap_or_else(|| {
        bs_dir(path).join("eval").join(format!("report-{}.md", Utc::now().format("%Y%m%d-%H%M%S")))
    });
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out_path, render_eval_report(&report))?;
    let json_path = out_path.with_extension("json");
    std::fs::write(&json_path, serde_json::to_string_pretty(&report)?)?;
    println!("\nSaved report to {}", out_path.display());
    println!("Saved machine-readable report to {}", json_path.display());

    Ok(())
}

async fn score_eval_answer(llm: &dyn LlmClient, sample: &EvalSample, answer: &str) -> Result<EvalScore> {
    let prompt = format!(
        "Score this codebase-understanding answer on a 0-5 scale.\n\
         Judge accuracy, specificity, use of design decisions/concepts, and usefulness to a developer.\n\
         Respond with JSON only: {{\"score\": <0-5>, \"rationale\": \"...\"}}\n\n\
         Category: {}\nPrompt: {}\nAnswer:\n{}",
        sample.category,
        sample.prompt,
        &answer[..answer.len().min(4000)]
    );

    let response = llm.chat(vec![Message { role: Role::User, content: prompt }], None).await?;
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

fn render_eval_report(report: &EvalReport) -> String {
    let mut out = String::new();
    out.push_str("# Evaluation Report\n\n");
    out.push_str(&format!("- Generated: {}\n", report.generated_at));
    out.push_str(&format!("- Project: {}\n", report.project_path));
    out.push_str(&format!("- Average score: {:.2}/5\n\n", report.average_score));

    for result in &report.results {
        out.push_str(&format!("## {}\n\n", result.category));
        out.push_str(&format!("**Prompt:** {}\n\n", result.prompt));
        out.push_str(&format!("**Score:** {}/5\n\n", result.score.score));
        out.push_str(&format!("**Rationale:** {}\n\n", result.score.rationale));
        out.push_str(&result.answer);
        out.push_str("\n\n");
    }

    out.push_str("## Drift Check\n\n");
    for line in &report.drift_check {
        out.push_str(&format!("- {}\n", line));
    }
    out
}

fn load_eval_samples(project_path: &PathBuf) -> Result<Vec<EvalSample>> {
    let repo_path = project_path.join("docs/eval/samples.json");
    if repo_path.exists() {
        let text = std::fs::read_to_string(&repo_path)?;
        return Ok(serde_json::from_str(&text)?);
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

fn load_previous_eval_report(project_path: &PathBuf) -> Option<EvalReport> {
    let eval_dir = bs_dir(project_path).join("eval");
    let mut reports: Vec<std::path::PathBuf> = std::fs::read_dir(&eval_dir).ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    reports.sort();
    let latest = reports.pop()?;
    let text = std::fs::read_to_string(latest).ok()?;
    serde_json::from_str(&text).ok()
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn bs_dir(p: &PathBuf) -> PathBuf {
    let d = p.join(".bs"); std::fs::create_dir_all(&d).ok(); d
}

fn resolve_command_path(path: Option<PathBuf>) -> Result<PathBuf> {
    let raw = match path {
        Some(path) => path,
        None => active_project_path().unwrap_or(std::env::current_dir()?),
    };
    Ok(std::fs::canonicalize(&raw).unwrap_or(raw))
}

fn active_project_path() -> Option<PathBuf> {
    let reg = load_registry();
    let active = reg.active?;
    reg.projects
        .into_iter()
        .find(|p| p.name == active)
        .map(|p| PathBuf::from(p.path))
}

fn graph_db_path(p: &PathBuf)     -> String { bs_dir(p).join("graph.db").to_string_lossy().to_string() }
fn memory_db_path(p: &PathBuf)    -> String { bs_dir(p).join("memory.db").to_string_lossy().to_string() }
fn knowledge_db_path(p: &PathBuf) -> String { bs_dir(p).join("knowledge.db").to_string_lossy().to_string() }

async fn persist_trace_decision(
    store: &GraphStore,
    vector_index: &VectorIndex,
    llm: &dyn LlmClient,
    graph: &KnowledgeGraph,
    anchor_ids: &[Uuid],
    title: &str,
    report: &TraceReport,
) -> Result<Uuid> {
    let decision = Node {
        id: Uuid::new_v4(),
        kind: NodeKind::Decision,
        name: title.to_string(),
        file_path: None,
        description: Some(report.summary.clone()),
        raw_source: Some(serde_json::to_string_pretty(report)?),
        created_at: Utc::now(),
    };
    store.save_node(&decision).await?;

    if let Ok(vec) = llm.embed(&format!("{}\n{}", title, report.summary)).await {
        let _ = vector_index.upsert(decision.id, &vec).await;
    }

    for anchor_id in anchor_ids {
        if graph.nodes.iter().any(|n| n.id == *anchor_id) {
            let edge = Edge {
                id: Uuid::new_v4(),
                from: *anchor_id,
                to: decision.id,
                kind: EdgeKind::ExplainedBy,
                label: Some("trace report".to_string()),
            };
            store.save_edge(&edge).await?;
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
) -> Result<()> {
    for evidence in &report.evidence {
        let source = evidence.source.to_lowercase();

        let mut matched_ids: Vec<Uuid> = Vec::new();
        let mut edge_kind: Option<EdgeKind> = None;

        if source.contains("commit") || source.contains("blame") || source.contains("diff") {
            edge_kind = Some(EdgeKind::EvidenceFromCommit);
            for hash in extract_commit_hashes(&evidence.detail) {
                matched_ids.extend(
                    graph.nodes.iter()
                        .filter(|n| n.kind == NodeKind::Commit && (n.name == hash || n.name.starts_with(&hash)))
                        .map(|n| n.id)
                );
            }
        }

        if source.contains("code") || source.contains("file") {
            edge_kind.get_or_insert(EdgeKind::EvidenceFromFile);
            matched_ids.extend(anchor_ids.iter().copied());
        }

        if source.contains("decision") || source.contains("concept") {
            edge_kind.get_or_insert(if source.contains("decision") {
                EdgeKind::EvidenceFromDecision
            } else {
                EdgeKind::EvidenceFromConcept
            });
            matched_ids.extend(
                graph.nodes.iter()
                    .filter(|n| matches!(n.kind, NodeKind::Concept | NodeKind::Decision))
                    .filter(|n| {
                        let name = n.name.to_lowercase();
                        let desc = n.description.as_deref().unwrap_or("").to_lowercase();
                        let detail = evidence.detail.to_lowercase();
                        detail.contains(&name) || (!desc.is_empty() && detail.contains(&desc))
                    })
                    .map(|n| n.id)
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
            store.save_edge(&edge).await?;
        }
    }

    Ok(())
}

async fn persist_external_knowledge_to_graph(
    store: &GraphStore,
    vector_index: &VectorIndex,
    llm: &dyn LlmClient,
    graph: &KnowledgeGraph,
    knowledge: &loci_core::types::Knowledge,
) -> Result<Uuid> {
    let source_label = match &knowledge.source {
        loci_core::types::KnowledgeSource::File { path } => path.clone(),
        loci_core::types::KnowledgeSource::Url { url } => url.clone(),
        loci_core::types::KnowledgeSource::Conversation { .. } => "[conversation]".to_string(),
        loci_core::types::KnowledgeSource::Auto => "[auto]".to_string(),
    };

    let prompt = format!(
        "Extract one reusable project concept from this external material.\n\
         Reply with exactly one concise paragraph, or SKIP if nothing reusable stands out.\n\n\
         Source: {}\n\n{}",
        source_label,
        &knowledge.content[..knowledge.content.len().min(3000)]
    );

    let extracted = match llm.chat(vec![Message { role: Role::User, content: prompt }], None).await? {
        loci_llm::LlmResponse::Text(text) => text.trim().to_string(),
        _ => "SKIP".to_string(),
    };

    if extracted == "SKIP" || extracted.is_empty() {
        return Ok(Uuid::nil());
    }

    let node = Node {
        id: Uuid::new_v4(),
        kind: NodeKind::Concept,
        name: format!("External: {}", source_label.chars().take(80).collect::<String>()),
        file_path: None,
        description: Some(extracted.clone()),
        raw_source: Some(knowledge.content[..knowledge.content.len().min(3000)].to_string()),
        created_at: Utc::now(),
    };
    store.save_node(&node).await?;

    if let Ok(vec) = llm.embed(&extracted).await {
        let _ = vector_index.upsert(node.id, &vec).await;
    }

    match &knowledge.source {
        loci_core::types::KnowledgeSource::File { path } => {
            for file_node in graph.nodes.iter().filter(|n| n.kind == NodeKind::File && n.file_path.as_deref() == Some(path.as_str())) {
                let edge = Edge {
                    id: Uuid::new_v4(),
                    from: file_node.id,
                    to: node.id,
                    kind: EdgeKind::ExplainedBy,
                    label: Some("external material".to_string()),
                };
                store.save_edge(&edge).await?;
            }
        }
        loci_core::types::KnowledgeSource::Url { .. } => {}
        loci_core::types::KnowledgeSource::Conversation { .. } => {}
        loci_core::types::KnowledgeSource::Auto => {}
    }

    Ok(node.id)
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

fn is_evidence_edge(kind: &EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::RelatedTo
            | EdgeKind::EvidenceFromFile
            | EdgeKind::EvidenceFromCommit
            | EdgeKind::EvidenceFromConcept
            | EdgeKind::EvidenceFromDecision
    )
}

fn edge_kind_label(kind: &EdgeKind, fallback: &str) -> String {
    match kind {
        EdgeKind::EvidenceFromFile => "evidence:file".to_string(),
        EdgeKind::EvidenceFromCommit => "evidence:commit".to_string(),
        EdgeKind::EvidenceFromConcept => "evidence:concept".to_string(),
        EdgeKind::EvidenceFromDecision => "evidence:decision".to_string(),
        _ => fallback.to_string(),
    }
}

fn require_llm(cfg: &BsConfig, provider: Option<&str>) -> Result<Box<dyn LlmClient>> {
    cfg.build_client(provider).map_err(|_| {
        eprintln!("No LLM configured. Create .bs/config.toml:");
        eprintln!("  cp config.example.toml .bs/config.toml");
        anyhow::anyhow!("LLM not configured")
    })
}

#[cfg(test)]
mod tests {
    use super::{extract_commit_hashes, is_trace_question};

    #[test]
    fn extracts_commit_hashes_from_mixed_text() {
        let hashes = extract_commit_hashes("commit b841111 fixed it, follow-up 2676630 touched graph");
        assert_eq!(hashes, vec!["b841111".to_string(), "2676630".to_string()]);
    }

    #[test]
    fn detects_trace_question_in_english_and_chinese() {
        assert!(is_trace_question("why was this designed this way?"));
        assert!(is_trace_question("这个设计为什么这么做？"));
        assert!(!is_trace_question("这个模块有哪些公开函数？"));
    }
}
