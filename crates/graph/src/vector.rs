use sqlx::SqlitePool;
use uuid::Uuid;
use anyhow::Result;

/// Stores and queries node embeddings in SQLite.
/// Embeddings are stored as BLOB (f32 little-endian bytes).
/// Similarity search is done in-process — fast enough for <100K nodes.
pub struct VectorIndex {
    pool: SqlitePool,
}

impl VectorIndex {
    pub async fn new(pool: SqlitePool) -> Result<Self> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS embeddings (
                node_id TEXT PRIMARY KEY,
                vector  BLOB NOT NULL
            )"
        ).execute(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn upsert(&self, node_id: Uuid, embedding: &[f32]) -> Result<()> {
        let blob = floats_to_bytes(embedding);
        sqlx::query("INSERT OR REPLACE INTO embeddings (node_id, vector) VALUES (?, ?)")
            .bind(node_id.to_string())
            .bind(blob)
            .execute(&self.pool).await?;
        Ok(())
    }

    /// Return the top-k most similar node IDs to the query embedding.
    pub async fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<(Uuid, f32)>> {
        let rows = sqlx::query_as::<_, (String, Vec<u8>)>(
            "SELECT node_id, vector FROM embeddings"
        ).fetch_all(&self.pool).await?;

        let mut scored: Vec<(Uuid, f32)> = rows.into_iter()
            .filter_map(|(id, blob)| {
                let vec = bytes_to_floats(&blob);
                let score = cosine_similarity(query, &vec);
                Uuid::parse_str(&id).ok().map(|uid| (uid, score))
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored)
    }

    pub async fn count(&self) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM embeddings")
            .fetch_one(&self.pool).await?;
        Ok(n)
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() { return 0.0; }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 { 0.0 } else { dot / (norm_a * norm_b) }
}

fn floats_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn bytes_to_floats(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}
