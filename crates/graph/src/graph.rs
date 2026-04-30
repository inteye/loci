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
    pub fn node_by_id(&self, id: Uuid) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

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

    pub fn find_file_node(&self, target: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| {
            node.kind == NodeKind::File
                && (node.name == target || node.file_path.as_deref() == Some(target))
        })
    }

    pub fn nodes_in_file(&self, file_path: &str) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|node| node.file_path.as_deref() == Some(file_path))
            .collect()
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

    pub fn expand_ids_with_neighbors(
        &self,
        seeds: &std::collections::HashSet<Uuid>,
        depth: usize,
    ) -> std::collections::HashSet<Uuid> {
        let mut expanded = seeds.clone();
        let mut frontier = seeds.clone();

        for _ in 0..depth {
            let mut next = std::collections::HashSet::new();
            for id in &frontier {
                for (_, node) in self.neighbors(*id) {
                    if expanded.insert(node.id) {
                        next.insert(node.id);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }

        expanded
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_node(kind: NodeKind, name: &str, file_path: Option<&str>) -> Node {
        Node {
            id: Uuid::new_v4(),
            kind,
            name: name.to_string(),
            file_path: file_path.map(|value| value.to_string()),
            description: None,
            raw_source: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn finds_file_nodes_by_name_or_path() {
        let mut graph = KnowledgeGraph::default();
        let file = make_node(NodeKind::File, "main.rs", Some("src/main.rs"));
        let file_id = file.id;
        graph.add_node(file);
        graph.add_node(make_node(NodeKind::Function, "run", Some("src/main.rs")));

        assert_eq!(
            graph.find_file_node("main.rs").map(|node| node.id),
            Some(file_id)
        );
        assert_eq!(
            graph.find_file_node("src/main.rs").map(|node| node.id),
            Some(file_id)
        );
        assert_eq!(graph.nodes_in_file("src/main.rs").len(), 2);
    }

    #[test]
    fn expands_neighbors_by_depth() {
        let mut graph = KnowledgeGraph::default();
        let file = make_node(NodeKind::File, "main.rs", Some("src/main.rs"));
        let function = make_node(NodeKind::Function, "run", Some("src/main.rs"));
        let concept = make_node(NodeKind::Concept, "Boot flow", None);

        let file_id = file.id;
        let function_id = function.id;
        let concept_id = concept.id;

        graph.add_node(file);
        graph.add_node(function);
        graph.add_node(concept);
        graph.add_edge(Edge {
            id: Uuid::new_v4(),
            from: file_id,
            to: function_id,
            kind: EdgeKind::Contains,
            label: None,
        });
        graph.add_edge(Edge {
            id: Uuid::new_v4(),
            from: function_id,
            to: concept_id,
            kind: EdgeKind::ExplainedBy,
            label: None,
        });

        let seeds = std::collections::HashSet::from([file_id]);
        let one_hop = graph.expand_ids_with_neighbors(&seeds, 1);
        let two_hop = graph.expand_ids_with_neighbors(&seeds, 2);

        assert!(one_hop.contains(&file_id));
        assert!(one_hop.contains(&function_id));
        assert!(!one_hop.contains(&concept_id));

        assert!(two_hop.contains(&concept_id));
    }
}
