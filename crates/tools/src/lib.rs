use async_trait::async_trait;
use loci_core::error::Result;
use serde_json::Value;

/// Every tool must implement this trait.
/// The description is what the LLM sees — write it carefully.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<Value>;
}

#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    pub project_id: Option<uuid::Uuid>,
    pub session_id: Option<uuid::Uuid>,
    pub working_dir: Option<String>,
}

/// Registry that the Planner queries to discover available tools.
#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn register(&mut self, tool: impl Tool + 'static) {
        self.tools.push(Box::new(tool));
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    pub fn all(&self) -> &[Box<dyn Tool>] {
        &self.tools
    }
}

pub mod file;
pub mod http;
pub mod knowledge_search;
pub mod memory_recall;
pub mod shell;
