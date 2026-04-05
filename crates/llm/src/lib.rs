use async_trait::async_trait;
use serde_json::Value;
use loci_core::types::Message;
use loci_core::error::Result;

/// Unified LLM client — wraps any OpenAI-compatible endpoint (OpenAI, Claude via proxy, Ollama, etc.)
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn chat(&self, messages: Vec<Message>, tools: Option<Vec<ToolDef>>) -> Result<LlmResponse>;
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

pub mod openai;
pub mod config;
