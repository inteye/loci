use async_trait::async_trait;
use loci_core::{
    error::{AppError, Result},
    types::{Message, Role},
};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::{LlmClient, LlmResponse, ToolDef};

pub struct AnthropicClient {
    client: Client,
    api_key: String,
    endpoint: String,
    model: String,
}

impl AnthropicClient {
    pub fn new(api_key: &str, base_url: Option<&str>, model: &str) -> Self {
        let endpoint = match base_url {
            Some(url) if url.ends_with("/messages") => url.to_string(),
            Some(url) => format!("{}/messages", url.trim_end_matches('/')),
            None => "https://api.anthropic.com/v1/messages".to_string(),
        };

        Self {
            client: Client::new(),
            api_key: api_key.to_string(),
            endpoint,
            model: model.to_string(),
        }
    }
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: &'static str,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

#[derive(Deserialize)]
struct AnthropicContent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn chat(
        &self,
        messages: Vec<Message>,
        _tools: Option<Vec<ToolDef>>,
    ) -> Result<LlmResponse> {
        let mut system_parts = Vec::new();
        let mut request_messages = Vec::new();

        for message in messages {
            match message.role {
                Role::System => system_parts.push(message.content),
                Role::User => request_messages.push(AnthropicMessage {
                    role: "user",
                    content: message.content,
                }),
                Role::Assistant => request_messages.push(AnthropicMessage {
                    role: "assistant",
                    content: message.content,
                }),
                Role::Tool => request_messages.push(AnthropicMessage {
                    role: "user",
                    content: message.content,
                }),
            }
        }

        let request = AnthropicRequest {
            model: self.model.clone(),
            max_tokens: 4096,
            system: (!system_parts.is_empty()).then(|| system_parts.join("\n\n")),
            messages: request_messages,
        };

        let response = self
            .client
            .post(&self.endpoint)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&request)
            .send()
            .await
            .map_err(|e| AppError::Llm(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::Llm(format!(
                "anthropic request failed: {} {}",
                status, body
            )));
        }

        let body: AnthropicResponse = response
            .json()
            .await
            .map_err(|e| AppError::Llm(e.to_string()))?;

        let text = body
            .content
            .into_iter()
            .filter(|part| part.kind == "text")
            .map(|part| part.text)
            .collect::<Vec<_>>()
            .join("\n");

        Ok(LlmResponse::Text(text))
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Err(AppError::Llm(
            "Anthropic protocol provider does not support embeddings in this client".to_string(),
        ))
    }

    fn model(&self) -> &str {
        &self.model
    }
}
