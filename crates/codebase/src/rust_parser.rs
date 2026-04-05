use std::path::Path;
use serde::{Deserialize, Serialize};
use syn::{visit::Visit, ItemFn, ItemStruct, ItemEnum, ItemTrait, ItemImpl, ItemMod};
use anyhow::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedFile {
    pub path: String,
    pub symbols: Vec<Symbol>,
    pub imports: Vec<String>,
    pub doc_comment: Option<String>,
    /// (caller_name, callee_name) — within this file
    pub calls: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub visibility: Visibility,
    pub doc_comment: Option<String>,
    pub line: usize,
    /// For impl blocks: which type is being implemented
    pub impl_for: Option<String>,
    /// For functions: parameter names
    pub params: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SymbolKind {
    Function,
    AsyncFunction,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Visibility {
    Public,
    Private,
    Crate,
}

pub struct RustParser;

impl RustParser {
    pub fn parse_file(path: &Path) -> Result<ParsedFile> {
        let source = std::fs::read_to_string(path)?;
        let syntax = syn::parse_file(&source)
            .map_err(|e| anyhow::anyhow!("parse error in {:?}: {}", path, e))?;

        let mut visitor = SymbolVisitor::default();
        visitor.visit_file(&syntax);
        let doc_comment = extract_file_doc(&syntax);

        Ok(ParsedFile {
            path: path.to_string_lossy().to_string(),
            symbols: visitor.symbols,
            imports: visitor.imports,
            doc_comment,
            calls: visitor.calls,
        })
    }
}

#[derive(Default)]
struct SymbolVisitor {
    pub symbols: Vec<Symbol>,
    pub imports: Vec<String>,
    pub calls: Vec<(String, String)>,
    /// Stack of current function names being visited
    current_fn: Vec<String>,
}

impl<'ast> Visit<'ast> for SymbolVisitor {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let is_async = node.sig.asyncness.is_some();
        let params: Vec<String> = node.sig.inputs.iter().filter_map(|arg| {
            if let syn::FnArg::Typed(pat) = arg {
                if let syn::Pat::Ident(ident) = pat.pat.as_ref() {
                    return Some(ident.ident.to_string());
                }
            }
            None
        }).collect();

        let fn_name = node.sig.ident.to_string();
        self.symbols.push(Symbol {
            name: fn_name.clone(),
            kind: if is_async { SymbolKind::AsyncFunction } else { SymbolKind::Function },
            visibility: vis_to_enum(&node.vis),
            doc_comment: extract_doc_attrs(&node.attrs),
            line: 0,
            impl_for: None,
            params,
        });
        self.current_fn.push(fn_name);
        syn::visit::visit_item_fn(self, node);
        self.current_fn.pop();
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let Some(caller) = self.current_fn.last().cloned() {
            if let syn::Expr::Path(p) = node.func.as_ref() {
                if let Some(seg) = p.path.segments.last() {
                    self.calls.push((caller, seg.ident.to_string()));
                }
            }
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if let Some(caller) = self.current_fn.last().cloned() {
            self.calls.push((caller, node.method.to_string()));
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_item_struct(&mut self, node: &'ast ItemStruct) {
        self.symbols.push(Symbol {
            name: node.ident.to_string(),
            kind: SymbolKind::Struct,
            visibility: vis_to_enum(&node.vis),
            doc_comment: extract_doc_attrs(&node.attrs),
            line: 0,
            impl_for: None,
            params: vec![],
        });
    }

    fn visit_item_enum(&mut self, node: &'ast ItemEnum) {
        self.symbols.push(Symbol {
            name: node.ident.to_string(),
            kind: SymbolKind::Enum,
            visibility: vis_to_enum(&node.vis),
            doc_comment: extract_doc_attrs(&node.attrs),
            line: 0,
            impl_for: None,
            params: vec![],
        });
    }

    fn visit_item_trait(&mut self, node: &'ast ItemTrait) {
        self.symbols.push(Symbol {
            name: node.ident.to_string(),
            kind: SymbolKind::Trait,
            visibility: vis_to_enum(&node.vis),
            doc_comment: extract_doc_attrs(&node.attrs),
            line: 0,
            impl_for: None,
            params: vec![],
        });
    }

    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        let type_name = type_to_string(&node.self_ty);
        let trait_name = node.trait_.as_ref().map(|(_, path, _)| {
            path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default()
        });
        let name = match &trait_name {
            Some(t) => format!("impl {} for {}", t, type_name),
            None => format!("impl {}", type_name),
        };
        self.symbols.push(Symbol {
            name,
            kind: SymbolKind::Impl,
            visibility: Visibility::Private,
            doc_comment: None,
            line: 0,
            impl_for: Some(type_name),
            params: vec![],
        });
        syn::visit::visit_item_impl(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        self.symbols.push(Symbol {
            name: node.ident.to_string(),
            kind: SymbolKind::Module,
            visibility: vis_to_enum(&node.vis),
            doc_comment: extract_doc_attrs(&node.attrs),
            line: 0,
            impl_for: None,
            params: vec![],
        });
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        self.imports.push(use_tree_to_string(&node.tree));
    }
}

fn vis_to_enum(vis: &syn::Visibility) -> Visibility {
    match vis {
        syn::Visibility::Public(_) => Visibility::Public,
        syn::Visibility::Restricted(r) => {
            if r.path.is_ident("crate") { Visibility::Crate } else { Visibility::Private }
        }
        syn::Visibility::Inherited => Visibility::Private,
    }
}

fn extract_doc_attrs(attrs: &[syn::Attribute]) -> Option<String> {
    let docs: Vec<String> = attrs.iter().filter_map(|attr| {
        if attr.path().is_ident("doc") {
            if let syn::Meta::NameValue(nv) = &attr.meta {
                if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value {
                    return Some(s.value().trim().to_string());
                }
            }
        }
        None
    }).collect();
    if docs.is_empty() { None } else { Some(docs.join("\n")) }
}

fn extract_file_doc(file: &syn::File) -> Option<String> {
    extract_doc_attrs(&file.attrs)
}

fn type_to_string(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(p) => p.path.segments.last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => "Unknown".to_string(),
    }
}

fn use_tree_to_string(tree: &syn::UseTree) -> String {
    match tree {
        syn::UseTree::Path(p) => format!("{}::{}", p.ident, use_tree_to_string(&p.tree)),
        syn::UseTree::Name(n) => n.ident.to_string(),
        syn::UseTree::Glob(_) => "*".to_string(),
        syn::UseTree::Group(g) => {
            let items: Vec<_> = g.items.iter().map(use_tree_to_string).collect();
            format!("{{{}}}", items.join(", "))
        }
        syn::UseTree::Rename(r) => format!("{} as {}", r.ident, r.rename),
    }
}
