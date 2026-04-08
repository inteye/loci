use anyhow::Result;
/// Skills are higher-level capabilities built on top of Tools + LLM.
/// Each Skill encapsulates a complete workflow (prompt template + tool calls + output format).
use async_trait::async_trait;
use loci_llm::LlmClient;
use loci_tools::ToolContext;

#[async_trait]
pub trait Skill: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn run(&self, input: &str, llm: &dyn LlmClient, ctx: &ToolContext) -> Result<String>;
}

pub mod code_review;
pub mod commit_message;
pub mod doc_generate;
pub mod pr_description;

pub use code_review::CodeReviewSkill;
pub use commit_message::CommitMessageSkill;
pub use doc_generate::DocGenerateSkill;
pub use pr_description::PrDescriptionSkill;

/// Registry of available skills
pub struct SkillRegistry {
    skills: Vec<Box<dyn Skill>>,
}

impl Default for SkillRegistry {
    fn default() -> Self {
        let mut r = Self { skills: vec![] };
        r.register(CodeReviewSkill);
        r.register(CommitMessageSkill);
        r.register(DocGenerateSkill);
        r.register(PrDescriptionSkill);
        r
    }
}

impl SkillRegistry {
    pub fn register(&mut self, skill: impl Skill + 'static) {
        self.skills.push(Box::new(skill));
    }
    pub fn get(&self, name: &str) -> Option<&dyn Skill> {
        self.skills
            .iter()
            .find(|s| s.name() == name)
            .map(|s| s.as_ref())
    }
    pub fn list(&self) -> Vec<(&str, &str)> {
        self.skills
            .iter()
            .map(|s| (s.name(), s.description()))
            .collect()
    }
}
