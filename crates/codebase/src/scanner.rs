use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;
use anyhow::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub root: PathBuf,
    pub name: String,
    pub files: Vec<FileInfo>,
    pub total_lines: usize,
    pub language_breakdown: Vec<(String, usize)>, // (lang, file_count)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: PathBuf,
    pub relative_path: String,
    pub language: Language,
    pub size_bytes: u64,
    pub line_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
    Java,
    Markdown,
    Toml,
    Yaml,
    Other(String),
}

impl Language {
    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "rs" => Language::Rust,
            "py" => Language::Python,
            "ts" | "tsx" => Language::TypeScript,
            "js" | "jsx" => Language::JavaScript,
            "go" => Language::Go,
            "java" => Language::Java,
            "md" => Language::Markdown,
            "toml" => Language::Toml,
            "yaml" | "yml" => Language::Yaml,
            other => Language::Other(other.to_string()),
        }
    }

    pub fn is_code(&self) -> bool {
        matches!(self, Language::Rust | Language::Python | Language::TypeScript
            | Language::JavaScript | Language::Go | Language::Java)
    }
}

pub struct ProjectScanner;

impl ProjectScanner {
    pub fn scan(root: &Path) -> Result<ProjectSummary> {
        let name = root.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let mut files = Vec::new();
        let mut total_lines = 0;

        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !is_ignored(e.path()))
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path().to_path_buf();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let language = Language::from_extension(ext);
            let size_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);

            let line_count = if language.is_code() || matches!(language, Language::Markdown | Language::Toml | Language::Yaml) {
                std::fs::read_to_string(&path)
                    .map(|s| s.lines().count())
                    .unwrap_or(0)
            } else {
                0
            };

            total_lines += line_count;

            let relative_path = path.strip_prefix(root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            files.push(FileInfo { path, relative_path, language, size_bytes, line_count });
        }

        // Language breakdown
        let mut lang_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for f in &files {
            if f.language.is_code() {
                let key = format!("{:?}", f.language);
                *lang_counts.entry(key).or_default() += 1;
            }
        }
        let mut language_breakdown: Vec<(String, usize)> = lang_counts.into_iter().collect();
        language_breakdown.sort_by(|a, b| b.1.cmp(&a.1));

        Ok(ProjectSummary { root: root.to_path_buf(), name, files, total_lines, language_breakdown })
    }
}

fn is_ignored(path: &Path) -> bool {
    let ignored = [
        ".git", "target", "node_modules", ".next", "dist", "build",
        "__pycache__", ".venv", "venv", ".idea", ".vscode",
    ];
    path.components().any(|c| {
        ignored.iter().any(|i| c.as_os_str() == *i)
    })
}
