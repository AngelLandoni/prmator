//! Supported languages, file-type detection and the grammar/query registry.

use tree_sitter::Language;

/// A language this crate knows how to parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    C,
    Cpp,
    Java,
    Swift,
    Kotlin,
    Json,
    Toml,
    Yaml,
    Bash,
    Css,
    Html,
    Markdown,
}

impl Lang {
    /// Every supported language, in declaration order.
    pub const ALL: [Lang; 18] = [
        Lang::Rust,
        Lang::Python,
        Lang::JavaScript,
        Lang::TypeScript,
        Lang::Tsx,
        Lang::Go,
        Lang::C,
        Lang::Cpp,
        Lang::Java,
        Lang::Swift,
        Lang::Kotlin,
        Lang::Json,
        Lang::Toml,
        Lang::Yaml,
        Lang::Bash,
        Lang::Css,
        Lang::Html,
        Lang::Markdown,
    ];

    /// Human readable name, suitable for display in the UI.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Rust => "Rust",
            Lang::Python => "Python",
            Lang::JavaScript => "JavaScript",
            Lang::TypeScript => "TypeScript",
            Lang::Tsx => "TSX",
            Lang::Go => "Go",
            Lang::C => "C",
            Lang::Cpp => "C++",
            Lang::Java => "Java",
            Lang::Swift => "Swift",
            Lang::Kotlin => "Kotlin",
            Lang::Json => "JSON",
            Lang::Toml => "TOML",
            Lang::Yaml => "YAML",
            Lang::Bash => "Bash",
            Lang::Css => "CSS",
            Lang::Html => "HTML",
            Lang::Markdown => "Markdown",
        }
    }

    /// Resolves a language from a loose name or alias, as found in markdown code-fence info
    /// strings or tree-sitter injection queries (e.g. `"rs"`, `"py"`, `"c++"`, `"shell"`).
    /// Matching is case-insensitive.
    pub fn from_name(name: &str) -> Option<Lang> {
        let name = name.trim().to_ascii_lowercase();
        Some(match name.as_str() {
            "rust" | "rs" => Lang::Rust,
            "python" | "py" | "python3" => Lang::Python,
            "javascript" | "js" | "jsx" | "mjs" | "cjs" | "node" => Lang::JavaScript,
            "typescript" | "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "go" | "golang" => Lang::Go,
            "c" | "h" => Lang::C,
            "cpp" | "c++" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Lang::Cpp,
            "java" => Lang::Java,
            "swift" => Lang::Swift,
            "kotlin" | "kt" | "kts" => Lang::Kotlin,
            "json" | "jsonc" | "json5" => Lang::Json,
            "toml" => Lang::Toml,
            "yaml" | "yml" => Lang::Yaml,
            "bash" | "sh" | "shell" | "zsh" | "console" => Lang::Bash,
            "css" => Lang::Css,
            "html" | "htm" | "xhtml" => Lang::Html,
            "markdown" | "md" => Lang::Markdown,
            _ => return None,
        })
    }

    /// The tree-sitter grammar for this language.
    pub(crate) fn ts_language(self) -> Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::C => tree_sitter_c::LANGUAGE.into(),
            Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Lang::Java => tree_sitter_java::LANGUAGE.into(),
            Lang::Swift => tree_sitter_swift::LANGUAGE.into(),
            Lang::Kotlin => tree_sitter_kotlin_sg::LANGUAGE.into(),
            Lang::Json => tree_sitter_json::LANGUAGE.into(),
            Lang::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Lang::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Lang::Bash => tree_sitter_bash::LANGUAGE.into(),
            Lang::Css => tree_sitter_css::LANGUAGE.into(),
            Lang::Html => tree_sitter_html::LANGUAGE.into(),
            Lang::Markdown => tree_sitter_md::LANGUAGE.into(),
        }
    }
}

/// Detects the language of a file from its path (extension or well-known file name).
///
/// Only the final path component is considered; both `/` and `\` are accepted as separators.
pub fn detect(path: &str) -> Option<Lang> {
    let file_name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let lower = file_name.to_ascii_lowercase();

    // Well-known file names without a meaningful extension.
    match lower.as_str() {
        "cargo.lock" | "pipfile" | "poetry.lock" | "uv.lock" | ".rustfmt.toml" => {
            return Some(Lang::Toml)
        }
        ".bashrc" | ".bash_profile" | ".zshrc" | ".profile" | ".envrc" | "pkgbuild" => {
            return Some(Lang::Bash)
        }
        ".clang-format" | ".clang-tidy" => return Some(Lang::Yaml),
        "tsconfig.json" | "package.json" | ".prettierrc" | ".eslintrc" | ".babelrc" => {
            return Some(Lang::Json)
        }
        _ => {}
    }

    let (stem, ext) = lower.rsplit_once('.')?;
    if stem.is_empty() && !matches!(ext, "bashrc" | "zshrc") {
        // Dotfiles like `.gitignore` have no extension.
        return None;
    }
    Some(match ext {
        "rs" => Lang::Rust,
        "py" | "pyi" | "pyw" => Lang::Python,
        "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
        "ts" | "mts" | "cts" => Lang::TypeScript,
        "tsx" => Lang::Tsx,
        "go" => Lang::Go,
        "c" | "h" => Lang::C,
        "cc" | "cpp" | "cxx" | "c++" | "hh" | "hpp" | "hxx" | "h++" | "ipp" | "inl" | "tpp" => {
            Lang::Cpp
        }
        "java" => Lang::Java,
        "swift" => Lang::Swift,
        "kt" | "kts" => Lang::Kotlin,
        "json" | "jsonc" | "json5" | "geojson" | "webmanifest" => Lang::Json,
        "toml" => Lang::Toml,
        "yaml" | "yml" => Lang::Yaml,
        "sh" | "bash" | "zsh" | "ksh" | "bashrc" | "zshrc" => Lang::Bash,
        "css" => Lang::Css,
        "html" | "htm" | "xhtml" => Lang::Html,
        "md" | "markdown" | "mdx" | "mkd" => Lang::Markdown,
        _ => return None,
    })
}

/// Upper bound on synthetic closing braces appended by [`closing_suffix`].
const MAX_RECOVERY_BRACES: usize = 64;

/// For brace-delimited languages, returns `"\n}}…"` closing every `{` left open at the end of
/// `source`, or `None` when nothing is open.
///
/// Diff hunks routinely cut blocks open; tree-sitter then often folds the rest of the fragment
/// into a single flat `ERROR` node. Parsing `source + suffix` instead keeps every byte offset
/// and line number of the original text valid while recovering a sensible tree.
///
/// Stray leading `}` are ignored (they close blocks opened before the fragment). Braces inside
/// strings and comments are counted too: this is a heuristic, and a miscount only affects
/// recovery quality, never correctness of offsets.
pub(crate) fn closing_suffix(lang: Lang, source: &str) -> Option<String> {
    let brace_language = matches!(
        lang,
        Lang::Rust
            | Lang::JavaScript
            | Lang::TypeScript
            | Lang::Tsx
            | Lang::Go
            | Lang::C
            | Lang::Cpp
            | Lang::Java
            | Lang::Swift
            | Lang::Kotlin
    );
    if !brace_language {
        return None;
    }
    let open = source.bytes().fold(0usize, |depth, b| match b {
        b'{' => depth + 1,
        b'}' => depth.saturating_sub(1),
        _ => depth,
    });
    let open = open.min(MAX_RECOVERY_BRACES);
    (open > 0).then(|| {
        let mut suffix = String::with_capacity(open + 1);
        suffix.push('\n');
        suffix.extend(std::iter::repeat_n('}', open));
        suffix
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_by_extension_and_name() {
        assert_eq!(detect("crates/app/src/main.rs"), Some(Lang::Rust));
        assert_eq!(detect("src/foo.tests.rs"), Some(Lang::Rust));
        assert_eq!(detect("Cargo.lock"), Some(Lang::Toml));
        assert_eq!(detect("web/App.TSX"), Some(Lang::Tsx));
        assert_eq!(detect("a\\b\\c.hpp"), Some(Lang::Cpp));
        assert_eq!(detect("README.md"), Some(Lang::Markdown));
        assert_eq!(detect(".github/workflows/ci.yml"), Some(Lang::Yaml));
        assert_eq!(detect("Dockerfile"), None);
        assert_eq!(detect("Makefile"), None);
        assert_eq!(detect(".gitignore"), None);
        assert_eq!(detect("image.png"), None);
    }

    #[test]
    fn names_round_trip() {
        for lang in Lang::ALL {
            assert_eq!(Lang::from_name(lang.name()), Some(lang), "{lang:?}");
        }
    }

    #[test]
    fn closing_suffix_balances_open_blocks() {
        assert_eq!(
            closing_suffix(Lang::Rust, "}\nfn a() {\n if x {"),
            Some("\n}}".into())
        );
        assert_eq!(closing_suffix(Lang::Rust, "fn a() {}"), None);
        assert_eq!(closing_suffix(Lang::Python, "x = {"), None);
    }
}
