use loci_core::{
    error::{AppError, Result},
    types::*,
};
use loci_llm::{LlmClient, LlmResponse, ToolDef};
use loci_tools::{ToolContext, ToolRegistry};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

pub struct Executor {
    llm: Arc<dyn LlmClient>,
    tools: Arc<ToolRegistry>,
}

impl Executor {
    pub fn new(llm: Arc<dyn LlmClient>, tools: Arc<ToolRegistry>) -> Self {
        Self { llm, tools }
    }

    pub async fn execute(&self, mut plan: ExecutionPlan, ctx: ToolContext) -> Result<String> {
        let mut results: HashMap<Uuid, String> = HashMap::new();

        // Simple topological execution (no parallelism yet — add tokio::spawn later)
        loop {
            let ready: Vec<usize> = plan
                .tasks
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    t.status == TaskStatus::Pending
                        && t.depends_on.iter().all(|dep| results.contains_key(dep))
                })
                .map(|(i, _)| i)
                .collect();

            if ready.is_empty() {
                break;
            }

            for idx in ready {
                let task = &mut plan.tasks[idx];
                task.status = TaskStatus::Running;

                let result = self.run_task(task, &results, &ctx).await;
                match result {
                    Ok(r) => {
                        results.insert(task.id, r.clone());
                        task.result = Some(r);
                        task.status = TaskStatus::Done;
                    }
                    Err(e) => {
                        task.status = TaskStatus::Failed(e.to_string());
                    }
                }
            }
        }

        // Aggregate: return last done task's result or summarize
        let summary = plan
            .tasks
            .iter()
            .filter_map(|t| t.result.as_deref())
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok(summary)
    }

    async fn run_task(
        &self,
        task: &Task,
        prior_results: &HashMap<Uuid, String>,
        ctx: &ToolContext,
    ) -> Result<String> {
        // Build tool defs for this task's allowed tools
        let tool_defs: Vec<ToolDef> = task
            .tools
            .iter()
            .filter_map(|name| self.tools.get(name))
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters_schema(),
            })
            .collect();

        // Inject prior results as context
        let context_str = prior_results
            .values()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let system = "You are a task executor. Complete the given task using the available tools. \
                      Be concise and return only the result.";

        let user = if context_str.is_empty() {
            task.goal.clone()
        } else {
            format!(
                "Context from previous tasks:\n{context_str}\n\nTask: {}",
                task.goal
            )
        };

        let messages = vec![
            Message {
                role: Role::System,
                content: system.to_string(),
            },
            Message {
                role: Role::User,
                content: user,
            },
        ];

        // Agentic loop: keep calling until no more tool calls
        let mut msgs = messages;
        loop {
            let resp = self.llm.chat(msgs.clone(), Some(tool_defs.clone())).await?;
            match resp {
                LlmResponse::Text(t) => return Ok(t),
                LlmResponse::ToolCall { name, arguments } => {
                    let tool = self
                        .tools
                        .get(&name)
                        .ok_or_else(|| AppError::Tool(format!("unknown tool: {name}")))?;
                    let result = tool.execute(arguments.clone(), ctx).await?;
                    // Feed result back into conversation
                    msgs.push(Message {
                        role: Role::Assistant,
                        content: format!("Calling {name}"),
                    });
                    msgs.push(Message {
                        role: Role::Tool,
                        content: result.to_string(),
                    });
                }
            }
        }
    }
}
