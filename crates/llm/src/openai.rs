use async_trait::async_trait;
use async_openai::{Client, config::OpenAIConfig, types::*};
use serde_json::Value;
use loci_core::{types::{Message, Role}, error::{AppError, Result}};
use crate::{LlmClient, LlmResponse, ToolDef};

pub struct OpenAiClient {
    client: Client<OpenAIConfig>,
    model: String,
}

impl OpenAiClient {
    pub fn new(api_key: &str, base_url: Option<&str>, model: &str) -> Self {
        let mut cfg = OpenAIConfig::new().with_api_key(api_key);
        if let Some(url) = base_url {
            cfg = cfg.with_api_base(url);
        }
        Self { client: Client::with_config(cfg), model: model.to_string() }
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn chat(&self, messages: Vec<Message>, tools: Option<Vec<ToolDef>>) -> Result<LlmResponse> {
        let msgs: Vec<ChatCompletionRequestMessage> = messages.into_iter().map(|m| {
            match m.role {
                Role::System => ChatCompletionRequestSystemMessageArgs::default()
                    .content(m.content).build().unwrap().into(),
                Role::User => ChatCompletionRequestUserMessageArgs::default()
                    .content(m.content).build().unwrap().into(),
                Role::Assistant => ChatCompletionRequestAssistantMessageArgs::default()
                    .content(m.content).build().unwrap().into(),
                Role::Tool => ChatCompletionRequestUserMessageArgs::default()
                    .content(m.content).build().unwrap().into(),
            }
        }).collect();

        let chat_tools: Option<Vec<ChatCompletionTool>> = tools.map(|defs| {
            defs.into_iter().map(|t| {
                ChatCompletionToolArgs::default()
                    .r#type(ChatCompletionToolType::Function)
                    .function(FunctionObjectArgs::default()
                        .name(t.name)
                        .description(t.description)
                        .parameters(t.parameters)
                        .build().unwrap())
                    .build().unwrap()
            }).collect()
        });

        let mut builder = CreateChatCompletionRequestArgs::default();
        builder.model(self.model.clone()).messages(msgs);
        if let Some(t) = chat_tools { builder.tools(t); }
        let request = builder.build().unwrap();

        let resp = self.client.chat().create(request).await
            .map_err(|e| AppError::Llm(e.to_string()))?;

        let choice = resp.choices.into_iter().next()
            .ok_or_else(|| AppError::Llm("empty response".into()))?;

        if let Some(tool_calls) = choice.message.tool_calls {
            if let Some(tc) = tool_calls.into_iter().next() {
                let args: Value = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or(Value::Null);
                return Ok(LlmResponse::ToolCall { name: tc.function.name, arguments: args });
            }
        }

        let text = choice.message.content.unwrap_or_default();
        Ok(LlmResponse::Text(text))
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let req = CreateEmbeddingRequestArgs::default()
            .model("text-embedding-3-small")
            .input(text)
            .build().unwrap();
        let resp = self.client.embeddings().create(req).await
            .map_err(|e| AppError::Llm(e.to_string()))?;
        Ok(resp.data.into_iter().next().map(|e| e.embedding).unwrap_or_default())
    }

    fn model(&self) -> &str { &self.model }
}
