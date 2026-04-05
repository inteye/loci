use async_trait::async_trait;
use anyhow::Result;
use loci_core::types::{Message, Role};
use loci_llm::LlmClient;
use loci_tools::ToolContext;
use crate::Skill;

pub struct DocGenerateSkill;

#[async_trait]
impl Skill for DocGenerateSkill {
    fn name(&self) -> &str { "doc_generate" }
    fn description(&self) -> &str { "Generate documentation for a function or module" }

    async fn run(&self, input: &str, llm: &dyn LlmClient, _ctx: &ToolContext) -> Result<String> {
        let prompt = format!(
            "Generate comprehensive documentation for the following code.\n\
             Include: purpose, parameters, return value, example usage, and any important notes.\n\
             Output as Rust doc comments (///) or Markdown depending on context.\n\n\
             ```\n{}\n```", input
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
