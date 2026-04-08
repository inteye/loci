use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: Uuid,
    pub kind: NodeKind,
    pub name: String,
    pub file_path: Option<String>,
    pub description: Option<String>, // LLM-generated summary
    pub raw_source: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    File,
    Module,
    Struct,
    Enum,
    Trait,
    Function,
    Concept,  // LLM-extracted abstract concept
    Commit,   // Git commit as a traceability node
    Decision, // Derived design decision with supporting evidence
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: Uuid,
    pub from: Uuid,
    pub to: Uuid,
    pub kind: EdgeKind,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Contains,    // file contains function
    Calls,       // function calls function
    Implements,  // struct implements trait
    DependsOn,   // module depends on module
    RelatedTo,   // LLM-inferred semantic relation
    ChangedIn,   // symbol/file changed in commit
    ExplainedBy, // node explained by concept/decision
    EvidenceFromFile,
    EvidenceFromCommit,
    EvidenceFromConcept,
    EvidenceFromDecision,
}

/// In-memory graph — persisted via GraphStore
#[derive(Default)]
pub struct KnowledgeGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl KnowledgeGraph {
    pub fn add_node(&mut self, node: Node) -> Uuid {
        let id = node.id;
        self.nodes.push(node);
        id
    }

    pub fn add_edge(&mut self, edge: Edge) {
        self.edges.push(edge);
    }

    pub fn find_node_by_name(&self, name: &str) -> Option<&Node> {
        self.nodes
            .iter()
            .find(|n| n.name.eq_ignore_ascii_case(name))
    }

    pub fn neighbors(&self, node_id: Uuid) -> Vec<(&Edge, &Node)> {
        self.edges
            .iter()
            .filter(|e| e.from == node_id || e.to == node_id)
            .filter_map(|e| {
                let neighbor_id = if e.from == node_id { e.to } else { e.from };
                self.nodes
                    .iter()
                    .find(|n| n.id == neighbor_id)
                    .map(|n| (e, n))
            })
            .collect()
    }

    /// Compact text representation for LLM context injection
    pub fn to_context_str(&self, focus_node: Option<Uuid>) -> String {
        let ids: Option<std::collections::HashSet<Uuid>> = focus_node.map(|id| {
            let mut s = std::collections::HashSet::new();
            s.insert(id);
            for (_, n) in self.neighbors(id) {
                s.insert(n.id);
            }
            s
        });
        self.render_context(ids.as_ref())
    }

    /// Render only nodes in the given id set
    pub fn to_context_str_filtered(&self, ids: &std::collections::HashSet<Uuid>) -> String {
        self.render_context(Some(ids))
    }

    fn render_context(&self, filter: Option<&std::collections::HashSet<Uuid>>) -> String {
        let mut out = String::new();
        let nodes: Vec<&Node> = self
            .nodes
            .iter()
            .filter(|n| filter.map_or(true, |f| f.contains(&n.id)))
            .collect();

        for node in &nodes {
            out.push_str(&format!("[{:?}] {}", node.kind, node.name));
            if let Some(desc) = &node.description {
                out.push_str(&format!(" — {}", desc));
            }
            out.push('\n');
        }

        let node_ids: std::collections::HashSet<Uuid> = nodes.iter().map(|n| n.id).collect();
        out.push_str("\nRelations:\n");
        for edge in &self.edges {
            if node_ids.contains(&edge.from) && node_ids.contains(&edge.to) {
                let from = self
                    .nodes
                    .iter()
                    .find(|n| n.id == edge.from)
                    .map(|n| n.name.as_str())
                    .unwrap_or("?");
                let to = self
                    .nodes
                    .iter()
                    .find(|n| n.id == edge.to)
                    .map(|n| n.name.as_str())
                    .unwrap_or("?");
                out.push_str(&format!("  {} --[{:?}]--> {}\n", from, edge.kind, to));
            }
        }
        out
    }
}
