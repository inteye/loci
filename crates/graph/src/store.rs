use sqlx::{SqlitePool, Row};
use anyhow::Result;
use uuid::Uuid;
use crate::graph::{Node, Edge, NodeKind, EdgeKind, KnowledgeGraph};
use chrono::Utc;

pub struct GraphStore {
    pub pool: SqlitePool,
}

impl GraphStore {
    pub async fn new(db_path: &str) -> Result<Self> {
        let pool = SqlitePool::connect(&format!("sqlite://{}?mode=rwc", db_path)).await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS nodes (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                file_path TEXT,
                description TEXT,
                raw_source TEXT,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS edges (
                id TEXT PRIMARY KEY,
                from_id TEXT NOT NULL,
                to_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                label TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_nodes_name ON nodes(name);
            CREATE INDEX IF NOT EXISTS idx_edges_from ON edges(from_id);
            CREATE INDEX IF NOT EXISTS idx_edges_to ON edges(to_id);"
        ).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn clear(&self) -> Result<()> {
        sqlx::query("DELETE FROM edges").execute(&self.pool).await?;
        sqlx::query("DELETE FROM nodes").execute(&self.pool).await?;
        Ok(())
    }

    pub async fn save_node(&self, node: &Node) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO nodes (id, kind, name, file_path, description, raw_source, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(node.id.to_string())
        .bind(serde_json::to_string(&node.kind)?)
        .bind(&node.name)
        .bind(&node.file_path)
        .bind(&node.description)
        .bind(&node.raw_source)
        .bind(node.created_at.to_rfc3339())
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn save_edge(&self, edge: &Edge) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO edges (id, from_id, to_id, kind, label) VALUES (?, ?, ?, ?, ?)"
        )
        .bind(edge.id.to_string())
        .bind(edge.from.to_string())
        .bind(edge.to.to_string())
        .bind(serde_json::to_string(&edge.kind)?)
        .bind(&edge.label)
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn load_graph(&self) -> Result<KnowledgeGraph> {
        let mut graph = KnowledgeGraph::default();

        let rows = sqlx::query("SELECT id, kind, name, file_path, description, raw_source, created_at FROM nodes")
            .fetch_all(&self.pool).await?;

        for row in rows {
            let kind: NodeKind = serde_json::from_str(row.get::<&str, _>("kind"))?;
            graph.nodes.push(Node {
                id: Uuid::parse_str(row.get("id"))?,
                kind,
                name: row.get("name"),
                file_path: row.get("file_path"),
                description: row.get("description"),
                raw_source: row.get("raw_source"),
                created_at: chrono::DateTime::parse_from_rfc3339(row.get("created_at"))
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            });
        }

        let rows = sqlx::query("SELECT id, from_id, to_id, kind, label FROM edges")
            .fetch_all(&self.pool).await?;

        for row in rows {
            let kind: EdgeKind = serde_json::from_str(row.get::<&str, _>("kind"))?;
            graph.edges.push(Edge {
                id: Uuid::parse_str(row.get("id"))?,
                from: Uuid::parse_str(row.get("from_id"))?,
                to: Uuid::parse_str(row.get("to_id"))?,
                kind,
                label: row.get("label"),
            });
        }

        Ok(graph)
    }

    pub async fn update_node_description(&self, id: Uuid, description: &str) -> Result<()> {
        sqlx::query("UPDATE nodes SET description = ? WHERE id = ?")
            .bind(description)
            .bind(id.to_string())
            .execute(&self.pool).await?;
        Ok(())
    }
}
