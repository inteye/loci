use std::sync::Arc;
use loci_core::{types::*, error::{AppError, Result}};
use loci_llm::LlmClient;
use loci_tools::ToolRegistry;
use uuid::Uuid;
use chrono::Utc;

pub struct Planner {
    llm: Arc<dyn LlmClient>,
    tools: Arc<ToolRegistry>,
}

impl Planner {
    pub fn new(llm: Arc<dyn LlmClient>, tools: Arc<ToolRegistry>) -> Self {
        Self { llm, tools }
    }

    /// Ask the LLM to decompose the goal into a DAG of tasks.
    pub async fn plan(&self, goal: &str) -> Result<ExecutionPlan> {
        let tool_list: String = self.tools.all().iter()
            .map(|t| format!("- {}: {}", t.name(), t.description()))
            .collect::<Vec<_>>()
            .join("\n");

        let system = format!(
            "You are a task planner. Decompose the user's goal into a list of tasks.\n\
             Available tools:\n{tool_list}\n\n\
             Respond with JSON only:\n\
             {{\"tasks\": [{{\"id\": \"<uuid>\", \"goal\": \"...\", \"tools\": [\"tool_name\"], \"depends_on\": []}}]}}"
        );

        let messages = vec![
            Message { role: Role::System, content: system },
            Message { role: Role::User, content: goal.to_string() },
        ];

        let response = self.llm.chat(messages, None).await?;
        let text = match response {
            loci_llm::LlmResponse::Text(t) => t,
            _ => return Err(AppError::Planning("unexpected tool call from planner".into())),
        };

        // Parse the JSON plan
        let raw: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| AppError::Planning(format!("invalid plan JSON: {e}")))?;

        let tasks = raw["tasks"].as_array()
            .ok_or_else(|| AppError::Planning("missing tasks array".into()))?
            .iter()
            .map(|t| Task {
                id: t["id"].as_str().and_then(|s| Uuid::parse_str(s).ok()).unwrap_or_else(Uuid::new_v4),
                goal: t["goal"].as_str().unwrap_or("").to_string(),
                depends_on: t["depends_on"].as_array().map(|a| {
                    a.iter().filter_map(|v| v.as_str().and_then(|s| Uuid::parse_str(s).ok())).collect()
                }).unwrap_or_default(),
                tools: t["tools"].as_array().map(|a| {
                    a.iter().filter_map(|v| v.as_str().map(String::from)).collect()
                }).unwrap_or_default(),
                context_refs: vec![],
                status: TaskStatus::Pending,
                result: None,
            })
            .collect();

        Ok(ExecutionPlan {
            id: Uuid::new_v4(),
            goal: goal.to_string(),
            tasks,
            status: PlanStatus::Pending,
            created_at: Utc::now(),
        })
    }
}
