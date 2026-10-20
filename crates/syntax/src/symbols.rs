//! Lightweight symbol extraction (functions, types, impls, ...) by walking the syntax tree.
//!
//! This is deliberately heuristic: it aims to give a useful outline of a file (or a fragment of
//! one) for code review, not to be a precise semantic model. Each language has a small
//! classifier that recognises definition nodes; a shared, iterative tree walk tracks the
//! enclosing scopes so methods know their container.

use std::borrow::Cow;
use std::cell::RefCell;
use std::panic::AssertUnwindSafe;

use tree_sitter::{Node, Parser, Tree};

use crate::lang::{closing_suffix, Lang};
use crate::MAX_SOURCE_LEN;

/// What kind of definition a [`Symbol`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Class,
    Enum,
    Trait,
    Interface,
    Impl,
    Module,
    Const,
    TypeAlias,
    Macro,
    Test,
}

/// A definition found in a source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// Display name, e.g. `"apply"`. Rust impls use `"Type"` or `"Trait for Type"`.
    pub name: String,
    pub kind: SymbolKind,
    /// Enclosing impl/class/trait/module name, e.g. `Some("BatchOperation")` for a method.
    pub container: Option<String>,
    /// 1-based, inclusive line of the definition's first line.
    pub start_line: u32,
    /// 1-based, inclusive line of the definition's last line.
    pub end_line: u32,
}

/// Longest symbol/container name we report; anything longer is truncated.
const MAX_NAME_LEN: usize = 160;

/// Returns the definitions in `source`, in source order, nested ones included.
///
/// Rust functions carrying a `#[test]`-like attribute (or living inside `mod tests`) are
/// reported as [`SymbolKind::Test`]; similar conventions are applied for other languages
/// (`test_*` in Python, `TestXxx` in Go, `@Test` in Java/Kotlin, `it(...)`/`test(...)` in JS/TS).
/// Data/markup languages (JSON, TOML, YAML, CSS, HTML, Markdown) yield no symbols; for Bash
/// only function definitions are reported.
///
/// Inputs larger than 2 MB yield no symbols. Safe to call concurrently from any thread.
pub fn symbols(lang: Lang, source: &str) -> Vec<Symbol> {
    if source.len() > MAX_SOURCE_LEN || !has_symbols(lang) {
        return Vec::new();
    }
    std::panic::catch_unwind(AssertUnwindSafe(|| {
        let Some((tree, text)) = parse_with_recovery(lang, source) else {
            return Vec::new();
        };
        let mut out = Walker::new(lang, &text).run(&tree);
        // Recovery may append synthetic closing braces; never report lines past the input.
        let last_line = source.lines().count().max(1) as u32;
        for symbol in &mut out {
            symbol.end_line = symbol.end_line.min(last_line).max(symbol.start_line);
        }
        out
    }))
    .unwrap_or_default()
}

/// Languages for which [`symbols`] does any work.
fn has_symbols(lang: Lang) -> bool {
    !matches!(
        lang,
        Lang::Json | Lang::Toml | Lang::Yaml | Lang::Css | Lang::Html | Lang::Markdown
    )
}

/// Parses `source`, retrying with balanced braces when it looks like a truncated fragment.
///
/// Diff hunks often cut a block open (`fn apply(&mut self) {` without its closing brace). For
/// brace-delimited languages tree-sitter then tends to wrap everything in one flat `ERROR`
/// node and loses the definition. Appending the missing `}` (after the original text, so all
/// existing byte offsets and line numbers stay valid) recovers it.
fn parse_with_recovery(lang: Lang, source: &str) -> Option<(Tree, Cow<'_, str>)> {
    let tree = parse(lang, source)?;
    if !tree.root_node().has_error() {
        return Some((tree, Cow::Borrowed(source)));
    }
    let Some(suffix) = closing_suffix(lang, source) else {
        return Some((tree, Cow::Borrowed(source)));
    };
    let patched = format!("{source}{suffix}");
    match parse(lang, &patched) {
        Some(recovered) => Some((recovered, Cow::Owned(patched))),
        None => Some((tree, Cow::Borrowed(source))),
    }
}

thread_local! {
    /// Per-thread parser; `Parser` is `Send` but not `Sync`, and reusing it avoids reallocating
    /// its internal buffers on every call.
    static PARSER: RefCell<Parser> = RefCell::new(Parser::new());
}

fn parse(lang: Lang, source: &str) -> Option<Tree> {
    let run = |parser: &mut Parser| {
        parser.set_language(&lang.ts_language()).ok()?;
        parser.parse(source, None)
    };
    PARSER.with(|cell| match cell.try_borrow_mut() {
        Ok(mut parser) => run(&mut parser),
        Err(_) => run(&mut Parser::new()),
    })
}

/// A lexical scope that influences how nested definitions are classified.
#[derive(Clone, Debug)]
struct Scope {
    kind: ScopeKind,
    /// Name used as the `container` of nested symbols (`None` for anonymous scopes).
    name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScopeKind {
    /// A type-like container: impl, trait, class, struct body, interface, protocol, ...
    /// Functions directly inside it are methods.
    Type,
    /// A module / namespace / `describe` block.
    Module,
    /// A function body. Functions nested in it are plain functions, not methods.
    Function,
}

/// Result of classifying one node.
#[derive(Default)]
struct Classified {
    symbol: Option<Def>,
    /// Scope opened by this node for its descendants.
    scope: Option<Scope>,
}

struct Def {
    kind: SymbolKind,
    name: String,
    /// Explicit container, overriding the one derived from enclosing scopes (e.g. Go receivers
    /// or C++ `Foo::bar` out-of-line definitions).
    container: Option<String>,
}

impl Classified {
    fn def(kind: SymbolKind, name: String) -> Self {
        Self {
            symbol: Some(Def {
                kind,
                name,
                container: None,
            }),
            scope: None,
        }
    }

    fn with_scope(mut self, kind: ScopeKind, name: Option<String>) -> Self {
        self.scope = Some(Scope { kind, name });
        self
    }

    fn with_container(mut self, container: Option<String>) -> Self {
        if let Some(def) = &mut self.symbol {
            def.container = container;
        }
        self
    }

    fn scope_only(kind: ScopeKind, name: Option<String>) -> Self {
        Self::default().with_scope(kind, name)
    }
}

/// Walks a tree, classifying nodes and tracking scopes.
struct Walker<'s> {
    lang: Lang,
    src: &'s str,
    scopes: Vec<Scope>,
}

impl<'s> Walker<'s> {
    fn new(lang: Lang, src: &'s str) -> Self {
        Self {
            lang,
            src,
            scopes: Vec::new(),
        }
    }

    fn run(mut self, tree: &Tree) -> Vec<Symbol> {
        let mut out = Vec::new();
        // Iterative pre-order traversal (deep trees must not overflow the stack). Each entry
        // carries the scope depth that applies to it.
        let mut stack: Vec<(Node, usize)> = vec![(tree.root_node(), 0)];
        let mut children = Vec::new();
        while let Some((node, depth)) = stack.pop() {
            self.scopes.truncate(depth);

            let classified = self.classify(node);
            if let Some(def) = classified.symbol {
                if !def.name.is_empty() {
                    out.push(Symbol {
                        container: def.container.or_else(|| self.container()),
                        name: def.name,
                        kind: def.kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: end_line(node),
                    });
                }
            }
            if let Some(scope) = classified.scope {
                self.scopes.push(scope);
            }

            let depth = self.scopes.len();
            let mut cursor = node.walk();
            children.clear();
            children.extend(node.named_children(&mut cursor));
            stack.extend(children.drain(..).rev().map(|child| (child, depth)));
        }
        out
    }

    fn classify(&self, node: Node) -> Classified {
        match self.lang {
            Lang::Rust => self.rust(node),
            Lang::Python => self.python(node),
            Lang::JavaScript | Lang::TypeScript | Lang::Tsx => self.js(node),
            Lang::Go => self.go(node),
            Lang::C | Lang::Cpp => self.c(node),
            Lang::Java => self.java(node),
            Lang::Swift => self.swift(node),
            Lang::Kotlin => self.kotlin(node),
            Lang::Bash => self.bash(node),
            Lang::Json | Lang::Toml | Lang::Yaml | Lang::Css | Lang::Html | Lang::Markdown => {
                Classified::default()
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // Scope helpers
    // ---------------------------------------------------------------------------------------

    /// Innermost named type or module scope.
    fn container(&self) -> Option<String> {
        self.scopes
            .iter()
            .rev()
            .filter(|s| s.kind != ScopeKind::Function)
            .find_map(|s| s.name.clone())
    }

    /// Whether the innermost scope is type-like (i.e. a function here is a method).
    fn in_type(&self) -> bool {
        matches!(self.scopes.last(), Some(s) if s.kind == ScopeKind::Type)
    }

    /// Name of the innermost scope if it is type-like.
    fn enclosing_type_name(&self) -> Option<&str> {
        match self.scopes.last() {
            Some(s) if s.kind == ScopeKind::Type => s.name.as_deref(),
            _ => None,
        }
    }

    /// Function or method, depending on the enclosing scope.
    fn fn_kind(&self) -> SymbolKind {
        if self.in_type() {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        }
    }

    // ---------------------------------------------------------------------------------------
    // Text helpers
    // ---------------------------------------------------------------------------------------

    fn text(&self, node: Node) -> &'s str {
        self.src.get(node.byte_range()).unwrap_or("")
    }

    /// Source text of `node` with whitespace runs collapsed, truncated to a sane length.
    fn name_of(&self, node: Node) -> String {
        clean_name(self.text(node))
    }

    /// Cleaned text of the `field` child, or an empty string when absent.
    fn field_name(&self, node: Node, field: &str) -> String {
        node.child_by_field_name(field)
            .map(|n| self.name_of(n))
            .unwrap_or_default()
    }

    // ---------------------------------------------------------------------------------------
    // Rust
    // ---------------------------------------------------------------------------------------

    fn rust(&self, node: Node) -> Classified {
        use SymbolKind as K;
        match node.kind() {
            "function_item" => {
                let name = self.field_name(node, "name");
                let kind = if rust_has_test_attribute(node, self.src)
                    || (!self.in_type() && self.in_rust_test_module())
                {
                    K::Test
                } else {
                    self.fn_kind()
                };
                Classified::def(kind, name).with_scope(ScopeKind::Function, None)
            }
            "function_signature_item" => {
                Classified::def(self.fn_kind(), self.field_name(node, "name"))
            }
            "struct_item" | "union_item" => {
                Classified::def(K::Struct, self.field_name(node, "name"))
            }
            "enum_item" => Classified::def(K::Enum, self.field_name(node, "name")),
            "trait_item" => {
                let name = self.field_name(node, "name");
                Classified::def(K::Trait, name.clone()).with_scope(ScopeKind::Type, Some(name))
            }
            "impl_item" => {
                let ty = node.child_by_field_name("type");
                let ty_name = ty.map(|t| self.name_of(t)).unwrap_or_default();
                let name = match node.child_by_field_name("trait") {
                    Some(tr) => format!("{} for {}", self.name_of(tr), ty_name),
                    None => ty_name,
                };
                // Methods use the bare type name (`Foo` for `impl<T> Trait for Foo<T>`).
                let container = ty.map(|t| self.name_of(rust_base_type(t)));
                Classified::def(K::Impl, name).with_scope(ScopeKind::Type, container)
            }
            "mod_item" => {
                let name = self.field_name(node, "name");
                Classified::def(K::Module, name.clone()).with_scope(ScopeKind::Module, Some(name))
            }
            "const_item" | "static_item" => {
                Classified::def(K::Const, self.field_name(node, "name"))
            }
            "type_item" | "associated_type" => {
                Classified::def(K::TypeAlias, self.field_name(node, "name"))
            }
            "macro_definition" => Classified::def(K::Macro, self.field_name(node, "name")),
            _ => Classified::default(),
        }
    }

    /// Whether any enclosing scope is a `mod tests` / `mod test`.
    fn in_rust_test_module(&self) -> bool {
        self.scopes.iter().any(|s| {
            s.kind == ScopeKind::Module && matches!(s.name.as_deref(), Some("tests" | "test"))
        })
    }

    // ---------------------------------------------------------------------------------------
    // Python
    // ---------------------------------------------------------------------------------------

    fn python(&self, node: Node) -> Classified {
        match node.kind() {
            "function_definition" => {
                let name = self.field_name(node, "name");
                let is_test = (name == "test" || name.starts_with("test_"))
                    && self
                        .enclosing_type_name()
                        .is_none_or(|class| class.starts_with("Test"));
                let kind = if is_test {
                    SymbolKind::Test
                } else {
                    self.fn_kind()
                };
                Classified::def(kind, name).with_scope(ScopeKind::Function, None)
            }
            "class_definition" => {
                let name = self.field_name(node, "name");
                Classified::def(SymbolKind::Class, name.clone())
                    .with_scope(ScopeKind::Type, Some(name))
            }
            // Module-level `UPPER_CASE = ...` constants.
            "assignment" if self.scopes.is_empty() => {
                let is_top_level = node
                    .parent()
                    .filter(|p| p.kind() == "expression_statement")
                    .and_then(|p| p.parent())
                    .is_some_and(|gp| gp.kind() == "module");
                match node.child_by_field_name("left") {
                    Some(left) if is_top_level && left.kind() == "identifier" => {
                        let name = self.text(left);
                        if is_upper_case_ident(name) {
                            Classified::def(SymbolKind::Const, name.to_string())
                        } else {
                            Classified::default()
                        }
                    }
                    _ => Classified::default(),
                }
            }
            _ => Classified::default(),
        }
    }

    // ---------------------------------------------------------------------------------------
    // JavaScript / TypeScript / TSX
    // ---------------------------------------------------------------------------------------

    fn js(&self, node: Node) -> Classified {
        use SymbolKind as K;
        match node.kind() {
            "function_declaration" | "generator_function_declaration" => {
                Classified::def(K::Function, self.field_name(node, "name"))
                    .with_scope(ScopeKind::Function, None)
            }
            "class_declaration" | "abstract_class_declaration" | "class" => {
                let name = self.field_name(node, "name");
                if name.is_empty() {
                    // Anonymous class expression: still a type scope for its methods.
                    return Classified::scope_only(ScopeKind::Type, None);
                }
                Classified::def(K::Class, name.clone()).with_scope(ScopeKind::Type, Some(name))
            }
            "method_definition" => Classified::def(self.fn_kind(), self.field_name(node, "name"))
                .with_scope(ScopeKind::Function, None),
            "method_signature" | "abstract_method_signature" => {
                Classified::def(self.fn_kind(), self.field_name(node, "name"))
            }
            // Class fields initialised with a function: `onClick = () => {}`.
            "public_field_definition" | "field_definition" => {
                let is_fn = node
                    .child_by_field_name("value")
                    .is_some_and(|v| is_js_function_value(v.kind()));
                if !is_fn {
                    return Classified::default();
                }
                let name = node
                    .child_by_field_name("name")
                    .or_else(|| node.child_by_field_name("property"))
                    .map(|n| self.name_of(n))
                    .unwrap_or_default();
                Classified::def(self.fn_kind(), name).with_scope(ScopeKind::Function, None)
            }
            // `const foo = () => {}` / `let bar = function () {}`.
            "variable_declarator" => {
                let is_fn = node
                    .child_by_field_name("value")
                    .is_some_and(|v| is_js_function_value(v.kind()));
                match node.child_by_field_name("name") {
                    Some(name) if is_fn && name.kind() == "identifier" => {
                        Classified::def(K::Function, self.name_of(name))
                            .with_scope(ScopeKind::Function, None)
                    }
                    _ => Classified::default(),
                }
            }
            "interface_declaration" => {
                let name = self.field_name(node, "name");
                Classified::def(K::Interface, name.clone()).with_scope(ScopeKind::Type, Some(name))
            }
            "type_alias_declaration" => {
                Classified::def(K::TypeAlias, self.field_name(node, "name"))
            }
            "enum_declaration" => Classified::def(K::Enum, self.field_name(node, "name")),
            "internal_module" | "module" => {
                let name = self.field_name(node, "name");
                Classified::def(K::Module, name.clone()).with_scope(ScopeKind::Module, Some(name))
            }
            "call_expression" => self.js_test_call(node),
            _ => Classified::default(),
        }
    }

    /// Recognises test-framework calls: `describe("x", ...)` becomes a module-like container,
    /// `it("y", ...)` / `test("y", ...)` a test.
    fn js_test_call(&self, node: Node) -> Classified {
        let Some(callee) = node.child_by_field_name("function") else {
            return Classified::default();
        };
        // Accept `it(...)`, `it.only(...)`, `describe.skip(...)`, `test.each(...)(...)` is ignored.
        let base = match callee.kind() {
            "identifier" => callee,
            "member_expression" => match callee.child_by_field_name("object") {
                Some(obj) if obj.kind() == "identifier" => obj,
                _ => return Classified::default(),
            },
            _ => return Classified::default(),
        };
        let kind = match self.text(base) {
            "describe" | "suite" | "context" => SymbolKind::Module,
            "it" | "test" | "specify" => SymbolKind::Test,
            _ => return Classified::default(),
        };
        let title = node
            .child_by_field_name("arguments")
            .and_then(|args| args.named_child(0))
            .filter(|arg| matches!(arg.kind(), "string" | "template_string"))
            .map(|arg| clean_name(strip_quotes(self.text(arg))));
        let Some(title) = title else {
            return Classified::default();
        };
        let classified = Classified::def(kind, title.clone());
        if kind == SymbolKind::Module {
            classified.with_scope(ScopeKind::Module, Some(title))
        } else {
            classified.with_scope(ScopeKind::Function, None)
        }
    }

    // ---------------------------------------------------------------------------------------
    // Go
    // ---------------------------------------------------------------------------------------

    fn go(&self, node: Node) -> Classified {
        use SymbolKind as K;
        match node.kind() {
            "function_declaration" => {
                let name = self.field_name(node, "name");
                let kind = if is_go_test_name(&name) {
                    K::Test
                } else {
                    K::Function
                };
                Classified::def(kind, name).with_scope(ScopeKind::Function, None)
            }
            "method_declaration" => {
                let receiver = node
                    .child_by_field_name("receiver")
                    .and_then(|r| first_named_of_kind(r, "parameter_declaration"))
                    .and_then(|p| p.child_by_field_name("type"))
                    .map(|t| self.name_of(go_base_type(t)));
                Classified::def(K::Method, self.field_name(node, "name"))
                    .with_container(receiver)
                    .with_scope(ScopeKind::Function, None)
            }
            "type_spec" => {
                let kind = match node.child_by_field_name("type").map(|t| t.kind()) {
                    Some("struct_type") => K::Struct,
                    Some("interface_type") => K::Interface,
                    _ => K::TypeAlias,
                };
                Classified::def(kind, self.field_name(node, "name"))
            }
            "type_alias" => Classified::def(K::TypeAlias, self.field_name(node, "name")),
            "const_spec" => Classified::def(K::Const, self.field_name(node, "name")),
            _ => Classified::default(),
        }
    }

    // ---------------------------------------------------------------------------------------
    // C / C++
    // ---------------------------------------------------------------------------------------

    fn c(&self, node: Node) -> Classified {
        use SymbolKind as K;
        match node.kind() {
            "function_definition" => {
                let Some(target) = node
                    .child_by_field_name("declarator")
                    .and_then(c_function_name_node)
                else {
                    return Classified::scope_only(ScopeKind::Function, None);
                };
                self.c_function(target)
                    .with_scope(ScopeKind::Function, None)
            }
            // Method prototypes inside a class/struct body: `void m();`.
            "field_declaration" | "declaration" if self.in_type() => {
                match node
                    .child_by_field_name("declarator")
                    .filter(|d| c_is_function_declarator(*d))
                    .and_then(c_function_name_node)
                {
                    Some(target) => self.c_function(target),
                    None => Classified::default(),
                }
            }
            "struct_specifier" | "union_specifier" | "class_specifier" | "enum_specifier" => {
                // Only definitions (with a body) count; `struct foo *p;` is just a use.
                if node.child_by_field_name("body").is_none() {
                    return Classified::default();
                }
                let name = self.field_name(node, "name");
                let kind = match node.kind() {
                    "class_specifier" => K::Class,
                    "enum_specifier" => K::Enum,
                    _ => K::Struct,
                };
                if name.is_empty() {
                    return Classified::scope_only(ScopeKind::Type, None);
                }
                let scope = (kind != K::Enum).then_some(ScopeKind::Type);
                let classified = Classified::def(kind, name.clone());
                match scope {
                    Some(scope) => classified.with_scope(scope, Some(name)),
                    None => classified,
                }
            }
            "namespace_definition" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| self.name_of(n))
                    .unwrap_or_else(|| "(anonymous)".to_string());
                Classified::def(K::Module, name.clone()).with_scope(ScopeKind::Module, Some(name))
            }
            "type_definition" => {
                let name = node
                    .child_by_field_name("declarator")
                    .and_then(c_declarator_identifier)
                    .map(|n| self.name_of(n))
                    .unwrap_or_default();
                Classified::def(K::TypeAlias, name)
            }
            "alias_declaration" => Classified::def(K::TypeAlias, self.field_name(node, "name")),
            "preproc_def" | "preproc_function_def" => {
                Classified::def(K::Macro, self.field_name(node, "name"))
            }
            _ => Classified::default(),
        }
    }

    /// Builds the symbol for a C/C++ function whose name node is `target`.
    fn c_function(&self, target: Node) -> Classified {
        if target.kind() == "qualified_identifier" {
            // Out-of-line definition `Foo::bar` (possibly `ns::Foo::bar`): the container is
            // everything before the last `::`.
            let mut name_node = target;
            while let Some(inner) = name_node.child_by_field_name("name") {
                name_node = inner;
                if name_node.kind() != "qualified_identifier" {
                    break;
                }
            }
            let container = self
                .name_of(target)
                .rsplit_once("::")
                .map(|(scope, _)| scope.to_string());
            return Classified::def(SymbolKind::Method, self.name_of(name_node))
                .with_container(container);
        }
        Classified::def(self.fn_kind(), self.name_of(target))
    }

    // ---------------------------------------------------------------------------------------
    // Java
    // ---------------------------------------------------------------------------------------

    fn java(&self, node: Node) -> Classified {
        use SymbolKind as K;
        let type_kind = match node.kind() {
            "class_declaration" | "record_declaration" => Some(K::Class),
            "interface_declaration" | "annotation_type_declaration" => Some(K::Interface),
            "enum_declaration" => Some(K::Enum),
            _ => None,
        };
        if let Some(kind) = type_kind {
            let name = self.field_name(node, "name");
            return Classified::def(kind, name.clone()).with_scope(ScopeKind::Type, Some(name));
        }
        match node.kind() {
            "method_declaration" => {
                let is_test = first_named_of_kind(node, "modifiers").is_some_and(|mods| {
                    named_children(mods).any(|m| {
                        matches!(m.kind(), "marker_annotation" | "annotation")
                            && m.child_by_field_name("name").is_some_and(|n| {
                                matches!(
                                    self.text(n),
                                    "Test" | "ParameterizedTest" | "RepeatedTest" | "TestFactory"
                                )
                            })
                    })
                });
                let kind = if is_test { K::Test } else { self.fn_kind() };
                Classified::def(kind, self.field_name(node, "name"))
                    .with_scope(ScopeKind::Function, None)
            }
            "constructor_declaration" | "compact_constructor_declaration" => {
                Classified::def(K::Method, self.field_name(node, "name"))
                    .with_scope(ScopeKind::Function, None)
            }
            _ => Classified::default(),
        }
    }

    // ---------------------------------------------------------------------------------------
    // Swift
    // ---------------------------------------------------------------------------------------

    fn swift(&self, node: Node) -> Classified {
        use SymbolKind as K;
        match node.kind() {
            "class_declaration" => {
                let kind = match node
                    .child_by_field_name("declaration_kind")
                    .map(|k| self.text(k))
                {
                    Some("struct") => K::Struct,
                    Some("enum") => K::Enum,
                    Some("extension") => K::Impl,
                    _ => K::Class,
                };
                let name = self.field_name(node, "name");
                Classified::def(kind, name.clone()).with_scope(ScopeKind::Type, Some(name))
            }
            "protocol_declaration" => {
                let name = self.field_name(node, "name");
                Classified::def(K::Interface, name.clone()).with_scope(ScopeKind::Type, Some(name))
            }
            "function_declaration" => {
                let name = self.field_name(node, "name");
                // XCTest convention: `func testSomething()` inside a test case class.
                let kind = if self.in_type() && name.starts_with("test") {
                    K::Test
                } else {
                    self.fn_kind()
                };
                Classified::def(kind, name).with_scope(ScopeKind::Function, None)
            }
            "protocol_function_declaration" => {
                Classified::def(self.fn_kind(), self.field_name(node, "name"))
            }
            "init_declaration" => {
                Classified::def(K::Method, "init".to_string()).with_scope(ScopeKind::Function, None)
            }
            "deinit_declaration" => Classified::def(K::Method, "deinit".to_string())
                .with_scope(ScopeKind::Function, None),
            "typealias_declaration" => Classified::def(K::TypeAlias, self.field_name(node, "name")),
            _ => Classified::default(),
        }
    }

    // ---------------------------------------------------------------------------------------
    // Kotlin (this grammar exposes few field names, so children are matched by kind)
    // ---------------------------------------------------------------------------------------

    fn kotlin(&self, node: Node) -> Classified {
        use SymbolKind as K;
        match node.kind() {
            "class_declaration" => {
                let name = first_named_of_kind(node, "type_identifier")
                    .map(|n| self.name_of(n))
                    .unwrap_or_default();
                let is_interface = children(node).any(|c| c.kind() == "interface");
                let is_enum = first_named_of_kind(node, "enum_class_body").is_some()
                    || first_named_of_kind(node, "modifiers")
                        .is_some_and(|m| self.text(m).split_whitespace().any(|w| w == "enum"));
                let kind = if is_interface {
                    K::Interface
                } else if is_enum {
                    K::Enum
                } else {
                    K::Class
                };
                Classified::def(kind, name.clone()).with_scope(ScopeKind::Type, Some(name))
            }
            "object_declaration" | "companion_object" => {
                let name = first_named_of_kind(node, "type_identifier")
                    .map(|n| self.name_of(n))
                    .unwrap_or_else(|| "Companion".to_string());
                Classified::def(K::Class, name.clone()).with_scope(ScopeKind::Type, Some(name))
            }
            "function_declaration" => {
                let name = first_named_of_kind(node, "simple_identifier")
                    .map(|n| self.name_of(n))
                    .unwrap_or_default();
                let is_test = first_named_of_kind(node, "modifiers").is_some_and(|mods| {
                    named_children(mods)
                        .filter(|m| m.kind() == "annotation")
                        .any(|a| {
                            let text = self.text(a).trim_start_matches('@');
                            text == "Test" || text.ends_with(".Test")
                        })
                });
                let kind = if is_test { K::Test } else { self.fn_kind() };
                Classified::def(kind, name).with_scope(ScopeKind::Function, None)
            }
            "type_alias" => {
                let name = first_named_of_kind(node, "type_identifier")
                    .map(|n| self.name_of(n))
                    .unwrap_or_default();
                Classified::def(K::TypeAlias, name)
            }
            "property_declaration" => {
                let is_const = first_named_of_kind(node, "modifiers")
                    .is_some_and(|m| self.text(m).split_whitespace().any(|w| w == "const"));
                let name = first_named_of_kind(node, "variable_declaration")
                    .and_then(|v| first_named_of_kind(v, "simple_identifier"));
                match name {
                    Some(name) if is_const => Classified::def(K::Const, self.name_of(name)),
                    _ => Classified::default(),
                }
            }
            _ => Classified::default(),
        }
    }

    // ---------------------------------------------------------------------------------------
    // Bash
    // ---------------------------------------------------------------------------------------

    fn bash(&self, node: Node) -> Classified {
        match node.kind() {
            "function_definition" => {
                Classified::def(SymbolKind::Function, self.field_name(node, "name"))
                    .with_scope(ScopeKind::Function, None)
            }
            _ => Classified::default(),
        }
    }
}

// -------------------------------------------------------------------------------------------
// Free helpers
// -------------------------------------------------------------------------------------------

/// 1-based inclusive last line of `node`. A node ending at column 0 (i.e. right after a
/// newline) really ends on the previous line.
fn end_line(node: Node) -> u32 {
    let start = node.start_position().row;
    let end = node.end_position();
    let row = if end.column == 0 && end.row > start {
        end.row - 1
    } else {
        end.row
    };
    row as u32 + 1
}

/// Collapses whitespace runs into single spaces and truncates overly long names.
fn clean_name(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_NAME_LEN));
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.len() > MAX_NAME_LEN {
            let mut cut = MAX_NAME_LEN;
            while !out.is_char_boundary(cut) {
                cut -= 1;
            }
            out.truncate(cut);
            out.push('…');
            break;
        }
    }
    out
}

fn children<'t>(node: Node<'t>) -> impl Iterator<Item = Node<'t>> {
    (0..node.child_count() as u32).filter_map(move |i| node.child(i as _))
}

fn named_children<'t>(node: Node<'t>) -> impl Iterator<Item = Node<'t>> {
    (0..node.named_child_count() as u32).filter_map(move |i| node.named_child(i as _))
}

fn first_named_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    named_children(node).find(|c| c.kind() == kind)
}

/// Whether a Rust item is preceded by a test attribute such as `#[test]`, `#[tokio::test]`,
/// `#[rstest]` or `#[test_case(..)]`.
fn rust_has_test_attribute(item: Node, src: &str) -> bool {
    let mut sibling = item.prev_named_sibling();
    while let Some(node) = sibling {
        match node.kind() {
            "attribute_item" => {
                let path = node
                    .named_child(0)
                    .filter(|attr| attr.kind() == "attribute")
                    .and_then(|attr| attr.named_child(0))
                    .and_then(|path| src.get(path.byte_range()))
                    .unwrap_or("");
                let last = path.rsplit("::").next().unwrap_or(path);
                if matches!(
                    last,
                    "test" | "rstest" | "test_case" | "test_log" | "quickcheck"
                ) {
                    return true;
                }
            }
            // Doc comments and regular comments may sit between attributes and the item.
            "line_comment" | "block_comment" => {}
            _ => return false,
        }
        sibling = node.prev_named_sibling();
    }
    false
}

/// `Foo<T>` -> `Foo`, `&mut Foo` -> `Foo`, `crate::a::Foo` stays as is.
fn rust_base_type<'t>(mut ty: Node<'t>) -> Node<'t> {
    loop {
        let next = match ty.kind() {
            "generic_type" => ty.child_by_field_name("type"),
            "reference_type" | "pointer_type" => ty.child_by_field_name("type"),
            _ => None,
        };
        match next {
            Some(n) => ty = n,
            None => return ty,
        }
    }
}

/// `*Foo` -> `Foo`, `Foo[T]` -> `Foo`.
fn go_base_type<'t>(mut ty: Node<'t>) -> Node<'t> {
    loop {
        let next = match ty.kind() {
            "pointer_type" => ty.named_child(0),
            "generic_type" => ty.child_by_field_name("type"),
            _ => None,
        };
        match next {
            Some(n) => ty = n,
            None => return ty,
        }
    }
}

/// Go's `go test` conventions: `TestXxx`, `BenchmarkXxx`, `FuzzXxx`, `ExampleXxx`.
fn is_go_test_name(name: &str) -> bool {
    ["Test", "Benchmark", "Fuzz", "Example"]
        .iter()
        .any(|prefix| {
            name.strip_prefix(prefix).is_some_and(|rest| {
                rest.chars()
                    .next()
                    .is_none_or(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit())
            })
        })
}

fn is_upper_case_ident(name: &str) -> bool {
    name.chars().any(|c| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn is_js_function_value(kind: &str) -> bool {
    matches!(
        kind,
        "arrow_function" | "function_expression" | "function" | "generator_function"
    )
}

/// Removes one layer of matching quotes/backticks from a string literal.
fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    for q in ['"', '\'', '`'] {
        if let Some(inner) = s.strip_prefix(q).and_then(|r| r.strip_suffix(q)) {
            return inner;
        }
    }
    s
}

/// Whether a C/C++ declarator declares a function (possibly behind pointers/references).
fn c_is_function_declarator(mut decl: Node) -> bool {
    loop {
        match decl.kind() {
            "function_declarator" => return true,
            "pointer_declarator" | "reference_declarator" | "attributed_declarator" => {
                match decl
                    .child_by_field_name("declarator")
                    .or_else(|| decl.named_child(0))
                {
                    Some(inner) => decl = inner,
                    None => return false,
                }
            }
            _ => return false,
        }
    }
}

/// Finds the name node of a function declarator chain, e.g. the `identifier` in
/// `int *(*foo)(void)` or the `qualified_identifier` in `void Foo::bar()`.
fn c_function_name_node<'t>(mut decl: Node<'t>) -> Option<Node<'t>> {
    for _ in 0..32 {
        match decl.kind() {
            "function_declarator"
            | "pointer_declarator"
            | "reference_declarator"
            | "parenthesized_declarator"
            | "attributed_declarator" => {
                decl = decl
                    .child_by_field_name("declarator")
                    .or_else(|| decl.named_child(0))?;
            }
            "identifier"
            | "field_identifier"
            | "qualified_identifier"
            | "destructor_name"
            | "operator_name"
            | "template_function"
            | "operator_cast" => return Some(decl),
            _ => return None,
        }
    }
    None
}

/// Finds the declared identifier in a `typedef` declarator (`typedef int (*cb)(void)` -> `cb`).
fn c_declarator_identifier<'t>(mut decl: Node<'t>) -> Option<Node<'t>> {
    for _ in 0..32 {
        match decl.kind() {
            "type_identifier" | "identifier" | "primitive_type" => return Some(decl),
            _ => {
                decl = decl
                    .child_by_field_name("declarator")
                    .or_else(|| decl.named_child(0))?;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use SymbolKind as K;

    /// Compact `(name, kind, container)` view for assertions.
    fn summary(lang: Lang, src: &str) -> Vec<(String, SymbolKind, Option<String>)> {
        symbols(lang, src)
            .into_iter()
            .map(|s| (s.name, s.kind, s.container))
            .collect()
    }

    fn entry(
        name: &str,
        kind: SymbolKind,
        container: Option<&str>,
    ) -> (String, SymbolKind, Option<String>) {
        (name.to_string(), kind, container.map(str::to_string))
    }

    const RUST: &str = r#"use std::fmt;

/// A thing.
pub struct Foo {
    x: i32,
}

impl Foo {
    pub fn bar() -> Self {
        Foo { x: 1 }
    }
}

impl fmt::Display for Foo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.x)
    }
}

pub trait Shape {
    const SIDES: u32;
    fn area(&self) -> f64;
}

enum Color { Red, Green }

const MAX: usize = 10;
type Alias = Vec<u8>;

macro_rules! square {
    ($x:expr) => { $x * $x };
}

#[test]
fn it_works() {
    assert_eq!(2 + 2, 4);
}

mod tests {
    use super::*;

    fn helper() {}

    #[tokio::test]
    async fn async_works() {}
}
"#;

    #[test]
    fn rust_symbols() {
        let got = summary(Lang::Rust, RUST);
        let expected = vec![
            entry("Foo", K::Struct, None),
            entry("Foo", K::Impl, None),
            entry("bar", K::Method, Some("Foo")),
            entry("fmt::Display for Foo", K::Impl, None),
            entry("fmt", K::Method, Some("Foo")),
            entry("Shape", K::Trait, None),
            entry("SIDES", K::Const, Some("Shape")),
            entry("area", K::Method, Some("Shape")),
            entry("Color", K::Enum, None),
            entry("MAX", K::Const, None),
            entry("Alias", K::TypeAlias, None),
            entry("square", K::Macro, None),
            entry("it_works", K::Test, None),
            entry("tests", K::Module, None),
            entry("helper", K::Test, Some("tests")),
            entry("async_works", K::Test, Some("tests")),
        ];
        assert_eq!(got, expected);
    }

    #[test]
    fn rust_line_numbers() {
        let syms = symbols(Lang::Rust, RUST);
        let foo = &syms[0];
        assert_eq!((foo.start_line, foo.end_line), (4, 6));
        let bar = syms.iter().find(|s| s.name == "bar").unwrap();
        assert_eq!((bar.start_line, bar.end_line), (9, 11));
        let impl_foo = &syms[1];
        assert_eq!((impl_foo.start_line, impl_foo.end_line), (8, 12));
    }

    #[test]
    fn rust_generic_impl_container() {
        let src = "impl<T: Clone> Iterator for Wrapper<T> {\n    fn next(&mut self) -> Option<T> { None }\n}\n";
        assert_eq!(
            summary(Lang::Rust, src),
            vec![
                entry("Iterator for Wrapper<T>", K::Impl, None),
                entry("next", K::Method, Some("Wrapper")),
            ]
        );
    }

    #[test]
    fn rust_fragment_does_not_panic() {
        // A diff hunk starting mid-impl and ending mid-function.
        let src = "        let x = 1;\n    }\n\n    pub fn apply(&mut self) {\n        if x {\n";
        let syms = symbols(Lang::Rust, src);
        assert!(syms.iter().any(|s| s.name == "apply"), "{syms:?}");
        for lang in Lang::ALL {
            let _ = symbols(lang, src);
            let _ = symbols(lang, "}}}{{{ (((");
            let _ = symbols(lang, "");
        }
    }

    #[test]
    fn python_symbols() {
        let src = "MAX_SIZE = 10\n\nclass Foo(Base):\n    def __init__(self):\n        pass\n\n    @property\n    def bar(self):\n        def inner():\n            pass\n        return 1\n\ndef top():\n    pass\n\ndef test_top():\n    assert top() is None\n\nclass TestFoo:\n    def test_bar(self):\n        pass\n";
        assert_eq!(
            summary(Lang::Python, src),
            vec![
                entry("MAX_SIZE", K::Const, None),
                entry("Foo", K::Class, None),
                entry("__init__", K::Method, Some("Foo")),
                entry("bar", K::Method, Some("Foo")),
                entry("inner", K::Function, Some("Foo")),
                entry("top", K::Function, None),
                entry("test_top", K::Test, None),
                entry("TestFoo", K::Class, None),
                entry("test_bar", K::Test, Some("TestFoo")),
            ]
        );
    }

    #[test]
    fn typescript_symbols() {
        let src = "export class Store<T> {\n  get(key: string): T { return this.m[key]; }\n  onChange = () => {};\n}\n\nexport interface Props {\n  run(): void;\n}\n\nexport type Id = string;\nenum Mode { A, B }\nnamespace Util {\n  export function helper() {}\n}\nexport const render = (p: Props) => null;\nfunction plain() {}\n\ndescribe('Store', () => {\n  it('gets values', () => {});\n});\n";
        assert_eq!(
            summary(Lang::TypeScript, src),
            vec![
                entry("Store", K::Class, None),
                entry("get", K::Method, Some("Store")),
                entry("onChange", K::Method, Some("Store")),
                entry("Props", K::Interface, None),
                entry("run", K::Method, Some("Props")),
                entry("Id", K::TypeAlias, None),
                entry("Mode", K::Enum, None),
                entry("Util", K::Module, None),
                entry("helper", K::Function, Some("Util")),
                entry("render", K::Function, None),
                entry("plain", K::Function, None),
                entry("Store", K::Module, None),
                entry("gets values", K::Test, Some("Store")),
            ]
        );
        // The TSX and JavaScript grammars share the same classifier.
        let js = "class A { m() {} }\nconst f = function () {};\n";
        assert_eq!(
            summary(Lang::JavaScript, js),
            vec![
                entry("A", K::Class, None),
                entry("m", K::Method, Some("A")),
                entry("f", K::Function, None),
            ]
        );
        assert_eq!(summary(Lang::Tsx, js), summary(Lang::JavaScript, js));
    }

    #[test]
    fn go_symbols() {
        let src = "package p\n\ntype Server struct{}\ntype Handler interface{}\ntype ID = string\n\nconst Max = 3\n\nfunc (s *Server) Serve() {}\nfunc New() *Server { return nil }\nfunc TestServe(t *testing.T) {}\n";
        assert_eq!(
            summary(Lang::Go, src),
            vec![
                entry("Server", K::Struct, None),
                entry("Handler", K::Interface, None),
                entry("ID", K::TypeAlias, None),
                entry("Max", K::Const, None),
                entry("Serve", K::Method, Some("Server")),
                entry("New", K::Function, None),
                entry("TestServe", K::Test, None),
            ]
        );
    }

    #[test]
    fn c_and_cpp_symbols() {
        let c = "#define MAX 4\ntypedef struct { int x; } Point;\nstruct node { int v; };\nstatic int *make(void) { return 0; }\n";
        assert_eq!(
            summary(Lang::C, c),
            vec![
                entry("MAX", K::Macro, None),
                entry("Point", K::TypeAlias, None),
                entry("node", K::Struct, None),
                entry("make", K::Function, None),
            ]
        );
        let cpp = "namespace ns {\nclass Widget {\n public:\n  void draw();\n  int size() const { return 0; }\n};\n}\nvoid ns::Widget::draw() {}\nusing Id = int;\nenum class Kind { A };\n";
        assert_eq!(
            summary(Lang::Cpp, cpp),
            vec![
                entry("ns", K::Module, None),
                entry("Widget", K::Class, Some("ns")),
                entry("draw", K::Method, Some("Widget")),
                entry("size", K::Method, Some("Widget")),
                entry("draw", K::Method, Some("ns::Widget")),
                entry("Id", K::TypeAlias, None),
                entry("Kind", K::Enum, None),
            ]
        );
    }

    #[test]
    fn java_swift_kotlin_symbols() {
        let java = "class Calc {\n  Calc() {}\n  int add(int a) { return a; }\n  @Test void adds() {}\n}\ninterface Op {}\nenum E { A }\n";
        assert_eq!(
            summary(Lang::Java, java),
            vec![
                entry("Calc", K::Class, None),
                entry("Calc", K::Method, Some("Calc")),
                entry("add", K::Method, Some("Calc")),
                entry("adds", K::Test, Some("Calc")),
                entry("Op", K::Interface, None),
                entry("E", K::Enum, None),
            ]
        );

        let swift = "struct Point {\n  func len() -> Int { 0 }\n}\nprotocol Shape { func area() }\nextension Point { init() {} }\nfunc top() {}\n";
        assert_eq!(
            summary(Lang::Swift, swift),
            vec![
                entry("Point", K::Struct, None),
                entry("len", K::Method, Some("Point")),
                entry("Shape", K::Interface, None),
                entry("area", K::Method, Some("Shape")),
                entry("Point", K::Impl, None),
                entry("init", K::Method, Some("Point")),
                entry("top", K::Function, None),
            ]
        );

        let kotlin = "class Repo {\n  fun load(): Int = 1\n}\nenum class Mode { A }\ntypealias Id = String\nfun main() {}\n@Test fun works() {}\n";
        assert_eq!(
            summary(Lang::Kotlin, kotlin),
            vec![
                entry("Repo", K::Class, None),
                entry("load", K::Method, Some("Repo")),
                entry("Mode", K::Enum, None),
                entry("Id", K::TypeAlias, None),
                entry("main", K::Function, None),
                entry("works", K::Test, None),
            ]
        );
    }

    #[test]
    fn data_languages_have_no_symbols() {
        assert!(symbols(Lang::Json, "{\"a\": 1}").is_empty());
        assert!(symbols(Lang::Toml, "[package]\nname = \"x\"\n").is_empty());
    }

    #[test]
    fn oversized_input_is_ignored() {
        let big = "fn a() {}\n".repeat(MAX_SOURCE_LEN / 10 + 1);
        assert!(symbols(Lang::Rust, &big).is_empty());
    }
}
