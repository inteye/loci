use crate::Skill;
use anyhow::Result;
use async_trait::async_trait;
use loci_core::types::{Message, Role};
use loci_llm::LlmClient;
use loci_tools::ToolContext;

pub struct CommitMessageSkill;

#[async_trait]
impl Skill for CommitMessageSkill {
    fn name(&self) -> &str {
        "commit_message"
    }
    fn description(&self) -> &str {
        "Generate a conventional commit message from a git diff"
    }

    async fn run(&self, input: &str, llm: &dyn LlmClient, _ctx: &ToolContext) -> Result<String> {
        let prompt = format!(
            "Generate a git commit message in Conventional Commits format for this diff.\n\
             Format: <type>(<scope>): <description>\n\
             Types: feat|fix|refactor|docs|test|chore|perf\n\
             Keep the subject line under 72 chars. Add a body if needed.\n\n\
             ```diff\n{}\n```",
            &input[..input.len().min(4000)]
        );
        let resp = llm
            .chat(
                vec![Message {
                    role: Role::User,
                    content: prompt,
                }],
                None,
            )
            .await?;
        Ok(match resp {
            loci_llm::LlmResponse::Text(t) => t,
            _ => String::new(),
        })
    }
}
