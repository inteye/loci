use std::path::Path;
use std::sync::Arc;

use loci_codebase::GitHistory;
use loci_core::error::Result;
use loci_core::types::{Message, Role};
use loci_llm::{LlmClient, LlmResponse};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TraceTimelineEvent {
    pub when: String,
    pub change: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TraceEvidence {
    pub source: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TraceReport {
    pub summary: String,
    pub timeline: Vec<TraceTimelineEvent>,
    pub evidence: Vec<TraceEvidence>,
    pub confidence: String,
    pub open_questions: Vec<String>,
}

impl TraceReport {
    pub fn to_markdown(&self, title: &str) -> String {
        let mut out = format!("# {}\n\n## Summary\n{}\n", title, self.summary);

        if !self.timeline.is_empty() {
            out.push_str("\n## Timeline\n");
            for event in &self.timeline {
                out.push_str(&format!("- {}: {}\n", event.when, event.change));
            }
        }

        if !self.evidence.is_empty() {
            out.push_str("\n## Evidence\n");
            for item in &self.evidence {
                out.push_str(&format!("- {}: {}\n", item.source, item.detail));
            }
        }

        out.push_str(&format!("\n## Confidence\n{}\n", self.confidence));

        if !self.open_questions.is_empty() {
            out.push_str("\n## Open Questions\n");
            for question in &self.open_questions {
                out.push_str(&format!("- {}\n", question));
            }
        }

        out
    }
}

pub struct TraceAgent {
    llm: Arc<dyn LlmClient>,
}

impl TraceAgent {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        Self { llm }
    }

    pub async fn explain_file(&self, repo_path: &Path, file_path: &str, code: &str) -> Result<TraceReport> {
        let history = GitHistory::file_history(repo_path, file_path, 6)?;
        let timeline = history.commits.iter()
            .map(|c| format!("- {} [{}] {} — {}", c.timestamp, c.hash, c.message, c.author))
            .collect::<Vec<_>>()
            .join("\n");
        let blame = history.blame_summary.iter()
            .map(|(hash, line)| format!("- [{}] {}", hash, line))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "You are a trace analysis agent for a codebase understanding tool.\n\
             Analyze the file history and current code. Infer why this file exists, how it evolved, \
             and what evidence supports your conclusion.\n\
             Respond with JSON only using this schema:\n\
             {{\"summary\":\"...\",\"timeline\":[{{\"when\":\"...\",\"change\":\"...\"}}],\
             \"evidence\":[{{\"source\":\"commit|blame|code\",\"detail\":\"...\"}}],\
             \"confidence\":\"high|medium|low\",\"open_questions\":[\"...\"]}}\n\n\
             File: {file_path}\n\n\
             Recent commits:\n{timeline}\n\n\
             Blame highlights:\n{blame}\n\n\
             Code snippet:\n```rust\n{}\n```",
            &code[..code.len().min(6000)]
        );

        self.run_report_prompt(prompt).await
    }

    pub async fn analyze_diff(&self, commit_ref: &str, diff: &str) -> Result<TraceReport> {
        let prompt = format!(
            "You are a trace analysis agent for a codebase understanding tool.\n\
             Analyze the diff and explain what changed, why it may have changed, and what evidence is available.\n\
             Respond with JSON only using this schema:\n\
             {{\"summary\":\"...\",\"timeline\":[{{\"when\":\"...\",\"change\":\"...\"}}],\
             \"evidence\":[{{\"source\":\"diff\",\"detail\":\"...\"}}],\
             \"confidence\":\"high|medium|low\",\"open_questions\":[\"...\"]}}\n\n\
             Commit or range: {commit_ref}\n\n\
             Diff:\n```diff\n{}\n```",
            &diff[..diff.len().min(8000)]
        );

        self.run_report_prompt(prompt).await
    }

    async fn run_report_prompt(&self, prompt: String) -> Result<TraceReport> {
        let response = self.llm.chat(
            vec![
                Message { role: Role::System, content: "You produce strict JSON trace reports.".to_string() },
                Message { role: Role::User, content: prompt },
            ],
            None,
        ).await?;

        match response {
            LlmResponse::Text(text) => {
                let parsed = serde_json::from_str::<TraceReport>(&text).unwrap_or_else(|_| TraceReport {
                    summary: text.trim().to_string(),
                    confidence: "low".to_string(),
                    ..Default::default()
                });
                Ok(parsed)
            }
            _ => Ok(TraceReport {
                summary: "Trace agent returned a non-text response.".to_string(),
                confidence: "low".to_string(),
                ..Default::default()
            }),
        }
    }
}
