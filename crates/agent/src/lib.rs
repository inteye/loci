use std::sync::Arc;
use loci_core::{types::*, error::Result};
use loci_llm::{LlmClient, LlmResponse, ToolDef};
use loci_tools::{ToolRegistry, ToolContext};
use uuid::Uuid;
use chrono::Utc;
use serde_json::json;

pub mod planner;
pub mod executor;

pub use planner::Planner;
pub use executor::Executor;

/// Top-level agent runner: plan → execute → return result
pub struct Agent {
    pub planner: Planner,
    pub executor: Executor,
}

impl Agent {
    pub fn new(llm: Arc<dyn LlmClient>, tools: Arc<ToolRegistry>) -> Self {
        Self {
            planner: Planner::new(llm.clone(), tools.clone()),
            executor: Executor::new(llm, tools),
        }
    }

    pub async fn run(&self, goal: &str, ctx: ToolContext) -> Result<String> {
        let plan = self.planner.plan(goal).await?;
        self.executor.execute(plan, ctx).await
    }
}
