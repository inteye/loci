use std::sync::Arc;
use loci_core::error::Result;
use loci_llm::LlmClient;
use loci_tools::{ToolRegistry, ToolContext};

pub mod planner;
pub mod executor;
pub mod trace;

pub use planner::Planner;
pub use executor::Executor;
pub use trace::{TraceAgent, TraceEvidence, TraceReport, TraceTimelineEvent};

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
