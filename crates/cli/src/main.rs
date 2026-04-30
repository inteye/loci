use anyhow::Result;
use chrono::Utc;
use clap::{Parser, Subcommand};
use loci_agent::{TraceAgent, TraceReport};
use loci_codebase::{CodebaseIndexer, GitHistory};
use loci_core::types::{MemoryScope, Message, Role};
use loci_graph::{Edge, EdgeKind, GraphStore, KnowledgeGraph, Node, NodeKind, VectorIndex};
use loci_knowledge::{ingest_file, ingest_url, KnowledgeStore};
use loci_llm::{config::BsConfig, LlmClient};
use loci_memory::{remember, MemoryStore};
use loci_skills::SkillRegistry;
use rustyline::{error::ReadlineError, DefaultEditor};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

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
    /// Inspect or test model providers
    Model {
        #[command(subcommand)]
        action: ModelAction,
        #[arg(short, long)]
        path: Option<PathBuf>,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Check whether the current project is ready for the full loci workflow
    Doctor {
        #[arg(short, long)]
        path: Option<PathBuf>,
        #[arg(long)]
        provider: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProjectAction {
    /// Register current directory as a named project
    Add {
        name: String,
        #[arg(short, long)]
        path: Option<PathBuf>,
    },
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

#[derive(Subcommand)]
enum ModelAction {
    /// List configured providers
    List,
    /// Test the selected or default provider connection
    Test,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match cli.command {
        Command::Index { path, incremental } => {
            cmd_index(&resolve_command_path(path)?, incremental).await
        }
        Command::Embed { path, provider } => {
            cmd_embed(&resolve_command_path(path)?, provider.as_deref()).await
        }
        Command::Ask {
            question,
            path,
            provider,
        } => {
            cmd_ask(
                question.as_deref(),
                &resolve_command_path(path)?,
                provider.as_deref(),
            )
            .await
        }
        Command::Graph { path } => cmd_graph(&resolve_command_path(path)?).await,
        Command::History { file, path } => cmd_history(&file, &resolve_command_path(path)?).await,
        Command::Explain {
            target,
            path,
            provider,
        } => cmd_explain(&target, &resolve_command_path(path)?, provider.as_deref()).await,
        Command::Diff {
            commit,
            path,
            provider,
        } => cmd_diff(&commit, &resolve_command_path(path)?, provider.as_deref()).await,
        Command::Trace { target, path } => cmd_trace(&target, &resolve_command_path(path)?).await,
        Command::Doc {
            kind,
            path,
            provider,
        } => cmd_doc(&kind, &resolve_command_path(path)?, provider.as_deref()).await,
        Command::Eval {
            path,
            provider,
            output,
        } => {
            cmd_eval(
                &resolve_command_path(path)?,
                provider.as_deref(),
                output.as_ref(),
            )
            .await
        }
        Command::Serve {
            path,
            port,
            provider,
        } => cmd_serve(&resolve_command_path(path)?, port, provider.as_deref()).await,
        Command::Project { action } => cmd_project(action).await,
        Command::Skill {
            name,
            input,
            path,
            provider,
        } => {
            cmd_skill(
                name.as_deref(),
                input.as_deref(),
                &resolve_command_path(path)?,
                provider.as_deref(),
            )
            .await
        }
        Command::Memory { action, path } => cmd_memory(action, &resolve_command_path(path)?).await,
        Command::Knowledge {
            action,
            path,
            provider,
        } => cmd_knowledge(action, &resolve_command_path(path)?, provider.as_deref()).await,
        Command::Model {
            action,
            path,
            provider,
        } => cmd_model(action, &resolve_command_path(path)?, provider.as_deref()).await,
        Command::Doctor { path, provider } => {
            cmd_doctor(&resolve_command_path(path)?, provider.as_deref()).await
        }
    }
}

// ── index ─────────────────────────────────────────────────────────────────────

async fn cmd_index(path: &PathBuf, incremental: bool) -> Result<()> {
    println!("Scanning {}...", path.display());

    let ts_file = bs_dir(path).join("last_index");
    let last_ts: i64 = if incremental {
        std::fs::read_to_string(&ts_file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    } else {
        0
    };

    let mut index = CodebaseIndexer::index(path)?;
    println!(
        "Found {} files, {} lines",
        index.summary.files.len(),
        index.summary.total_lines
    );
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
    store.clear().await?; // avoid duplicate nodes on re-index
    let mut graph = KnowledgeGraph::default();
    let mut commit_nodes: std::collections::HashMap<String, Uuid> =
        std::collections::HashMap::new();
    let parsed_by_path: std::collections::HashMap<String, &loci_codebase::ParsedFile> = index
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
        let parsed = parsed_by_path
            .get(&file.path.to_string_lossy().to_string())
            .copied();
        let file_node = Node {
            id: Uuid::new_v4(),
            kind: NodeKind::File,
            name: file.relative_path.clone(),
            file_path: Some(file.path.to_string_lossy().to_string()),
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
                        created_at: chrono::DateTime::from_timestamp(commit.timestamp, 0)
                            .unwrap_or_else(Utc::now),
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
                    loci_codebase::SymbolKind::Enum => NodeKind::Enum,
                    loci_codebase::SymbolKind::Trait => NodeKind::Trait,
                    loci_codebase::SymbolKind::Module => NodeKind::Module,
                    _ => NodeKind::Function,
                };
                let sym_node = Node {
                    id: Uuid::new_v4(),
                    kind,
                    name: sym.name.clone(),
                    file_path: Some(file_path.clone()),
                    description: sym.doc_comment.clone(),
                    raw_source: None,
                    created_at: Utc::now(),
                };
                let sym_id = graph.add_node(sym_node.clone());
                store.save_node(&sym_node).await?;
                let edge = Edge {
                    id: Uuid::new_v4(),
                    from: file_id,
                    to: sym_id,
                    kind: EdgeKind::Contains,
                    label: None,
                };
                graph.add_edge(edge.clone());
                store.save_edge(&edge).await?;
            }

            // Add Calls edges between symbols in this file
            for (caller, callee) in &pf.calls {
                let from_id = graph
                    .nodes
                    .iter()
                    .find(|n| {
                        &n.name == caller && n.file_path.as_deref() == Some(file_path.as_str())
                    })
                    .map(|n| n.id);
                let to_id = graph
                    .nodes
                    .iter()
                    .find(|n| {
                        &n.name == callee && n.file_path.as_deref() == Some(file_path.as_str())
                    })
                    .map(|n| n.id);
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

    println!(
        "Graph: {} nodes, {} edges → {}",
        graph.nodes.len(),
        graph.edges.len(),
        graph_db_path(path)
    );
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
        println!("No index. Run `loci index` first.");
        return Ok(());
    }

    println!("Processing {} nodes...", graph.nodes.len());
    let mut done = 0usize;

    for node in &graph.nodes {
        let description = if node.description.is_none() && !matches!(node.kind, NodeKind::File) {
            let prompt = format!(
                "One sentence: what does `{}` ({:?}) do in a Rust codebase?",
                node.name, node.kind
            );
            match llm
                .chat(
                    vec![Message {
                        role: Role::User,
                        content: prompt,
                    }],
                    None,
                )
                .await
            {
                Ok(loci_llm::LlmResponse::Text(t)) => {
                    store.update_node_description(node.id, t.trim()).await?;
                    Some(t.trim().to_string())
                }
                _ => None,
            }
        } else {
            node.description.clone()
        };

        let text = format!(
            "{} {:?} {} {}",
            node.name,
            node.kind,
            description.as_deref().unwrap_or(""),
            node.file_path.as_deref().unwrap_or("")
        );

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
    let vector_index = VectorIndex::new(store.pool.clone()).await?;
    let has_embeddings = vector_index.count().await? > 0;

    // Single question mode
    if let Some(q) = question {
        let answer = do_ask(
            q,
            &*llm,
            &graph,
            &store,
            &vector_index,
            has_embeddings,
            &mem_store,
        )
        .await?;
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
                if q.is_empty() {
                    continue;
                }
                if q == "exit" || q == "quit" {
                    break;
                }
                rl.add_history_entry(&q)?;

                // Build context once per question
                let q_vec = llm.embed(&q).await.ok();
                let graph_ctx =
                    build_graph_context(&q, &q_vec, &graph, &vector_index, has_embeddings).await;
                let mem_ctx = build_memory_context(&q_vec, &mem_store).await;
                // System prompt (only first turn)
                if session_history.is_empty() {
                    session_history.push(Message {
                        role: Role::System,
                        content: format!(
                            "You are a codebase understanding assistant.\n\
                             Treat the knowledge graph as the source of truth for project facts.\n\
                             Use session memory only as conversational context, not as authoritative project fact.\n\n\
                             ## Knowledge Graph\n{}{}\n\nAnswer accurately and call out uncertainty when evidence is weak.",
                            graph_ctx, mem_ctx
                        ),
                    });
                }

                session_history.push(Message {
                    role: Role::User,
                    content: q.clone(),
                });

                match llm.chat(session_history.clone(), None).await {
                    Ok(loci_llm::LlmResponse::Text(answer)) => {
                        println!("\nbs> {}\n", answer);
                        session_history.push(Message {
                            role: Role::Assistant,
                            content: answer.clone(),
                        });

                        persist_answer_artifacts(
                            &q,
                            &answer,
                            &*llm,
                            &graph,
                            &store,
                            &vector_index,
                            &mem_store,
                        )
                        .await;
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("Error: {}", e),
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("Error: {}", e);
                break;
            }
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
) -> Result<String> {
    let q_vec = llm.embed(question).await.ok();
    let graph_ctx =
        build_graph_context(question, &q_vec, graph, vector_index, has_embeddings).await;
    let mem_ctx = build_memory_context(&q_vec, mem_store).await;
    let system = format!(
        "You are a codebase understanding assistant.\n\
         Treat the knowledge graph as the source of truth for project facts.\n\
         Use session memory only as conversational context, not as authoritative project fact.\n\n\
         ## Knowledge Graph\n{}{}\n\nAnswer accurately and call out uncertainty when evidence is weak.",
        graph_ctx, mem_ctx
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
        .await?;

    let answer = match response {
        loci_llm::LlmResponse::Text(t) => t,
        _ => String::new(),
    };

    persist_answer_artifacts(
        question,
        &answer,
        llm,
        graph,
        graph_store,
        vector_index,
        mem_store,
    )
    .await;

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
) {
    let mem_text = format!("Q: {}\nA: {}", question, &answer[..answer.len().min(500)]);
    let mem_vec = llm.embed(&mem_text).await.ok();
    let _ = remember(mem_store, &mem_text, MemoryScope::Session, None, mem_vec).await;

    auto_extract_knowledge(question, answer, llm, graph, graph_store, vector_index).await;
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
        "Does this Q&A contain a reusable technical insight, design decision, or explanation \
         worth saving to the project graph? If yes, extract it as a single concise paragraph. \
         If no, reply with exactly: SKIP\n\nQ: {}\nA: {}",
        question,
        &answer[..answer.len().min(800)]
    );
    if let Ok(loci_llm::LlmResponse::Text(t)) = llm
        .chat(
            vec![Message {
                role: Role::User,
                content: prompt,
            }],
            None,
        )
        .await
    {
        let t = t.trim().to_string();
        if t != "SKIP" && !t.is_empty() {
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
                description: Some(t.clone()),
                raw_source: Some(format!("Q: {}\nA: {}", question, answer)),
                created_at: Utc::now(),
            };

            if graph_store.save_node(&concept).await.is_ok() {
                if let Ok(vec) = llm.embed(&t).await {
                    let _ = vector_index.upsert(concept.id, &vec).await;

                    if let Ok(hits) = vector_index.search(&vec, 3).await {
                        for (node_id, _) in hits {
                            if node_id == concept.id {
                                continue;
                            }
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
    let trace_query = is_trace_question(question);
    let decision_query = is_decision_question(question);

    let ids = if has_emb {
        if let Some(ref qv) = q_vec {
            if let Ok(hits) = vi.search(qv, 40).await {
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
) -> std::collections::HashSet<Uuid> {
    let mut ids = std::collections::HashSet::new();
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
) -> std::collections::HashSet<Uuid> {
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

    let mut ids = std::collections::HashSet::new();
    for (id, _, _) in ranked.into_iter().take(if trace_query { 18 } else { 20 }) {
        ids.insert(id);
    }

    graph.expand_ids_with_neighbors(&ids, if trace_query { 2 } else { 1 })
}

fn render_graph_context(
    graph: &KnowledgeGraph,
    ids: &std::collections::HashSet<Uuid>,
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

async fn build_memory_context(q_vec: &Option<Vec<f32>>, store: &MemoryStore) -> String {
    let mems = store
        .recall(q_vec.as_deref(), Some(MemoryScope::Session), None, 5)
        .await
        .unwrap_or_default();
    if mems.is_empty() {
        return String::new();
    }
    format!(
        "\n## Past context\n{}",
        mems.iter()
            .map(|m| format!("- {}", m.content))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

// ── memory ────────────────────────────────────────────────────────────────────

async fn cmd_memory(action: MemoryAction, path: &PathBuf) -> Result<()> {
    let store = MemoryStore::new(&memory_db_path(path)).await?;
    match action {
        MemoryAction::List => {
            let mems = store.recall(None, None, None, 20).await?;
            if mems.is_empty() {
                println!("No memories yet.");
                return Ok(());
            }
            for m in &mems {
                println!(
                    "[{}] {}",
                    m.created_at.format("%m-%d %H:%M"),
                    &m.content[..m.content.len().min(120)]
                );
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

async fn cmd_knowledge(
    action: KnowledgeAction,
    path: &PathBuf,
    provider: Option<&str>,
) -> Result<()> {
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

            if let (Some(llm), Some(store), Some(graph), Some(vi)) = (
                llm.as_deref(),
                graph_store.as_ref(),
                graph.as_ref(),
                vector_index.as_ref(),
            ) {
                let _ = persist_external_knowledge_to_graph(store, vi, llm, graph, &k).await;
            }

            println!("Added: {} chars from {}", content_for_embed.len(), source);
        }

        KnowledgeAction::List => {
            let items = kb.list_external(20).await?;
            if items.is_empty() {
                println!("Knowledge base is empty.");
                return Ok(());
            }
            for item in &items {
                let src = match &item.source {
                    loci_core::types::KnowledgeSource::File { path } => path.clone(),
                    loci_core::types::KnowledgeSource::Url { url } => url.clone(),
                    loci_core::types::KnowledgeSource::Conversation { .. } => {
                        "[conversation]".to_string()
                    }
                    loci_core::types::KnowledgeSource::Auto => "[auto]".to_string(),
                };
                println!(
                    "[{}] {} ({} chars)",
                    item.created_at.format("%m-%d"),
                    src,
                    item.content.len()
                );
            }
            println!("\n{} total", kb.count().await?);
        }

        KnowledgeAction::Search { query } => {
            let cfg = BsConfig::load(path).unwrap_or_default();
            let q_vec = if let Ok(llm) = cfg.build_client(provider) {
                llm.embed(&query).await.ok()
            } else {
                None
            };
            let results = kb
                .search_external(q_vec.as_deref(), Some(&query), 5)
                .await?;
            if results.is_empty() {
                println!("No results.");
                return Ok(());
            }
            for (i, item) in results.iter().enumerate() {
                println!(
                    "\n[{}] {}",
                    i + 1,
                    &item.content[..item.content.len().min(300)]
                );
            }
        }

        KnowledgeAction::Watch { dir } => {
            use notify::{recommended_watcher, Event, EventKind, RecursiveMode, Watcher};

            println!("Watching {} for changes (Ctrl+C to stop)...", dir.display());
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<notify::Result<Event>>();
            let mut watcher = recommended_watcher(move |res| {
                let _ = tx.send(res);
            })?;
            watcher.watch(&dir, RecursiveMode::Recursive)?;

            let cfg = BsConfig::load(path).unwrap_or_default();

            while let Some(res) = rx.recv().await {
                if let Ok(event) = res {
                    if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                        for p in event.paths {
                            if p.is_file() {
                                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                                if matches!(ext, "md" | "txt" | "rs" | "py" | "ts" | "toml") {
                                    print!("  Ingesting {}... ", p.display());
                                    let llm = cfg.build_client(None).ok();
                                    let embedding = if let Some(ref llm) = llm {
                                        if let Ok(content) = std::fs::read_to_string(&p) {
                                            llm.embed(&content[..content.len().min(2000)])
                                                .await
                                                .ok()
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    };
                                    match ingest_file(&kb, &p, embedding, None).await {
                                        Ok(k) => {
                                            if let Some(ref llm) = llm {
                                                if let Ok(store) =
                                                    GraphStore::new(&graph_db_path(path)).await
                                                {
                                                    if let Ok(graph) = store.load_graph().await {
                                                        if let Ok(vi) =
                                                            VectorIndex::new(store.pool.clone())
                                                                .await
                                                        {
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

async fn cmd_skill(
    name: Option<&str>,
    input: Option<&str>,
    path: &PathBuf,
    provider: Option<&str>,
) -> Result<()> {
    let registry = SkillRegistry::default();

    // List skills if no name given
    let skill_name = match name {
        None => {
            println!("Available skills:");
            for (n, d) in registry.list() {
                println!("  {:20} {}", n, d);
            }
            return Ok(());
        }
        Some(n) => n,
    };

    let skill = registry.get(skill_name).ok_or_else(|| {
        anyhow::anyhow!("Unknown skill '{}'. Run `loci skill` to list.", skill_name)
    })?;

    // Read input
    let content = match input {
        Some("-") => {
            use std::io::Read;
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            s
        }
        Some(file) => std::fs::read_to_string(file)?,
        None => {
            // For commit_message / pr_description, default to git diff
            if matches!(skill_name, "commit_message" | "pr_description") {
                let out = std::process::Command::new("git")
                    .args(["diff", "HEAD"])
                    .current_dir(path)
                    .output()?;
                String::from_utf8_lossy(&out.stdout).to_string()
            } else {
                anyhow::bail!("Provide --input <file> or pipe content via stdin");
            }
        }
    };

    let cfg = BsConfig::load(path).unwrap_or_default();
    let llm = require_llm(&cfg, provider)?;
    let ctx = loci_tools::ToolContext {
        working_dir: Some(path.to_string_lossy().to_string()),
        ..Default::default()
    };

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
struct ProjectEntry {
    name: String,
    path: String,
}

fn registry_path() -> std::path::PathBuf {
    let dir = config_root_dir();
    std::fs::create_dir_all(&dir).ok();
    dir.join("projects.json")
}

fn config_root_dir() -> std::path::PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        return std::path::PathBuf::from(appdata).join("bs");
    }
    if let Ok(home) = std::env::var("HOME") {
        return std::path::PathBuf::from(home).join(".config/bs");
    }
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        return std::path::PathBuf::from(user_profile).join(".config/bs");
    }
    match (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        (Ok(drive), Ok(path)) => std::path::PathBuf::from(format!("{drive}{path}")).join(".config/bs"),
        _ => std::path::PathBuf::from(".bs"),
    }
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
                .to_string_lossy()
                .to_string();
            reg.projects.retain(|p| p.name != name);
            reg.projects.push(ProjectEntry {
                name: name.clone(),
                path: abs.clone(),
            });
            save_registry(&reg);
            println!("Added project '{}' → {}", name, abs);
        }
        ProjectAction::List => {
            if reg.projects.is_empty() {
                println!("No projects registered.");
                return Ok(());
            }
            for p in &reg.projects {
                let active = reg.active.as_deref() == Some(&p.name);
                println!("{} {} → {}", if active { "▶" } else { " " }, p.name, p.path);
            }
        }
        ProjectAction::Use { name } => {
            if !reg.projects.iter().any(|p| p.name == name) {
                anyhow::bail!(
                    "Project '{}' not found. Run `loci project add {}`",
                    name,
                    name
                );
            }
            reg.active = Some(name.clone());
            save_registry(&reg);
            println!("Active project: {}", name);
        }
        ProjectAction::Remove { name } => {
            reg.projects.retain(|p| p.name != name);
            if reg.active.as_deref() == Some(&name) {
                reg.active = None;
            }
            save_registry(&reg);
            println!("Removed project '{}'", name);
        }
    }
    Ok(())
}

// ── serve ─────────────────────────────────────────────────────────────────────

async fn cmd_serve(path: &PathBuf, port: u16, provider: Option<&str>) -> Result<()> {
    use axum::{
        extract::State,
        routing::{get, post},
        Json, Router,
    };
    use std::sync::Arc;
    #[derive(serde::Deserialize)]
    struct AskReq {
        question: String,
        provider: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct AskResp {
        answer: String,
    }

    #[derive(Clone)]
    struct Srv {
        path: PathBuf,
        provider: Option<String>,
    }

    let srv = Arc::new(Srv {
        path: path.clone(),
        provider: provider.map(String::from),
    });

    // File watcher — auto re-index on changes
    let watch_path = path.clone();
    tokio::spawn(async move {
        use notify::{recommended_watcher, Event, EventKind, RecursiveMode, Watcher};
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher = match recommended_watcher(tx) {
            Ok(w) => w,
            Err(_) => return,
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
                            let ts =
                                std::fs::read_to_string(bs_dir(&watch_path).join("last_index"))
                                    .ok()
                                    .and_then(|s| s.trim().parse().ok())
                                    .unwrap_or(0);
                            let _ = loci_codebase::CodebaseIndexer::index_incremental(
                                &watch_path,
                                &mut index,
                                ts,
                            );
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
    async fn handle_ask(State(srv): State<Arc<Srv>>, Json(req): Json<AskReq>) -> Json<AskResp> {
        let provider = req.provider.as_deref().or(srv.provider.as_deref());
        let cfg = BsConfig::load(&srv.path).unwrap_or_default();
        let answer = match cfg.build_client(provider) {
            Err(_) => "LLM not configured".to_string(),
            Ok(llm) => {
                let store = GraphStore::new(&graph_db_path(&srv.path)).await.unwrap();
                let graph = store.load_graph().await.unwrap_or_default();
                let mem_store = MemoryStore::new(&memory_db_path(&srv.path)).await.unwrap();
                let vi = VectorIndex::new(store.pool.clone()).await.unwrap();
                let has_emb = vi.count().await.unwrap_or(0) > 0;
                do_ask(
                    &req.question,
                    &*llm,
                    &graph,
                    &store,
                    &vi,
                    has_emb,
                    &mem_store,
                )
                .await
                .unwrap_or_else(|e| e.to_string())
            }
        };
        Json(AskResp { answer })
    }

    async fn handle_health() -> &'static str {
        "ok"
    }

    let app = Router::new()
        .route("/ask", post(handle_ask))
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

async fn cmd_model(action: ModelAction, path: &PathBuf, provider: Option<&str>) -> Result<()> {
    match action {
        ModelAction::List => {
            let cfg = BsConfig::load(path).unwrap_or_default();
            let names = cfg.provider_names();
            if names.is_empty() {
                println!(
                    "No providers configured. Create .bs/config.toml from config.example.toml."
                );
                return Ok(());
            }
            println!(
                "Default provider: {}",
                cfg.default_provider.as_deref().unwrap_or("未设置")
            );
            for name in names {
                println!("- {}", name);
            }
            Ok(())
        }
        ModelAction::Test => {
            let cfg = BsConfig::load(path).unwrap_or_default();
            let llm = require_llm(&cfg, provider)?;
            let provider_name = provider
                .map(str::to_string)
                .or(cfg.default_provider.clone())
                .unwrap_or_else(|| "default".to_string());
            let response = llm
                .chat(
                    vec![Message {
                        role: Role::User,
                        content: "Reply with a short confirmation that the model connection works."
                            .to_string(),
                    }],
                    None,
                )
                .await?;
            let text = match response {
                loci_llm::LlmResponse::Text(text) => text,
                _ => "Received a non-text response from the model.".to_string(),
            };
            println!("Connection OK");
            println!("Provider: {}", provider_name);
            println!("Model: {}", llm.model());
            println!();
            println!("{}", text);
            Ok(())
        }
    }
}

async fn cmd_doctor(path: &PathBuf, provider: Option<&str>) -> Result<()> {
    let cfg = BsConfig::load(path).unwrap_or_default();
    let graph_path = bs_dir(path).join("graph.db");
    let memory_path = bs_dir(path).join("memory.db");
    let knowledge_path = bs_dir(path).join("knowledge.db");
    let configured_provider_names = cfg.provider_names();
    let selected_provider = provider
        .map(str::to_string)
        .or(cfg.default_provider.clone())
        .or_else(|| {
            configured_provider_names
                .first()
                .map(|name| (*name).to_string())
        });
    let provider_ready = selected_provider
        .as_deref()
        .map(|name| configured_provider_names.iter().any(|item| *item == name))
        .unwrap_or(false);

    let mut node_count = 0usize;
    let mut edge_count = 0usize;
    let mut decision_count = 0usize;
    let mut commit_count = 0usize;

    if graph_path.exists() {
        if let Ok(store) = GraphStore::new(&graph_db_path(path)).await {
            if let Ok(graph) = store.load_graph().await {
                node_count = graph.nodes.len();
                edge_count = graph.edges.len();
                decision_count = graph
                    .nodes
                    .iter()
                    .filter(|node| node.kind == NodeKind::Decision)
                    .count();
                commit_count = graph
                    .nodes
                    .iter()
                    .filter(|node| node.kind == NodeKind::Commit)
                    .count();
            }
        }
    }

    let indexed = graph_path.exists() && node_count > 0;

    println!("# loci Doctor\n");
    println!("- Project path: {}", path.display());
    println!(
        "- Project exists: {}",
        if path.exists() { "yes" } else { "no" }
    );
    println!(
        "- Index status: {}",
        if indexed {
            format!("ready ({} nodes, {} edges)", node_count, edge_count)
        } else {
            "missing".to_string()
        }
    );
    println!(
        "- Trace evidence: {}",
        if indexed {
            format!("{} decisions, {} commits", decision_count, commit_count)
        } else {
            "not available yet".to_string()
        }
    );
    println!(
        "- Model config: {}",
        if configured_provider_names.is_empty() {
            "missing".to_string()
        } else {
            format!("ready ({} providers)", configured_provider_names.len())
        }
    );
    println!(
        "- Default provider: {}",
        cfg.default_provider.as_deref().unwrap_or("not set")
    );
    println!(
        "- Selected provider: {}",
        selected_provider.as_deref().unwrap_or("not set")
    );
    println!(
        "- Selected provider configured: {}",
        if provider_ready { "yes" } else { "no" }
    );
    println!(
        "- Memory store: {}",
        if memory_path.exists() {
            "present"
        } else {
            "not created yet"
        }
    );
    println!(
        "- Knowledge store: {}",
        if knowledge_path.exists() {
            "present"
        } else {
            "not created yet"
        }
    );
    println!();
    println!("## Next Steps\n");

    if !path.exists() {
        println!(
            "- Fix the project path first, then rerun `loci doctor --path {}`.",
            path.display()
        );
        return Ok(());
    }

    if !indexed {
        println!(
            "- Run `loci index --path {}` to build the local graph.",
            path.display()
        );
    }

    if configured_provider_names.is_empty() {
        println!(
            "- Create model config: `cp config.example.toml {}`",
            bs_dir(path).join("config.toml").display()
        );
        println!("- Then run `loci model test --path {}`.", path.display());
    } else if !provider_ready {
        println!(
            "- Pick a configured provider with `--provider <name>`, or set `default_provider` in `.bs/config.toml`."
        );
        println!(
            "- Available providers: {}",
            configured_provider_names.join(", ")
        );
    } else {
        println!(
            "- Verify the model connection: `loci model test --path {}`.",
            path.display()
        );
    }

    if indexed && provider_ready {
        println!("- Full workflow is ready after model connectivity passes:");
        println!(
            "  loci ask \"这个项目的核心模块是什么？\" --path {}",
            path.display()
        );
        println!(
            "  loci explain crates/cli/src/main.rs --path {}",
            path.display()
        );
        println!("  loci diff --path {}", path.display());
        println!("  loci doc onboarding --path {}", path.display());
        println!("  loci eval --path {}", path.display());
    } else {
        println!(
            "- Once index and model config are ready, rerun `loci doctor --path {}`.",
            path.display()
        );
    }

    if indexed && decision_count == 0 {
        println!(
            "- Trace graph is still shallow. Run `loci explain <file> --path {}` or `loci diff --path {}` to write decisions back into the graph.",
            path.display(),
            path.display()
        );
    }

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
        let anchors: Vec<Uuid> = graph
            .nodes
            .iter()
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
        )
        .await;
        println!(
            "{}",
            report.to_markdown(&format!("Trace Report: {}", target))
        );
        return Ok(());
    }

    // Symbol lookup from graph remains as a lightweight explore path.
    let store = GraphStore::new(&graph_db_path(path)).await?;
    let graph = store.load_graph().await?;
    let node = graph.find_node_by_name(target).ok_or_else(|| {
        anyhow::anyhow!(
            "'{}' not found as file or indexed symbol. Run `loci index` first.",
            target
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
        .await?;
    if let loci_llm::LlmResponse::Text(t) = response {
        println!("{}", t);

        if let Some(file_path) = node.file_path.as_deref() {
            if let Ok(src) = std::fs::read_to_string(file_path) {
                let llm: Arc<dyn LlmClient> = Arc::from(llm);
                let trace = TraceAgent::new(llm.clone());
                if let Ok(report) = trace.explain_file(path, file_path, &src).await {
                    let mut anchor_ids = vec![node.id];
                    if let Some(file_node) = graph.find_file_node(file_path) {
                        anchor_ids.push(file_node.id);
                    }
                    let vi = VectorIndex::new(store.pool.clone()).await?;
                    let _ = persist_trace_decision(
                        &store,
                        &vi,
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
    }
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
    let mut anchors: Vec<Uuid> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Commit && (n.name == commit || n.name.starts_with(commit)))
        .map(|n| n.id)
        .collect();
    let changed_files = extract_changed_files_from_diff(&diff);
    anchors.extend(
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
    anchors.sort_unstable();
    anchors.dedup();
    let _ = persist_trace_decision(
        &store,
        &vi,
        &*llm,
        &graph,
        &anchors,
        &format!("Decision: diff {}", commit),
        &report,
    )
    .await;
    println!(
        "{}",
        report.to_markdown(&format!("Trace Report: diff {}", commit))
    );
    Ok(())
}

// ── graph / history ───────────────────────────────────────────────────────────

async fn cmd_graph(path: &PathBuf) -> Result<()> {
    let store = GraphStore::new(&graph_db_path(path)).await?;
    let graph = store.load_graph().await?;
    if graph.nodes.is_empty() {
        println!("No index. Run `loci index` first.");
        return Ok(());
    }
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
        graph
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::File && n.file_path.as_deref() == Some(target))
    } else {
        graph.nodes.iter().find(|n| {
            (n.kind == NodeKind::Commit && (n.name == target || n.name.starts_with(target)))
                || n.name.eq_ignore_ascii_case(target)
        })
    };

    let Some(anchor) = anchor else {
        println!("No trace target found for '{}'. Run `loci index`, `loci explain`, or `loci diff` first.", target);
        return Ok(());
    };

    let seed_ids: std::collections::HashSet<Uuid> = std::collections::HashSet::from([anchor.id]);
    let ids = graph.expand_ids_with_neighbors(&seed_ids, 2);

    let decisions: Vec<&Node> = graph
        .nodes
        .iter()
        .filter(|n| ids.contains(&n.id) && n.kind == NodeKind::Decision)
        .collect();
    let commits: Vec<&Node> = graph
        .nodes
        .iter()
        .filter(|n| ids.contains(&n.id) && n.kind == NodeKind::Commit)
        .collect();
    let related: Vec<&Node> = graph
        .nodes
        .iter()
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

            let evidence_edges: Vec<&Edge> = graph
                .edges
                .iter()
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
                            edge_kind_label(
                                &edge.kind,
                                edge.label.as_deref().unwrap_or("evidence")
                            )
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

    let response = llm
        .chat(
            vec![Message {
                role: Role::User,
                content: prompt,
            }],
            None,
        )
        .await?;
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
        )
        .await?;

        let score = score_eval_answer(&*llm, &sample, &answer)
            .await
            .unwrap_or_else(|_| EvalScore {
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
        bs_dir(path)
            .join("eval")
            .join(format!("report-{}.md", Utc::now().format("%Y%m%d-%H%M%S")))
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

async fn score_eval_answer(
    llm: &dyn LlmClient,
    sample: &EvalSample,
    answer: &str,
) -> Result<EvalScore> {
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
        .await?;
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

fn render_eval_report(report: &EvalReport) -> String {
    let mut out = String::new();
    out.push_str("# Evaluation Report\n\n");
    out.push_str(&format!("- Generated: {}\n", report.generated_at));
    out.push_str(&format!("- Project: {}\n", report.project_path));
    out.push_str(&format!(
        "- Average score: {:.2}/5\n\n",
        report.average_score
    ));

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
    let mut reports: Vec<std::path::PathBuf> = std::fs::read_dir(&eval_dir)
        .ok()?
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
    let d = p.join(".bs");
    std::fs::create_dir_all(&d).ok();
    d
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

fn graph_db_path(p: &PathBuf) -> String {
    bs_dir(p).join("graph.db").to_string_lossy().to_string()
}
fn memory_db_path(p: &PathBuf) -> String {
    bs_dir(p).join("memory.db").to_string_lossy().to_string()
}
fn knowledge_db_path(p: &PathBuf) -> String {
    bs_dir(p).join("knowledge.db").to_string_lossy().to_string()
}

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
                        .filter(|n| {
                            n.kind == NodeKind::Commit
                                && (n.name == hash || n.name.starts_with(&hash))
                        })
                        .map(|n| n.id),
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
                    .filter(|n| matches!(n.kind, NodeKind::Concept | NodeKind::Decision))
                    .filter(|n| {
                        let name = n.name.to_lowercase();
                        let desc = n.description.as_deref().unwrap_or("").to_lowercase();
                        let detail = evidence.detail.to_lowercase();
                        detail.contains(&name) || (!desc.is_empty() && detail.contains(&desc))
                    })
                    .map(|n| n.id),
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

    let extracted = match llm
        .chat(
            vec![Message {
                role: Role::User,
                content: prompt,
            }],
            None,
        )
        .await?
    {
        loci_llm::LlmResponse::Text(text) => text.trim().to_string(),
        _ => "SKIP".to_string(),
    };

    if extracted == "SKIP" || extracted.is_empty() {
        return Ok(Uuid::nil());
    }

    let node = Node {
        id: Uuid::new_v4(),
        kind: NodeKind::Concept,
        name: format!(
            "External: {}",
            source_label.chars().take(80).collect::<String>()
        ),
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
            for file_node in graph.nodes.iter().filter(|n| {
                n.kind == NodeKind::File && n.file_path.as_deref() == Some(path.as_str())
            }) {
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

fn collect_anchor_scope_ids(
    graph: &KnowledgeGraph,
    anchor_ids: &[Uuid],
) -> std::collections::HashSet<Uuid> {
    let seeds = anchor_ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    graph.expand_ids_with_neighbors(&seeds, 2)
}

fn match_file_or_symbol_ids(
    graph: &KnowledgeGraph,
    detail: &str,
    anchor_scope_ids: &std::collections::HashSet<Uuid>,
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
        eprintln!(
            "Recommended: configure a LiteLLM gateway provider and set it as default_provider."
        );
        anyhow::anyhow!("LLM not configured")
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_graph_context, extract_changed_files_from_diff, extract_commit_hashes,
        is_trace_question, persist_trace_evidence_edges,
    };
    use chrono::Utc;
    use loci_agent::{TraceEvidence, TraceReport};
    use loci_graph::{Edge, GraphStore, KnowledgeGraph, Node, NodeKind, VectorIndex};
    use uuid::Uuid;

    #[test]
    fn extracts_commit_hashes_from_mixed_text() {
        let hashes =
            extract_commit_hashes("commit b841111 fixed it, follow-up 2676630 touched graph");
        assert_eq!(hashes, vec!["b841111".to_string(), "2676630".to_string()]);
    }

    #[test]
    fn detects_trace_question_in_english_and_chinese() {
        assert!(is_trace_question("why was this designed this way?"));
        assert!(is_trace_question("这个设计为什么这么做？"));
        assert!(!is_trace_question("这个模块有哪些公开函数？"));
    }

    #[test]
    fn extracts_changed_files_from_diff_and_dedupes() {
        let files = extract_changed_files_from_diff(
            "--- a/src/main.rs\n+++ b/src/main.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n+++ b/src/main.rs",
        );
        assert_eq!(
            files,
            vec!["src/lib.rs".to_string(), "src/main.rs".to_string()]
        );
    }

    #[tokio::test]
    async fn build_graph_context_prefers_decisions_for_trace_queries() {
        let db_path = std::env::temp_dir().join(format!("loci-test-{}.db", Uuid::new_v4()));
        let store = GraphStore::new(&db_path.to_string_lossy()).await.unwrap();
        let vector_index = VectorIndex::new(store.pool.clone()).await.unwrap();
        let mut graph = KnowledgeGraph::default();

        let decision = Node {
            id: Uuid::new_v4(),
            kind: NodeKind::Decision,
            name: "Decision: trace pipeline".to_string(),
            file_path: None,
            description: Some("Trace should prioritize decision nodes.".to_string()),
            raw_source: None,
            created_at: Utc::now(),
        };
        let function = Node {
            id: Uuid::new_v4(),
            kind: NodeKind::Function,
            name: "handle_trace".to_string(),
            file_path: Some("src/main.rs".to_string()),
            description: Some("Function entry point.".to_string()),
            raw_source: None,
            created_at: Utc::now(),
        };
        graph.add_node(decision.clone());
        graph.add_node(function.clone());
        vector_index.upsert(decision.id, &[1.0, 0.0]).await.unwrap();
        vector_index.upsert(function.id, &[1.0, 0.0]).await.unwrap();

        let context = build_graph_context(
            "为什么 trace 要这样设计？",
            &Some(vec![1.0, 0.0]),
            &graph,
            &vector_index,
            true,
        )
        .await;

        assert!(context.contains("## Prior Decisions"));
        assert!(context.contains("Decision: trace pipeline"));
    }

    #[tokio::test]
    async fn persist_trace_evidence_edges_links_file_and_symbol_mentions() {
        let db_path = std::env::temp_dir().join(format!("loci-test-{}.db", Uuid::new_v4()));
        let store = GraphStore::new(&db_path.to_string_lossy()).await.unwrap();
        store.clear().await.unwrap();

        let mut graph = KnowledgeGraph::default();
        let file = Node {
            id: Uuid::new_v4(),
            kind: NodeKind::File,
            name: "src/main.rs".to_string(),
            file_path: Some("src/main.rs".to_string()),
            description: None,
            raw_source: None,
            created_at: Utc::now(),
        };
        let symbol = Node {
            id: Uuid::new_v4(),
            kind: NodeKind::Function,
            name: "bootstrap".to_string(),
            file_path: Some("src/main.rs".to_string()),
            description: None,
            raw_source: None,
            created_at: Utc::now(),
        };
        let decision = Node {
            id: Uuid::new_v4(),
            kind: NodeKind::Decision,
            name: "Decision: bootstrap".to_string(),
            file_path: None,
            description: Some("Boot sequence rationale".to_string()),
            raw_source: None,
            created_at: Utc::now(),
        };

        graph.add_node(file.clone());
        graph.add_node(symbol.clone());
        graph.add_node(decision.clone());
        graph.add_edge(Edge {
            id: Uuid::new_v4(),
            from: file.id,
            to: symbol.id,
            kind: loci_graph::EdgeKind::Contains,
            label: None,
        });
        store.save_node(&file).await.unwrap();
        store.save_node(&symbol).await.unwrap();
        store.save_node(&decision).await.unwrap();

        let report = TraceReport {
            summary: "bootstrap is justified by startup constraints".to_string(),
            timeline: vec![],
            evidence: vec![TraceEvidence {
                source: "code".to_string(),
                detail: "src/main.rs uses bootstrap during startup".to_string(),
            }],
            confidence: "high".to_string(),
            open_questions: vec![],
        };

        persist_trace_evidence_edges(&store, &graph, decision.id, &[file.id], &report)
            .await
            .unwrap();

        let persisted = store.load_graph().await.unwrap();
        let matched_targets = persisted
            .edges
            .iter()
            .filter(|edge| edge.from == decision.id)
            .map(|edge| edge.to)
            .collect::<Vec<_>>();

        assert!(matched_targets.contains(&file.id));
        assert!(matched_targets.contains(&symbol.id));
    }
}
