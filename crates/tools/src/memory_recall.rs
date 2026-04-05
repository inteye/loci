// Placeholder — will call into loci-memory crate
use async_trait::async_trait;
use serde_json::{json, Value};
use loci_core::error::Result;
use crate::{Tool, ToolContext};

pub struct MemoryRecall;

#[async_trait]
impl Tool for MemoryRecall {
    fn name(&self) -> &str { "memory_recall" }
    fn description(&self) -> &str {
        "Recall relevant memories from past sessions or project context."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "scope": { "type": "string", "enum": ["session", "project", "global"], "default": "global" }
            },
            "required": ["query"]
        })
    }
    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<Value> {
        // TODO: inject MemoryStore via Arc and call recall()
        Ok(json!({ "memories": [] }))
    }
}
