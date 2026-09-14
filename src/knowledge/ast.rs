//! Tree-sitter declaration extraction for the codebase graph.
//!
//! Replaces line-by-line pattern matching with a real parse. The hand-written
//! [`super::graph_rag::parse_declaration`] recognised the declaration forms people
//! commonly write, but it is still a matcher over single lines: a signature split
//! across lines, a `fn` inside a macro body, or a declaration whose modifiers arrive in
//! an order nobody thought of are all invisible to it, and it cannot tell code from a
//! string literal containing code.
//!
//! A parse has none of those failure modes — it knows what a function *is* — so this is
//! the primary path and the matcher is the fallback for languages with no grammar
//! compiled in (C, Ruby, Java, PHP, shell, …).
//!
//! Grammars are pinned exactly in Cargo.toml: tree-sitter node kind names are part of a
//! grammar's API and change between versions, so a caret bump could silently stop
//! matching and quietly shrink the graph.

use tree_sitter::{Node, Parser};

use super::graph_rag::DeclKind;

/// Languages with a grammar compiled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lang {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
}

impl Lang {
    /// Pick a grammar from the file extension, or `None` to fall back to the matcher.
    pub(crate) fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "rs" => Some(Lang::Rust),
            "py" | "pyi" => Some(Lang::Python),
            "js" | "mjs" | "cjs" | "jsx" => Some(Lang::JavaScript),
            "ts" | "mts" | "cts" => Some(Lang::TypeScript),
            "tsx" => Some(Lang::Tsx),
            "go" => Some(Lang::Go),
            _ => None,
        }
    }

    fn language(self) -> tree_sitter::Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
        }
    }

    /// Node kinds that declare a function, and those that declare a type.
    ///
    /// Written per grammar rather than guessed from a shared vocabulary, because the
    /// grammars genuinely disagree: Rust has `function_item`, Go has
    /// `function_declaration` *and* `method_declaration`, and TypeScript reuses
    /// JavaScript's `function_declaration` while adding `interface_declaration`.
    fn kinds(self) -> (&'static [&'static str], &'static [&'static str]) {
        match self {
            Lang::Rust => (
                &["function_item"],
                &[
                    "struct_item",
                    "enum_item",
                    "trait_item",
                    "type_item",
                    "union_item",
                ],
            ),
            Lang::Python => (&["function_definition"], &["class_definition"]),
            Lang::JavaScript => (
                &[
                    "function_declaration",
                    "generator_function_declaration",
                    "method_definition",
                ],
                &["class_declaration"],
            ),
            Lang::TypeScript | Lang::Tsx => (
                &[
                    "function_declaration",
                    "generator_function_declaration",
                    "method_definition",
                ],
                &[
                    "class_declaration",
                    "interface_declaration",
                    "type_alias_declaration",
                    "enum_declaration",
                ],
            ),
            Lang::Go => (
                &["function_declaration", "method_declaration"],
                &["type_declaration"],
            ),
        }
    }
}

/// Parse `source` and return every declaration it contains.
///
/// Returns `None` when the grammar cannot be loaded, so the caller falls back rather
/// than silently indexing nothing.
pub(crate) fn extract(source: &str, lang: Lang) -> Option<Vec<(DeclKind, String)>> {
    let mut parser = Parser::new();
    parser.set_language(&lang.language()).ok()?;
    let tree = parser.parse(source, None)?;

    let (fn_kinds, type_kinds) = lang.kinds();
    let mut out: Vec<(DeclKind, String)> = Vec::new();
    let mut stack = vec![tree.root_node()];

    // Iterative walk: a deeply nested file would otherwise risk the stack, and this
    // runs over whole repositories.
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        if fn_kinds.contains(&kind) {
            if let Some(name) = declaration_name(node, source) {
                out.push((DeclKind::Function, name));
            }
        } else if type_kinds.contains(&kind) {
            if let Some(name) = declaration_name(node, source) {
                out.push((DeclKind::Type, name));
            }
        } else if kind == "lexical_declaration" || kind == "variable_declaration" {
            // `const handler = () => …` is a binding whose value is a function. Only
            // count it when the value really is one, so `const MAX = 10` stays out.
            if let Some(name) = arrow_binding_name(node, source) {
                out.push((DeclKind::Function, name));
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    // The walk pops children in reverse, so restore source order — the graph is
    // persisted and diffed, and a stable order keeps those diffs readable.
    out.reverse();
    Some(out)
}

/// The `name` field every declaration node carries, as source text.
fn declaration_name(node: Node<'_>, source: &str) -> Option<String> {
    let name = node
        .child_by_field_name("name")
        // Go's `type_declaration` wraps one or more `type_spec` children.
        .or_else(|| {
            node.named_child(0)
                .filter(|c| c.kind() == "type_spec")
                .and_then(|c| c.child_by_field_name("name"))
        })?;
    let text = name.utf8_text(source.as_bytes()).ok()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// `const name = (…) => …` / `let name = async function () {}`.
fn arrow_binding_name(node: Node<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "variable_declarator" {
            continue;
        }
        let value = child.child_by_field_name("value")?;
        if !matches!(
            value.kind(),
            "arrow_function" | "function_expression" | "function"
        ) {
            continue;
        }
        let name = child.child_by_field_name("name")?;
        let text = name.utf8_text(source.as_bytes()).ok()?.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(src: &str, lang: Lang) -> Vec<String> {
        extract(src, lang)
            .expect("grammar must load")
            .into_iter()
            .map(|(_, n)| n)
            .collect()
    }

    #[test]
    fn rust_finds_every_visibility_and_modifier_form() {
        let src = r#"
            pub fn simple() {}
            pub(crate) fn scoped() {}
            pub(in crate::a) fn deep() {}
            pub async unsafe fn complicated() {}
            impl Thing { fn method(&self) {} }
            pub struct Config;
            pub trait Filter {}
            type Alias = u32;
        "#;
        let found = names(src, Lang::Rust);

        for want in [
            "simple",
            "scoped",
            "deep",
            "complicated",
            "method",
            "Config",
            "Filter",
            "Alias",
        ] {
            assert!(
                found.contains(&want.to_string()),
                "missing {want} in {found:?}"
            );
        }
    }

    /// The case a line matcher structurally cannot handle.
    #[test]
    fn rust_finds_a_signature_split_across_lines() {
        let src = "pub fn wrapped(\n    a: usize,\n    b: usize,\n) -> usize { a + b }";

        assert_eq!(names(src, Lang::Rust), vec!["wrapped".to_string()]);
    }

    /// The other one: code inside a string is not code.
    #[test]
    fn a_function_inside_a_string_literal_is_not_a_declaration() {
        let src = r#"pub fn real() { let s = "pub fn fake() {}"; let _ = s; }"#;

        assert_eq!(names(src, Lang::Rust), vec!["real".to_string()]);
    }

    #[test]
    fn a_commented_out_function_is_not_a_declaration() {
        let src = "// pub fn commented() {}\n/* pub fn blocked() {} */\npub fn real() {}";

        assert_eq!(names(src, Lang::Rust), vec!["real".to_string()]);
    }

    #[test]
    fn python_finds_sync_async_and_methods() {
        let src = "def handler():\n    pass\n\nasync def ahandler():\n    pass\n\nclass Service:\n    def method(self):\n        pass\n";
        let found = names(src, Lang::Python);

        for want in ["handler", "ahandler", "Service", "method"] {
            assert!(
                found.contains(&want.to_string()),
                "missing {want} in {found:?}"
            );
        }
    }

    #[test]
    fn go_finds_functions_methods_and_types() {
        let src = "package main\ntype Repo struct{}\nfunc Plain() {}\nfunc (r *Repo) Method() {}\n";
        let found = names(src, Lang::Go);

        for want in ["Plain", "Method", "Repo"] {
            assert!(
                found.contains(&want.to_string()),
                "missing {want} in {found:?}"
            );
        }
    }

    #[test]
    fn typescript_finds_exports_arrows_and_interfaces() {
        let src = "export function exported() {}\nexport default function def() {}\nconst arrow = (a: number) => a;\nexport interface Props { a: number }\nconst MAX = 10;\n";
        let found = names(src, Lang::Tsx);

        for want in ["exported", "def", "arrow", "Props"] {
            assert!(
                found.contains(&want.to_string()),
                "missing {want} in {found:?}"
            );
        }
        assert!(
            !found.contains(&"MAX".to_string()),
            "a plain constant is not a function: {found:?}"
        );
    }

    #[test]
    fn extensions_map_to_the_right_grammar() {
        assert_eq!(Lang::from_extension("rs"), Some(Lang::Rust));
        assert_eq!(Lang::from_extension("tsx"), Some(Lang::Tsx));
        assert_eq!(Lang::from_extension("go"), Some(Lang::Go));
        // No grammar compiled in: the caller falls back to the line matcher.
        assert_eq!(Lang::from_extension("rb"), None);
        assert_eq!(Lang::from_extension("c"), None);
    }

    #[test]
    fn malformed_source_yields_what_it_can_rather_than_failing() {
        let src = "pub fn good() {}\npub fn broken(";

        let found = names(src, Lang::Rust);

        assert!(found.contains(&"good".to_string()), "{found:?}");
    }
}
