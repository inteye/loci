use crate::{Tool, ToolContext};
use async_trait::async_trait;
use loci_core::error::{AppError, Result};
use serde_json::{json, Value};

pub struct FileRead;
pub struct FileWrite;

#[async_trait]
impl Tool for FileRead {
    fn name(&self) -> &str {
        "file_read"
    }
    fn description(&self) -> &str {
        "Read the contents of a file at the given path."
    }
    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] })
    }
    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<Value> {
        let path = params["path"]
            .as_str()
            .ok_or_else(|| AppError::Tool("missing path".into()))?;
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| AppError::Tool(e.to_string()))?;
        Ok(json!({ "content": content }))
    }
}

#[async_trait]
impl Tool for FileWrite {
    fn name(&self) -> &str {
        "file_write"
    }
    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        })
    }
    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<Value> {
        let path = params["path"]
            .as_str()
            .ok_or_else(|| AppError::Tool("missing path".into()))?;
        let content = params["content"].as_str().unwrap_or("");
        tokio::fs::write(path, content)
            .await
            .map_err(|e| AppError::Tool(e.to_string()))?;
        Ok(json!({ "ok": true }))
    }
}
