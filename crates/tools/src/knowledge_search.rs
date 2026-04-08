// Placeholder — will call into loci-knowledge crate for vector search
use crate::{Tool, ToolContext};
use async_trait::async_trait;
use loci_core::error::Result;
use serde_json::{json, Value};

pub struct KnowledgeSearch;

#[async_trait]
impl Tool for KnowledgeSearch {
    fn name(&self) -> &str {
        "knowledge_search"
    }
    fn description(&self) -> &str {
        "Search the local knowledge base for relevant information. Use before answering factual questions."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "limit": { "type": "integer", "default": 5 }
            },
            "required": ["query"]
        })
    }
    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<Value> {
        // TODO: inject KnowledgeStore via Arc and call search()
        Ok(json!({ "results": [] }))
    }
}
