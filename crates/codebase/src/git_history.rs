use std::path::Path;
use serde::{Deserialize, Serialize};
use anyhow::Result;
use git2::Repository;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commit {
    pub hash: String,
    pub message: String,
    pub author: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHistory {
    pub path: String,
    pub commits: Vec<Commit>,
    /// Blame: list of (commit_hash, line_content)
    pub blame_summary: Vec<(String, String)>,
}

pub struct GitHistory;

impl GitHistory {
    /// Get recent commits for a specific file
    pub fn file_history(repo_path: &Path, file_path: &str, limit: usize) -> Result<FileHistory> {
        let repo = Repository::open(repo_path)?;
        let mut revwalk = repo.revwalk()?;
        revwalk.push_head()?;
        revwalk.set_sorting(git2::Sort::TIME)?;

        let mut commits = Vec::new();

        for oid in revwalk.flatten().take(200) {
            if commits.len() >= limit { break; }
            let commit = repo.find_commit(oid)?;
            let tree = commit.tree()?;

            // Check if this file was touched in this commit
            let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
            let diff = repo.diff_tree_to_tree(
                parent_tree.as_ref(),
                Some(&tree),
                None,
            )?;

            let touched = diff.deltas().any(|d| {
                d.new_file().path().map(|p| p.to_string_lossy().as_ref() == file_path).unwrap_or(false)
            });

            if touched {
                commits.push(Commit {
                    hash: oid.to_string()[..8].to_string(),
                    message: commit.summary().unwrap_or("").to_string(),
                    author: commit.author().name().unwrap_or("").to_string(),
                    timestamp: commit.time().seconds(),
                });
            }
        }

        let blame_summary = Self::blame_summary(&repo, file_path)?;
        Ok(FileHistory { path: file_path.to_string(), commits, blame_summary })
    }

    /// Get the most recent N commits for the whole repo
    pub fn recent_commits(repo_path: &Path, limit: usize) -> Result<Vec<Commit>> {
        let repo = Repository::open(repo_path)?;
        let mut revwalk = repo.revwalk()?;
        revwalk.push_head()?;
        revwalk.set_sorting(git2::Sort::TIME)?;

        let commits = revwalk
            .flatten()
            .take(limit)
            .filter_map(|oid| {
                repo.find_commit(oid).ok().map(|c| Commit {
                    hash: oid.to_string()[..8].to_string(),
                    message: c.summary().unwrap_or("").to_string(),
                    author: c.author().name().unwrap_or("").to_string(),
                    timestamp: c.time().seconds(),
                })
            })
            .collect();

        Ok(commits)
    }

    fn blame_summary(repo: &Repository, file_path: &str) -> Result<Vec<(String, String)>> {
        let path = Path::new(file_path);
        let blame = repo.blame_file(path, None)?;
        let file_content = std::fs::read_to_string(repo.workdir().unwrap_or(repo.path()).join(path))
            .or_else(|_| std::fs::read_to_string(path))
            .unwrap_or_default();
        let lines: Vec<&str> = file_content.lines().collect();

        let mut summary = Vec::new();
        for hunk in blame.iter().take(8) {
            let commit_hash = hunk.final_commit_id().to_string()[..8].to_string();
            let start = hunk.final_start_line().saturating_sub(1) as usize;
            let end = start.saturating_add(hunk.lines_in_hunk());
            let excerpt = lines
                .get(start..end.min(lines.len()))
                .unwrap_or(&[])
                .iter()
                .map(|line| line.trim())
                .find(|line| !line.is_empty())
                .unwrap_or("");

            let content = if excerpt.is_empty() {
                format!("lines {}-{}", start + 1, end)
            } else {
                format!("lines {}-{}: {}", start + 1, end, excerpt.chars().take(120).collect::<String>())
            };

            summary.push((commit_hash, content));
        }

        Ok(summary)
    }
}
