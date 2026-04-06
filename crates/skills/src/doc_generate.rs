use async_trait::async_trait;
use anyhow::Result;
use loci_core::types::{Message, Role};
use loci_graph::{GraphStore, NodeKind, VectorIndex};
use loci_llm::LlmClient;
use loci_tools::ToolContext;
use uuid::Uuid;
use crate::Skill;

pub struct DocGenerateSkill;

#[async_trait]
impl Skill for DocGenerateSkill {
    fn name(&self) -> &str { "doc_generate" }
    fn description(&self) -> &str { "Generate documentation for a function or module" }

    async fn run(&self, input: &str, llm: &dyn LlmClient, ctx: &ToolContext) -> Result<String> {
        let graph_context = build_doc_context(input, llm, ctx).await.unwrap_or_default();
        let prompt = format!(
            "Generate comprehensive documentation for the following code.\n\
             Prioritize any existing project decisions and concepts when they are relevant.\n\
             Include: purpose, parameters, return value, example usage, important notes, and design rationale.\n\
             Clearly separate factual description from inferred rationale.\n\
             Output as Rust doc comments (///) or Markdown depending on context.\n\n\
             Project context:\n{}\n\n\
             ```\n{}\n```", input
            , graph_context
        );
        let resp = llm.chat(vec![Message { role: Role::User, content: prompt }], None).await?;
        Ok(match resp { loci_llm::LlmResponse::Text(t) => t, _ => String::new() })
    }
}

pub struct PrDescriptionSkill;

#[async_trait]
impl Skill for PrDescriptionSkill {
    fn name(&self) -> &str { "pr_description" }
    fn description(&self) -> &str { "Generate a PR description from a git diff or commit list" }

    async fn run(&self, input: &str, llm: &dyn LlmClient, _ctx: &ToolContext) -> Result<String> {
        let prompt = format!(
            "Generate a pull request description for the following changes.\n\
             Include:\n\
             ## Summary\n(what and why)\n\
             ## Changes\n(bullet list of key changes)\n\
             ## Testing\n(how to test)\n\n\
             ```\n{}\n```", &input[..input.len().min(5000)]
        );
        let resp = llm.chat(vec![Message { role: Role::User, content: prompt }], None).await?;
        Ok(match resp { loci_llm::LlmResponse::Text(t) => t, _ => String::new() })
    }
}

async fn build_doc_context(input: &str, llm: &dyn LlmClient, ctx: &ToolContext) -> Result<String> {
    let Some(workdir) = &ctx.working_dir else {
        return Ok(String::new());
    };

    let db_path = std::path::Path::new(workdir).join(".bs/graph.db");
    if !db_path.exists() {
        return Ok(String::new());
    }

    let store = GraphStore::new(&db_path.to_string_lossy()).await?;
    let graph = store.load_graph().await?;
    if graph.nodes.is_empty() {
        return Ok(String::new());
    }

    let vector_index = VectorIndex::new(store.pool.clone()).await?;
    let query_vec = llm.embed(&input[..input.len().min(2000)]).await.ok();

    let mut ids: Vec<Uuid> = Vec::new();
    if let Some(qv) = query_vec.as_deref() {
        if let Ok(hits) = vector_index.search(qv, 24).await {
            ids.extend(
                hits.into_iter()
                    .filter_map(|(id, _)| {
                        graph.nodes.iter()
                            .find(|n| n.id == id && matches!(n.kind, NodeKind::Decision | NodeKind::Concept))
                            .map(|n| n.id)
                    })
            );
        }
    }

    if ids.is_empty() {
        let lowered = input.to_lowercase();
        ids.extend(
            graph.nodes.iter()
                .filter(|n| matches!(n.kind, NodeKind::Decision | NodeKind::Concept))
                .filter(|n| {
                    lowered.contains(&n.name.to_lowercase()) ||
                    n.description.as_deref().map(|d| lowered.contains(&d.to_lowercase())).unwrap_or(false)
                })
                .map(|n| n.id)
                .take(8)
        );
    }

    ids.sort_unstable();
    ids.dedup();

    let decisions: Vec<String> = ids.iter()
        .filter_map(|id| graph.nodes.iter().find(|n| n.id == *id && n.kind == NodeKind::Decision))
        .map(|n| format!("- {}: {}", n.name, n.description.as_deref().unwrap_or("")))
        .collect();

    let concepts: Vec<String> = ids.iter()
        .filter_map(|id| graph.nodes.iter().find(|n| n.id == *id && n.kind == NodeKind::Concept))
        .map(|n| format!("- {}: {}", n.name, n.description.as_deref().unwrap_or("")))
        .collect();

    if decisions.is_empty() && concepts.is_empty() {
        return Ok(String::new());
    }

    let mut out = String::new();
    if !decisions.is_empty() {
        out.push_str("## Decisions\n");
        out.push_str(&decisions.join("\n"));
        out.push('\n');
    }
    if !concepts.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("## Concepts\n");
        out.push_str(&concepts.join("\n"));
    }

    Ok(out)
}
