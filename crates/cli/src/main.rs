use std::path::PathBuf;
use clap::{Parser, Subcommand};
use anyhow::Result;
use rustyline::{DefaultEditor, error::ReadlineError};
use loci_codebase::{CodebaseIndexer, GitHistory};
use loci_graph::{KnowledgeGraph, GraphStore, Node, Edge, NodeKind, EdgeKind, VectorIndex};
use loci_llm::{LlmClient, config::BsConfig};
use loci_memory::{MemoryStore, remember};
use loci_knowledge::{KnowledgeStore, ingest_file, ingest_url};
use loci_skills::SkillRegistry;
use loci_core::types::{Message, Role, MemoryScope};
use uuid::Uuid;
use chrono::Utc;

#[derive(Parser)]
#[command(name = "loci", about = "Sage — codebase understanding agent")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan and index a codebase
    Index {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Only re-parse files changed since last index
        #[arg(long)]
        incremental: bool,
    },
    /// Generate embeddings + LLM descriptions for all graph nodes
    Embed {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Ask a question about the codebase
    Ask {
        /// Question (omit for interactive chat mode)
        question: Option<String>,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Explain a file or symbol in plain language
    Explain {
        /// File path or symbol name
        target: String,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Explain recent git changes
    Diff {
        /// Commit ref or range (default: HEAD uncommitted changes)
        #[arg(default_value = "HEAD")]
        commit: String,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Run as background server (watches files + serves HTTP API)
    Serve {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
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
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
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
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Show git history for a file
    History {
        file: String,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Manage memories
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Manage knowledge base
    Knowledge {
        #[command(subcommand)]
        action: KnowledgeAction,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        provider: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProjectAction {
    /// Register current directory as a named project
    Add { name: String, #[arg(short, long, default_value = ".")] path: PathBuf },
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
        Command::Index { path, incremental }                    => cmd_index(&path, incremental).await,
        Command::Embed { path, provider }                        => cmd_embed(&path, provider.as_deref()).await,
        Command::Ask { question, path, provider }                => cmd_ask(question.as_deref(), &path, provider.as_deref()).await,
        Command::Graph { path }                          => cmd_graph(&path).await,
        Command::History { file, path }                  => cmd_history(&file, &path).await,
        Command::Explain { target, path, provider }      => cmd_explain(&target, &path, provider.as_deref()).await,
        Command::Diff { commit, path, provider }         => cmd_diff(&commit, &path, provider.as_deref()).await,
        Command::Serve { path, port, provider }          => cmd_serve(&path, port, provider.as_deref()).await,
        Command::Project { action }                      => cmd_project(action).await,
        Command::Skill { name, input, path, provider }   => cmd_skill(name.as_deref(), input.as_deref(), &path, provider.as_deref()).await,
        Command::Memory { action, path }                 => cmd_memory(action, &path).await,
        Command::Knowledge { action, path, provider }    => cmd_knowledge(action, &path, provider.as_deref()).await,
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

    for pf in &index.parsed_files {
        let file_node = Node {
            id: Uuid::new_v4(), kind: NodeKind::File,
            name: pf.path.clone(), file_path: Some(pf.path.clone()),
            description: pf.doc_comment.clone(), raw_source: None, created_at: Utc::now(),
        };
        let file_id = graph.add_node(file_node.clone());
        store.save_node(&file_node).await?;

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
                name: sym.name.clone(), file_path: Some(pf.path.clone()),
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
            let from_id = graph.nodes.iter().find(|n| &n.name == caller && n.file_path.as_deref() == Some(&pf.path)).map(|n| n.id);
            let to_id   = graph.nodes.iter().find(|n| &n.name == callee && n.file_path.as_deref() == Some(&pf.path)).map(|n| n.id);
            if let (Some(from), Some(to)) = (from_id, to_id) {
                if from != to {
                    let edge = Edge { id: Uuid::new_v4(), from, to, kind: EdgeKind::Calls, label: None };
                    graph.add_edge(edge.clone());
                    store.save_edge(&edge).await?;
                }
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
        let answer = do_ask(q, &*llm, &graph, &vector_index, has_embeddings, &mem_store, &kb_store).await?;
        println!("{}", answer);
        return Ok(());
    }

    // Interactive chat mode
    println!("Sage chat (type 'exit' or Ctrl+D to quit)\n");
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
                let graph_ctx = build_graph_context(&q_vec, &graph, &vector_index, has_embeddings).await;
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

                        // Save to memory
                        let mem_text = format!("Q: {}\nA: {}", q, &answer[..answer.len().min(500)]);
                        let mem_vec = llm.embed(&mem_text).await.ok();
                        let _ = remember(&mem_store, &mem_text, MemoryScope::Project, None, mem_vec).await;
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
    vector_index: &VectorIndex,
    has_embeddings: bool,
    mem_store: &MemoryStore,
    kb_store: &KnowledgeStore,
) -> Result<String> {
    let q_vec = llm.embed(question).await.ok();
    let graph_ctx = build_graph_context(&q_vec, graph, vector_index, has_embeddings).await;
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

    let mem_text = format!("Q: {}\nA: {}", question, &answer[..answer.len().min(500)]);
    let mem_vec = llm.embed(&mem_text).await.ok();
    let _ = remember(mem_store, &mem_text, MemoryScope::Project, None, mem_vec).await;

    // Auto-extract reusable knowledge from the answer
    auto_extract_knowledge(question, &answer, llm, kb_store).await;

    Ok(answer)
}

async fn auto_extract_knowledge(question: &str, answer: &str, llm: &dyn LlmClient, kb: &KnowledgeStore) {
    let prompt = format!(
        "Does this Q&A contain a reusable technical insight, design decision, or explanation \
         worth saving to a knowledge base? If yes, extract it as a single concise paragraph. \
         If no, reply with exactly: SKIP\n\nQ: {}\nA: {}",
        question, &answer[..answer.len().min(800)]
    );
    if let Ok(loci_llm::LlmResponse::Text(t)) = llm.chat(
        vec![Message { role: Role::User, content: prompt }], None
    ).await {
        let t = t.trim().to_string();
        if t != "SKIP" && !t.is_empty() {
            let k = loci_core::types::Knowledge {
                id: Uuid::new_v4(),
                source: loci_core::types::KnowledgeSource::Auto,
                content: t,
                embedding: None,
                project_id: None,
                tags: vec!["auto-extracted".to_string()],
                created_at: Utc::now(),
            };
            let _ = kb.save(&k).await;
        }
    }
}

async fn build_graph_context(q_vec: &Option<Vec<f32>>, graph: &KnowledgeGraph, vi: &VectorIndex, has_emb: bool) -> String {
    if has_emb {
        if let Some(ref qv) = q_vec {
            if let Ok(hits) = vi.search(qv, 20).await {
                let mut ids: std::collections::HashSet<Uuid> = hits.into_iter().map(|(id,_)| id).collect();
                for id in ids.clone() { for (_, n) in graph.neighbors(id) { ids.insert(n.id); } }
                return graph.to_context_str_filtered(&ids);
            }
        }
    }
    graph.to_context_str(None)
}

async fn build_memory_context(q_vec: &Option<Vec<f32>>, store: &MemoryStore) -> String {
    let mems = store.recall(q_vec.as_deref(), None, None, 5).await.unwrap_or_default();
    if mems.is_empty() { return String::new(); }
    format!("\n## Past context\n{}", mems.iter().map(|m| format!("- {}", m.content)).collect::<Vec<_>>().join("\n"))
}

async fn build_kb_context(q_vec: &Option<Vec<f32>>, store: &KnowledgeStore) -> String {
    let items = store.search(q_vec.as_deref(), None, 3).await.unwrap_or_default();
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
            if let Some(llm) = llm {
                let snippet = &content_for_embed[..content_for_embed.len().min(2000)];
                if let Ok(vec) = llm.embed(snippet).await {
                    let mut k2 = k.clone();
                    k2.embedding = Some(vec);
                    kb.save(&k2).await?;
                }
            }

            println!("Added: {} chars from {}", content_for_embed.len(), source);
        }

        KnowledgeAction::List => {
            let items = kb.list(20).await?;
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
            let results = kb.search(q_vec.as_deref(), Some(&query), 5).await?;
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
                                        Ok(k) => println!("{} chars", k.content.len()),
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
    use tokio::sync::RwLock;

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
                do_ask(&req.question, &*llm, &graph, &vi, has_emb, &mem_store, &kb_store)
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
    let (content, context_hint) = if std::path::Path::new(target).exists() {
        let src = std::fs::read_to_string(target)?;
        let hist = GitHistory::file_history(path, target, 5).ok();
        let hist_str = hist.map(|h| {
            h.commits.iter().map(|c| format!("  [{}] {} — {}", c.hash, c.message, c.author))
                .collect::<Vec<_>>().join("\n")
        }).unwrap_or_default();
        (src, format!("File: {}\nRecent git history:\n{}", target, hist_str))
    } else {
        // Symbol lookup from graph
        let store = GraphStore::new(&graph_db_path(path)).await?;
        let graph = store.load_graph().await?;
        match graph.find_node_by_name(target) {
            Some(node) => {
                let neighbors = graph.neighbors(node.id);
                let ctx = format!("Symbol: {} ({:?})\nFile: {}\nRelated: {}",
                    node.name, node.kind,
                    node.file_path.as_deref().unwrap_or("unknown"),
                    neighbors.iter().map(|(_, n)| n.name.as_str()).collect::<Vec<_>>().join(", ")
                );
                (node.description.clone().unwrap_or_else(|| node.name.clone()), ctx)
            }
            None => anyhow::bail!("'{}' not found as file or indexed symbol. Run `loci index` first.", target),
        }
    };

    let prompt = format!(
        "Explain the following code/symbol to a developer who is new to this codebase.\n\
         Be clear and concise. Cover: what it does, why it exists, key design decisions.\n\
         Output Markdown.\n\n\
         Context:\n{}\n\n\
         Code:\n```\n{}\n```",
        context_hint,
        &content[..content.len().min(6000)]
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

    // Truncate large diffs
    let diff_snippet = &diff[..diff.len().min(8000)];

    let prompt = format!(
        "Analyze this git diff and provide:\n\
         1. **Summary** — what changed in one paragraph\n\
         2. **Changed modules** — list affected components\n\
         3. **Impact** — potential side effects or things to watch out for\n\
         4. **Suggested commit message** — conventional commit format\n\n\
         Output Markdown.\n\n\
         ```diff\n{}\n```",
        diff_snippet
    );

    let response = llm.chat(vec![Message { role: Role::User, content: prompt }], None).await?;
    if let loci_llm::LlmResponse::Text(t) = response { println!("{}", t); }
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
    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn bs_dir(p: &PathBuf) -> PathBuf {
    let d = p.join(".bs"); std::fs::create_dir_all(&d).ok(); d
}
fn graph_db_path(p: &PathBuf)     -> String { bs_dir(p).join("graph.db").to_string_lossy().to_string() }
fn memory_db_path(p: &PathBuf)    -> String { bs_dir(p).join("memory.db").to_string_lossy().to_string() }
fn knowledge_db_path(p: &PathBuf) -> String { bs_dir(p).join("knowledge.db").to_string_lossy().to_string() }

fn require_llm(cfg: &BsConfig, provider: Option<&str>) -> Result<Box<dyn LlmClient>> {
    cfg.build_client(provider).map_err(|_| {
        eprintln!("No LLM configured. Create .bs/config.toml:");
        eprintln!("  cp config.example.toml .bs/config.toml");
        anyhow::anyhow!("LLM not configured")
    })
}