use anyhow::Result;
use chrono::Utc;
use loci_core::types::{Memory, MemoryScope};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

/// Persistent memory store — three scopes: session, project, global.
/// Embeddings stored as BLOB for semantic recall (same scheme as VectorIndex).
pub struct MemoryStore {
    pool: SqlitePool,
}

impl MemoryStore {
    pub async fn new(db_path: &str) -> Result<Self> {
        let pool = SqlitePool::connect(&format!("sqlite://{}?mode=rwc", db_path)).await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS memories (
                id         TEXT PRIMARY KEY,
                scope      TEXT NOT NULL,
                content    TEXT NOT NULL,
                embedding  BLOB,
                project_id TEXT,
                created_at TEXT NOT NULL,
                expires_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_memories_scope ON memories(scope);
            CREATE INDEX IF NOT EXISTS idx_memories_project ON memories(project_id);",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save(&self, memory: &Memory) -> Result<()> {
        let embedding_blob: Option<Vec<u8>> = memory
            .embedding
            .as_ref()
            .map(|v| v.iter().flat_map(|f| f.to_le_bytes()).collect());
        sqlx::query(
            "INSERT OR REPLACE INTO memories
             (id, scope, content, embedding, project_id, created_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(memory.id.to_string())
        .bind(serde_json::to_string(&memory.scope)?)
        .bind(&memory.content)
        .bind(embedding_blob)
        .bind(memory.project_id.map(|id| id.to_string()))
        .bind(memory.created_at.to_rfc3339())
        .bind(memory.expires_at.map(|t| t.to_rfc3339()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Semantic recall: find top-k memories similar to query embedding.
    /// Falls back to recent memories if no embeddings stored.
    pub async fn recall(
        &self,
        query_embedding: Option<&[f32]>,
        scope: Option<MemoryScope>,
        project_id: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<Memory>> {
        let scope_filter = scope
            .as_ref()
            .map(|s| serde_json::to_string(s).unwrap_or_default());
        let project_filter = project_id.map(|id| id.to_string());

        let rows = sqlx::query(
            "SELECT id, scope, content, embedding, project_id, created_at, expires_at
             FROM memories
             WHERE (? IS NULL OR scope = ?)
               AND (? IS NULL OR project_id = ?)
               AND (expires_at IS NULL OR expires_at > ?)
             ORDER BY created_at DESC
             LIMIT 500",
        )
        .bind(&scope_filter)
        .bind(&scope_filter)
        .bind(&project_filter)
        .bind(&project_filter)
        .bind(Utc::now().to_rfc3339())
        .fetch_all(&self.pool)
        .await?;

        let mut memories: Vec<(Memory, Option<Vec<f32>>)> = rows
            .into_iter()
            .filter_map(|row| {
                let scope: MemoryScope = serde_json::from_str(row.get::<&str, _>("scope")).ok()?;
                let embedding: Option<Vec<f32>> =
                    row.get::<Option<Vec<u8>>, _>("embedding").map(|b| {
                        b.chunks_exact(4)
                            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                            .collect()
                    });
                let mem = Memory {
                    id: Uuid::parse_str(row.get("id")).ok()?,
                    scope,
                    content: row.get("content"),
                    embedding: embedding.clone(),
                    project_id: row
                        .get::<Option<&str>, _>("project_id")
                        .and_then(|s| Uuid::parse_str(s).ok()),
                    created_at: chrono::DateTime::parse_from_rfc3339(row.get("created_at"))
                        .ok()?
                        .with_timezone(&Utc),
                    expires_at: row
                        .get::<Option<&str>, _>("expires_at")
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Utc)),
                };
                Some((mem, embedding))
            })
            .collect();

        // If we have a query embedding, rank by cosine similarity
        if let Some(q) = query_embedding {
            memories.sort_by(|(_, a), (_, b)| {
                let sa = a.as_ref().map(|v| cosine(q, v)).unwrap_or(0.0);
                let sb = b.as_ref().map(|v| cosine(q, v)).unwrap_or(0.0);
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        Ok(memories.into_iter().take(limit).map(|(m, _)| m).collect())
    }

    pub async fn count(&self) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM memories")
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// Convenience: create a new memory and save it
pub async fn remember(
    store: &MemoryStore,
    content: &str,
    scope: MemoryScope,
    project_id: Option<Uuid>,
    embedding: Option<Vec<f32>>,
) -> Result<Memory> {
    let mem = Memory {
        id: Uuid::new_v4(),
        scope,
        content: content.to_string(),
        embedding,
        project_id,
        created_at: Utc::now(),
        expires_at: None,
    };
    store.save(&mem).await?;
    Ok(mem)
}
