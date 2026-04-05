use sqlx::{SqlitePool, Row};
use uuid::Uuid;
use chrono::Utc;
use anyhow::Result;
use loci_core::types::{Knowledge, KnowledgeSource};

pub struct KnowledgeStore {
    pool: SqlitePool,
}

impl KnowledgeStore {
    pub async fn new(db_path: &str) -> Result<Self> {
        let pool = SqlitePool::connect(&format!("sqlite://{}?mode=rwc", db_path)).await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS knowledge (
                id         TEXT PRIMARY KEY,
                source     TEXT NOT NULL,
                content    TEXT NOT NULL,
                embedding  BLOB,
                project_id TEXT,
                tags       TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_knowledge_project ON knowledge(project_id);"
        ).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn save(&self, k: &Knowledge) -> Result<()> {
        let blob: Option<Vec<u8>> = k.embedding.as_ref().map(|v| {
            v.iter().flat_map(|f| f.to_le_bytes()).collect()
        });
        sqlx::query(
            "INSERT OR REPLACE INTO knowledge (id, source, content, embedding, project_id, tags, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(k.id.to_string())
        .bind(serde_json::to_string(&k.source)?)
        .bind(&k.content)
        .bind(blob)
        .bind(k.project_id.map(|id| id.to_string()))
        .bind(serde_json::to_string(&k.tags)?)
        .bind(k.created_at.to_rfc3339())
        .execute(&self.pool).await?;
        Ok(())
    }

    /// Semantic search — cosine over stored embeddings, falls back to keyword scan
    pub async fn search(&self, query_embedding: Option<&[f32]>, keyword: Option<&str>, limit: usize) -> Result<Vec<Knowledge>> {
        let rows = sqlx::query(
            "SELECT id, source, content, embedding, project_id, tags, created_at FROM knowledge ORDER BY created_at DESC LIMIT 1000"
        ).fetch_all(&self.pool).await?;

        let mut items: Vec<(Knowledge, f32)> = rows.into_iter().filter_map(|row| {
            let source: KnowledgeSource = serde_json::from_str(row.get::<&str,_>("source")).ok()?;
            let tags: Vec<String> = serde_json::from_str(row.get::<&str,_>("tags")).unwrap_or_default();
            let embedding: Option<Vec<f32>> = row.get::<Option<Vec<u8>>,_>("embedding").map(|b| {
                b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0],c[1],c[2],c[3]])).collect()
            });
            let content: String = row.get("content");

            // keyword filter
            if let Some(kw) = keyword {
                if !content.to_lowercase().contains(&kw.to_lowercase()) { return None; }
            }

            let score = query_embedding
                .zip(embedding.as_ref())
                .map(|(q, e)| cosine(q, e))
                .unwrap_or(0.0);

            let k = Knowledge {
                id: Uuid::parse_str(row.get("id")).ok()?,
                source,
                content,
                embedding,
                project_id: row.get::<Option<&str>,_>("project_id").and_then(|s| Uuid::parse_str(s).ok()),
                tags,
                created_at: chrono::DateTime::parse_from_rfc3339(row.get("created_at")).ok()?.with_timezone(&Utc),
            };
            Some((k, score))
        }).collect();

        items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(items.into_iter().take(limit).map(|(k,_)| k).collect())
    }

    pub async fn list(&self, limit: usize) -> Result<Vec<Knowledge>> {
        self.search(None, None, limit).await
    }

    pub async fn count(&self) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM knowledge")
            .fetch_one(&self.pool).await?;
        Ok(n)
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() { return 0.0; }
    let dot: f32 = a.iter().zip(b).map(|(x,y)| x*y).sum();
    let na = a.iter().map(|x| x*x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x*x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na * nb) }
}

/// Ingest a local file into the knowledge store
pub async fn ingest_file(store: &KnowledgeStore, path: &std::path::Path, embedding: Option<Vec<f32>>, project_id: Option<Uuid>) -> Result<Knowledge> {
    let content = std::fs::read_to_string(path)?;
    let k = Knowledge {
        id: Uuid::new_v4(),
        source: KnowledgeSource::File { path: path.to_string_lossy().to_string() },
        content,
        embedding,
        project_id,
        tags: vec![],
        created_at: Utc::now(),
    };
    store.save(&k).await?;
    Ok(k)
}

/// Fetch a URL and ingest its text content
pub async fn ingest_url(store: &KnowledgeStore, url: &str, embedding: Option<Vec<f32>>, project_id: Option<Uuid>) -> Result<Knowledge> {
    let text = reqwest::get(url).await?.text().await?;
    // Strip HTML tags naively
    let content = strip_html(&text);
    let k = Knowledge {
        id: Uuid::new_v4(),
        source: KnowledgeSource::Url { url: url.to_string() },
        content,
        embedding,
        project_id,
        tags: vec![],
        created_at: Utc::now(),
    };
    store.save(&k).await?;
    Ok(k)
}

fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    // Collapse whitespace
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
