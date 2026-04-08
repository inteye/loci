use crate::{Tool, ToolContext};
use async_trait::async_trait;
use loci_core::error::{AppError, Result};
use serde_json::{json, Value};

pub struct HttpRequest;

#[async_trait]
impl Tool for HttpRequest {
    fn name(&self) -> &str {
        "http_request"
    }
    fn description(&self) -> &str {
        "Make an HTTP request to an external API or URL."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "method": { "type": "string", "enum": ["GET","POST","PUT","DELETE"], "default": "GET" },
                "url": { "type": "string" },
                "headers": { "type": "object" },
                "body": { "type": "string" }
            },
            "required": ["url"]
        })
    }
    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<Value> {
        let url = params["url"]
            .as_str()
            .ok_or_else(|| AppError::Tool("missing url".into()))?;
        let method = params["method"].as_str().unwrap_or("GET");
        let client = reqwest::Client::new();

        let mut req = match method {
            "POST" => client.post(url),
            "PUT" => client.put(url),
            "DELETE" => client.delete(url),
            _ => client.get(url),
        };

        if let Some(body) = params["body"].as_str() {
            req = req.body(body.to_string());
        }

        let resp = req
            .send()
            .await
            .map_err(|e| AppError::Tool(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| AppError::Tool(e.to_string()))?;

        Ok(json!({ "status": status, "body": text }))
    }
}
