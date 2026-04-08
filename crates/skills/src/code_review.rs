use crate::Skill;
use anyhow::Result;
use async_trait::async_trait;
use loci_core::types::{Message, Role};
use loci_llm::LlmClient;
use loci_tools::ToolContext;

pub struct CodeReviewSkill;

#[async_trait]
impl Skill for CodeReviewSkill {
    fn name(&self) -> &str {
        "code_review"
    }
    fn description(&self) -> &str {
        "Review code for bugs, style issues, and improvements"
    }

    async fn run(&self, input: &str, llm: &dyn LlmClient, _ctx: &ToolContext) -> Result<String> {
        let prompt = format!(
            "Review the following code. Provide:\n\
             1. **Bugs** — actual or potential bugs\n\
             2. **Style** — readability and convention issues\n\
             3. **Improvements** — performance, safety, or design suggestions\n\
             4. **Summary** — overall assessment (1-2 sentences)\n\n\
             Output Markdown.\n\n```\n{}\n```",
            input
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
