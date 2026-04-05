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

        Ok(FileHistory { path: file_path.to_string(), commits, blame_summary: vec![] })
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
}
