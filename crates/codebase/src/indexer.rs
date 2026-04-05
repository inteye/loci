use std::path::Path;
use anyhow::Result;
use crate::{
    scanner::{ProjectScanner, ProjectSummary, Language},
    rust_parser::{RustParser, ParsedFile},
    ts_parser::TsParser,
};

pub struct CodebaseIndexer;

pub struct CodebaseIndex {
    pub summary: ProjectSummary,
    pub parsed_files: Vec<ParsedFile>,
}

impl CodebaseIndex {
    pub fn to_llm_context(&self) -> String {
        let mut out = format!("# Project: {}\nFiles: {}, Lines: {}\n",
            self.summary.name, self.summary.files.len(), self.summary.total_lines);

        out.push_str("\n## Language breakdown\n");
        for (lang, count) in &self.summary.language_breakdown {
            out.push_str(&format!("- {}: {} files\n", lang, count));
        }

        out.push_str("\n## Public symbols\n");
        for pf in &self.parsed_files {
            let public: Vec<_> = pf.symbols.iter()
                .filter(|s| s.visibility == crate::rust_parser::Visibility::Public)
                .collect();
            if public.is_empty() { continue; }
            out.push_str(&format!("\n### {}\n", pf.path));
            for sym in &public {
                out.push_str(&format!("- `{}` ({:?}) {}\n",
                    sym.name, sym.kind,
                    sym.doc_comment.as_deref().unwrap_or("")));
            }
        }
        out
    }
}

impl CodebaseIndexer {
    pub fn index(root: &Path) -> Result<CodebaseIndex> {
        let summary = ProjectScanner::scan(root)?;

        let parsed_files: Vec<ParsedFile> = summary.files.iter()
            .filter_map(|f| match f.language {
                Language::Rust => RustParser::parse_file(&f.path).ok(),
                Language::Python | Language::TypeScript | Language::JavaScript
                    | Language::Go | Language::Java
                    => TsParser::parse_file(&f.path).ok(),
                _ => None,
            })
            .collect();

        Ok(CodebaseIndex { summary, parsed_files })
    }

    /// Incremental: only re-parse files modified after `since` (unix timestamp)
    pub fn index_incremental(root: &Path, existing: &mut CodebaseIndex, since: i64) -> Result<usize> {
        let summary = ProjectScanner::scan(root)?;
        let mut updated = 0usize;

        for file in &summary.files {
            let mtime = std::fs::metadata(&file.path)
                .and_then(|m| m.modified())
                .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64)
                .unwrap_or(0);

            if mtime <= since { continue; }

            let parsed = match file.language {
                Language::Rust => RustParser::parse_file(&file.path).ok(),
                Language::Python | Language::TypeScript | Language::JavaScript
                    | Language::Go | Language::Java
                    => TsParser::parse_file(&file.path).ok(),
                _ => None,
            };

            if let Some(pf) = parsed {
                // Replace or insert
                if let Some(existing_pf) = existing.parsed_files.iter_mut().find(|p| p.path == pf.path) {
                    *existing_pf = pf;
                } else {
                    existing.parsed_files.push(pf);
                }
                updated += 1;
            }
        }

        existing.summary = summary;
        Ok(updated)
    }
}
