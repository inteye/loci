use async_trait::async_trait;
use loci_core::error::Result;
use loci_core::types::Message;
use serde_json::Value;

/// Unified LLM client for provider protocols such as OpenAI-compatible and Anthropic.
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDef>>,
    ) -> Result<LlmResponse>;
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
    fn model(&self) -> &str;
}

#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value, // JSON Schema
}

#[derive(Debug)]
pub enum LlmResponse {
    Text(String),
    ToolCall { name: String, arguments: Value },
}

pub mod anthropic;
pub mod config;
pub mod openai;
