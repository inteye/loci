/// Tree-sitter based parser for Python, TypeScript, Go, and Java.
/// Extracts the same Symbol/ParsedFile types as the Rust parser.
use std::path::Path;
use tree_sitter::{Parser, Node};
use crate::rust_parser::{ParsedFile, Symbol, SymbolKind, Visibility};
use anyhow::Result;

pub struct TsParser;

impl TsParser {
    pub fn parse_file(path: &Path) -> Result<ParsedFile> {
        let source = std::fs::read_to_string(path)?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        let mut parser = Parser::new();
        let language = match ext {
            "py" => tree_sitter_python::LANGUAGE.into(),
            "ts" | "tsx" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "js" | "jsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
            "go" => tree_sitter_go::LANGUAGE.into(),
            "java" => tree_sitter_java::LANGUAGE.into(),
            _ => return Err(anyhow::anyhow!("unsupported extension: {}", ext)),
        };
        parser.set_language(&language)?;

        let tree = parser.parse(&source, None)
            .ok_or_else(|| anyhow::anyhow!("parse failed"))?;

        let symbols = match ext {
            "py" => extract_python(&tree.root_node(), source.as_bytes()),
            "go" => extract_go(&tree.root_node(), source.as_bytes()),
            "java" => extract_java(&tree.root_node(), source.as_bytes()),
            _ => extract_typescript(&tree.root_node(), source.as_bytes()),
        };

        Ok(ParsedFile {
            path: path.to_string_lossy().to_string(),
            symbols,
            imports: vec![],
            doc_comment: None,
            calls: vec![],
        })
    }
}

fn node_text<'a>(node: &Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

fn extract_python(root: &Node, src: &[u8]) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let mut cursor = root.walk();

    for node in root.children(&mut cursor) {
        match node.kind() {
            "function_definition" | "decorated_definition" => {
                let target = if node.kind() == "decorated_definition" {
                    node.child_by_field_name("definition").unwrap_or(node)
                } else { node };
                if let Some(name_node) = target.child_by_field_name("name") {
                    let name = node_text(&name_node, src).to_string();
                    let is_async = target.child(0).map(|c| c.kind() == "async").unwrap_or(false);
                    symbols.push(Symbol {
                        name,
                        kind: if is_async { SymbolKind::AsyncFunction } else { SymbolKind::Function },
                        visibility: Visibility::Public,
                        doc_comment: extract_python_docstring(&target, src),
                        line: target.start_position().row + 1,
                        impl_for: None,
                        params: vec![],
                    });
                }
            }
            "class_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(Symbol {
                        name: node_text(&name_node, src).to_string(),
                        kind: SymbolKind::Struct,
                        visibility: Visibility::Public,
                        doc_comment: extract_python_docstring(&node, src),
                        line: node.start_position().row + 1,
                        impl_for: None,
                        params: vec![],
                    });
                }
            }
            _ => {}
        }
    }
    symbols
}

fn extract_python_docstring(node: &Node, src: &[u8]) -> Option<String> {
    // First statement in body that is an expression_statement containing a string
    let body = node.child_by_field_name("body")?;
    let first = body.child(0)?;
    if first.kind() == "expression_statement" {
        let expr = first.child(0)?;
        if expr.kind() == "string" {
            let raw = node_text(&expr, src);
            return Some(raw.trim_matches(|c| c == '"' || c == '\'').trim().to_string());
        }
    }
    None
}

fn extract_typescript(root: &Node, src: &[u8]) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    walk_ts(root, src, &mut symbols);
    symbols
}

fn walk_ts(node: &Node, src: &[u8], out: &mut Vec<Symbol>) {
    match node.kind() {
        "function_declaration" | "function" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let is_async = node.child(0).map(|c| node_text(&c, src) == "async").unwrap_or(false);
                out.push(Symbol {
                    name: node_text(&name_node, src).to_string(),
                    kind: if is_async { SymbolKind::AsyncFunction } else { SymbolKind::Function },
                    visibility: Visibility::Public,
                    doc_comment: None,
                    line: node.start_position().row + 1,
                    impl_for: None,
                    params: vec![],
                });
            }
        }
        "class_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                out.push(Symbol {
                    name: node_text(&name_node, src).to_string(),
                    kind: SymbolKind::Struct,
                    visibility: Visibility::Public,
                    doc_comment: None,
                    line: node.start_position().row + 1,
                    impl_for: None,
                    params: vec![],
                });
            }
        }
        "interface_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                out.push(Symbol {
                    name: node_text(&name_node, src).to_string(),
                    kind: SymbolKind::Trait,
                    visibility: Visibility::Public,
                    doc_comment: None,
                    line: node.start_position().row + 1,
                    impl_for: None,
                    params: vec![],
                });
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_ts(&child, src, out);
    }
}

fn extract_go(root: &Node, src: &[u8]) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let mut cursor = root.walk();
    for node in root.children(&mut cursor) {
        match node.kind() {
            "function_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(Symbol {
                        name: node_text(&name_node, src).to_string(),
                        kind: SymbolKind::Function,
                        visibility: Visibility::Public,
                        doc_comment: None,
                        line: node.start_position().row + 1,
                        impl_for: None,
                        params: vec![],
                    });
                }
            }
            "method_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let receiver = node.child_by_field_name("receiver")
                        .and_then(|r| r.child_by_field_name("type"))
                        .map(|t| node_text(&t, src).trim_start_matches('*').to_string());
                    symbols.push(Symbol {
                        name: node_text(&name_node, src).to_string(),
                        kind: SymbolKind::Function,
                        visibility: Visibility::Public,
                        doc_comment: None,
                        line: node.start_position().row + 1,
                        impl_for: receiver,
                        params: vec![],
                    });
                }
            }
            "type_declaration" => {
                // type Foo struct { ... } or type Bar interface { ... }
                let mut c = node.walk();
                for spec in node.children(&mut c) {
                    if spec.kind() == "type_spec" {
                        if let Some(name_node) = spec.child_by_field_name("name") {
                            let type_node = spec.child_by_field_name("type");
                            let kind = match type_node.map(|t| t.kind()) {
                                Some("interface_type") => SymbolKind::Trait,
                                _ => SymbolKind::Struct,
                            };
                            symbols.push(Symbol {
                                name: node_text(&name_node, src).to_string(),
                                kind,
                                visibility: Visibility::Public,
                                doc_comment: None,
                                line: spec.start_position().row + 1,
                                impl_for: None,
                                params: vec![],
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    symbols
}

fn extract_java(root: &Node, src: &[u8]) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    walk_java(root, src, &mut symbols);
    symbols
}

fn walk_java(node: &Node, src: &[u8], out: &mut Vec<Symbol>) {
    match node.kind() {
        "class_declaration" | "record_declaration" | "enum_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                out.push(Symbol {
                    name: node_text(&name_node, src).to_string(),
                    kind: SymbolKind::Struct,
                    visibility: Visibility::Public,
                    doc_comment: None,
                    line: node.start_position().row + 1,
                    impl_for: None,
                    params: vec![],
                });
            }
        }
        "interface_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                out.push(Symbol {
                    name: node_text(&name_node, src).to_string(),
                    kind: SymbolKind::Trait,
                    visibility: Visibility::Public,
                    doc_comment: None,
                    line: node.start_position().row + 1,
                    impl_for: None,
                    params: vec![],
                });
            }
        }
        "method_declaration" | "constructor_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                out.push(Symbol {
                    name: node_text(&name_node, src).to_string(),
                    kind: SymbolKind::Function,
                    visibility: Visibility::Public,
                    doc_comment: None,
                    line: node.start_position().row + 1,
                    impl_for: None,
                    params: vec![],
                });
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_java(&child, src, out);
    }
}
