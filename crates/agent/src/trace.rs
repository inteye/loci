use std::path::Path;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use loci_codebase::{Commit, GitHistory};
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
        let stable_timeline = build_commit_timeline(&history.commits);
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

        let mut report = self.run_report_prompt(prompt).await?;
        report.timeline = merge_timeline(stable_timeline, report.timeline);
        Ok(report)
    }

    pub async fn analyze_diff(&self, commit_ref: &str, diff: &str) -> Result<TraceReport> {
        let stable_timeline = build_diff_timeline(commit_ref, diff);
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

        let mut report = self.run_report_prompt(prompt).await?;
        report.timeline = merge_timeline(stable_timeline, report.timeline);
        Ok(report)
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

fn build_commit_timeline(commits: &[Commit]) -> Vec<TraceTimelineEvent> {
    let mut events = Vec::new();
    for commit in commits.iter().rev() {
        let when = Utc.timestamp_opt(commit.timestamp, 0)
            .single()
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| commit.timestamp.to_string());
        let change = format!("{} [{}] by {}", commit.message, commit.hash, commit.author);

        let should_merge = events.last_mut().map(|last: &mut TraceTimelineEvent| {
            if last.when == when && last.change == change {
                true
            } else {
                false
            }
        }).unwrap_or(false);

        if !should_merge {
            events.push(TraceTimelineEvent { when, change });
        }
    }
    events
}

fn build_diff_timeline(commit_ref: &str, diff: &str) -> Vec<TraceTimelineEvent> {
    let files_changed = diff.lines()
        .filter_map(|line| line.strip_prefix("+++ b/").or_else(|| line.strip_prefix("--- a/")))
        .filter(|line| *line != "/dev/null")
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    let summary = if files_changed.is_empty() {
        format!("Diff analyzed for {}", commit_ref)
    } else {
        format!("Changed files: {}", files_changed.join(", "))
    };

    vec![TraceTimelineEvent {
        when: commit_ref.to_string(),
        change: summary,
    }]
}

fn merge_timeline(mut stable: Vec<TraceTimelineEvent>, inferred: Vec<TraceTimelineEvent>) -> Vec<TraceTimelineEvent> {
    for event in inferred {
        let exists = stable.iter().any(|existing| existing.when == event.when && existing.change == event.change);
        if !exists {
            stable.push(event);
        }
    }
    stable
}

#[cfg(test)]
mod tests {
    use super::{build_commit_timeline, build_diff_timeline, merge_timeline, TraceEvidence, TraceReport, TraceTimelineEvent};
    use loci_codebase::Commit;

    #[test]
    fn markdown_includes_all_sections_when_present() {
        let report = TraceReport {
            summary: "Summarized change".to_string(),
            timeline: vec![TraceTimelineEvent {
                when: "2026-04-06".to_string(),
                change: "Refactored trace pipeline".to_string(),
            }],
            evidence: vec![TraceEvidence {
                source: "commit".to_string(),
                detail: "b841111".to_string(),
            }],
            confidence: "high".to_string(),
            open_questions: vec!["Need more blame detail".to_string()],
        };

        let markdown = report.to_markdown("Trace Report");
        assert!(markdown.contains("# Trace Report"));
        assert!(markdown.contains("## Summary"));
        assert!(markdown.contains("## Timeline"));
        assert!(markdown.contains("## Evidence"));
        assert!(markdown.contains("## Confidence"));
        assert!(markdown.contains("## Open Questions"));
    }

    #[test]
    fn commit_timeline_is_stable_and_oldest_first() {
        let commits = vec![
            Commit {
                hash: "bbb2222".to_string(),
                message: "second change".to_string(),
                author: "Bob".to_string(),
                timestamp: 1_710_086_400,
            },
            Commit {
                hash: "aaa1111".to_string(),
                message: "first change".to_string(),
                author: "Alice".to_string(),
                timestamp: 1_709_913_600,
            },
        ];

        let timeline = build_commit_timeline(&commits);
        assert_eq!(timeline.len(), 2);
        assert!(timeline[0].change.contains("first change"));
        assert!(timeline[1].change.contains("second change"));
    }

    #[test]
    fn merge_timeline_keeps_stable_items_and_adds_new_inferred_items() {
        let merged = merge_timeline(
            vec![TraceTimelineEvent { when: "2026-04-01".to_string(), change: "base".to_string() }],
            vec![
                TraceTimelineEvent { when: "2026-04-01".to_string(), change: "base".to_string() },
                TraceTimelineEvent { when: "2026-04-02".to_string(), change: "extra".to_string() },
            ],
        );

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].change, "base");
        assert_eq!(merged[1].change, "extra");
    }

    #[test]
    fn diff_timeline_mentions_changed_files() {
        let timeline = build_diff_timeline("HEAD", "--- a/foo.rs\n+++ b/foo.rs\n--- a/bar.rs\n+++ b/bar.rs");
        assert_eq!(timeline.len(), 1);
        assert!(timeline[0].change.contains("foo.rs"));
        assert!(timeline[0].change.contains("bar.rs"));
    }
}
