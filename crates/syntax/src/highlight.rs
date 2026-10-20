//! Syntax highlighting built on `tree-sitter-highlight`.
//!
//! Each language's [`HighlightConfiguration`] is compiled lazily, once per process, and shared
//! between threads (configurations are immutable). The mutable [`Highlighter`] state lives in a
//! thread-local, so concurrent callers never contend on a lock.

use std::cell::RefCell;
use std::ops::Range;
use std::panic::AssertUnwindSafe;
use std::sync::OnceLock;

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::lang::{closing_suffix, Lang};
use crate::MAX_SOURCE_LEN;

/// Semantic category of a highlighted span.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Highlight {
    Keyword,
    Function,
    Type,
    String,
    Number,
    Comment,
    Constant,
    Variable,
    Property,
    Operator,
    Punctuation,
    Attribute,
    Tag,
    Module,
    Macro,
    Label,
    Escape,
}

/// Capture names we recognise, paired with the category they map to.
///
/// `tree-sitter-highlight` matches capture names against this list by dotted prefix and picks
/// the longest match, so `function.method.call` resolves to `function.method`, while
/// `function.macro` beats `function`. Captures matching nothing are ignored (the enclosing
/// highlight, if any, stays in effect).
///
/// The list is shared by every language so that highlight indices stay meaningful across
/// injected layers (e.g. a Rust code fence inside Markdown).
const CAPTURES: &[(&str, Highlight)] = &[
    // Keywords and keyword-like captures used by older query conventions.
    ("keyword", Highlight::Keyword),
    ("conditional", Highlight::Keyword),
    ("repeat", Highlight::Keyword),
    ("include", Highlight::Keyword),
    ("exception", Highlight::Keyword),
    ("storageclass", Highlight::Keyword),
    // Functions and macros.
    ("function", Highlight::Function),
    ("method", Highlight::Function),
    ("function.macro", Highlight::Macro),
    ("function.special", Highlight::Macro),
    ("macro", Highlight::Macro),
    // Types.
    ("type", Highlight::Type),
    ("constructor", Highlight::Type),
    // Strings and escapes.
    ("string", Highlight::String),
    ("character", Highlight::String),
    ("string.escape", Highlight::Escape),
    ("escape", Highlight::Escape),
    ("string.special.key", Highlight::Property),
    // Literals.
    ("number", Highlight::Number),
    ("float", Highlight::Number),
    ("boolean", Highlight::Constant),
    ("constant", Highlight::Constant),
    // Comments.
    ("comment", Highlight::Comment),
    // Variables and members.
    ("variable", Highlight::Variable),
    ("parameter", Highlight::Variable),
    ("variable.member", Highlight::Property),
    ("property", Highlight::Property),
    ("field", Highlight::Property),
    // Operators and punctuation.
    ("operator", Highlight::Operator),
    ("punctuation", Highlight::Punctuation),
    ("delimiter", Highlight::Punctuation),
    // Attributes / annotations / decorators.
    ("attribute", Highlight::Attribute),
    ("decorator", Highlight::Attribute),
    ("annotation", Highlight::Attribute),
    // Markup.
    ("tag", Highlight::Tag),
    ("tag.attribute", Highlight::Attribute),
    // Namespaces.
    ("module", Highlight::Module),
    ("namespace", Highlight::Module),
    // Labels / lifetimes.
    ("label", Highlight::Label),
    // Markdown (both the legacy `text.*` and newer `markup.*` conventions).
    ("text.title", Highlight::Keyword),
    ("markup.heading", Highlight::Keyword),
    ("text.literal", Highlight::String),
    ("markup.raw", Highlight::String),
    ("text.uri", Highlight::Tag),
    ("markup.link.url", Highlight::Tag),
    ("text.reference", Highlight::Label),
    ("markup.link", Highlight::Label),
];

/// Names passed to [`HighlightConfiguration::configure`]; index `i` maps to `CAPTURES[i].1`.
fn capture_names() -> Vec<&'static str> {
    CAPTURES.iter().map(|(name, _)| *name).collect()
}

/// Identifies one compiled highlight configuration. Every [`Lang`] has one, plus the inline
/// Markdown grammar which is only ever reached through injection from block Markdown.
#[derive(Clone, Copy)]
enum ConfigKey {
    Lang(Lang),
    MarkdownInline,
}

impl ConfigKey {
    const COUNT: usize = Lang::ALL.len() + 1;

    fn index(self) -> usize {
        match self {
            ConfigKey::Lang(lang) => lang as usize,
            ConfigKey::MarkdownInline => Lang::ALL.len(),
        }
    }

    /// Resolves a language name produced by an injection query.
    fn from_injection_name(name: &str) -> Option<ConfigKey> {
        if name == "markdown_inline" {
            return Some(ConfigKey::MarkdownInline);
        }
        // Code-fence info strings may carry extra attributes, e.g. "rust,ignore" or "py {.x}".
        let name = name
            .split(|c: char| c == ',' || c == '{' || c.is_whitespace())
            .next()
            .unwrap_or(name);
        Lang::from_name(name).map(ConfigKey::Lang)
    }

    /// Builds the configuration: grammar plus (highlights, injections, locals) queries.
    fn build(self) -> Option<HighlightConfiguration> {
        use tree_sitter_javascript as js;
        use tree_sitter_typescript as ts;

        let (language, name, highlights, injections): (_, _, String, &str) = match self {
            ConfigKey::MarkdownInline => (
                tree_sitter_md::INLINE_LANGUAGE.into(),
                "markdown_inline",
                tree_sitter_md::HIGHLIGHT_QUERY_INLINE.into(),
                tree_sitter_md::INJECTION_QUERY_INLINE,
            ),
            ConfigKey::Lang(lang) => {
                let (highlights, injections): (String, &str) = match lang {
                    // The Rust query tags numeric literals as `constant.builtin`; retag them so
                    // numbers look the same across languages.
                    Lang::Rust => (
                        tree_sitter_rust::HIGHLIGHTS_QUERY
                            .replace(
                                "(integer_literal) @constant.builtin",
                                "(integer_literal) @number",
                            )
                            .replace(
                                "(float_literal) @constant.builtin",
                                "(float_literal) @number",
                            ),
                        tree_sitter_rust::INJECTIONS_QUERY,
                    ),
                    Lang::Python => (tree_sitter_python::HIGHLIGHTS_QUERY.into(), ""),
                    Lang::JavaScript => (
                        [js::JSX_HIGHLIGHT_QUERY, js::HIGHLIGHT_QUERY].join("\n"),
                        js::INJECTIONS_QUERY,
                    ),
                    // The TypeScript queries only cover TS-specific syntax and are meant to be
                    // layered on top of the JavaScript ones. Earlier patterns take precedence.
                    Lang::TypeScript => (
                        [ts::HIGHLIGHTS_QUERY, js::HIGHLIGHT_QUERY].join("\n"),
                        js::INJECTIONS_QUERY,
                    ),
                    Lang::Tsx => (
                        [
                            ts::HIGHLIGHTS_QUERY,
                            js::JSX_HIGHLIGHT_QUERY,
                            js::HIGHLIGHT_QUERY,
                        ]
                        .join("\n"),
                        js::INJECTIONS_QUERY,
                    ),
                    Lang::Go => (tree_sitter_go::HIGHLIGHTS_QUERY.into(), ""),
                    Lang::C => (tree_sitter_c::HIGHLIGHT_QUERY.into(), ""),
                    // C++ queries likewise extend the C ones.
                    Lang::Cpp => (
                        [
                            tree_sitter_cpp::HIGHLIGHT_QUERY,
                            tree_sitter_c::HIGHLIGHT_QUERY,
                        ]
                        .join("\n"),
                        "",
                    ),
                    Lang::Java => (tree_sitter_java::HIGHLIGHTS_QUERY.into(), ""),
                    Lang::Swift => (tree_sitter_swift::HIGHLIGHTS_QUERY.into(), ""),
                    Lang::Kotlin => (tree_sitter_kotlin_sg::HIGHLIGHTS_QUERY.into(), ""),
                    Lang::Json => (tree_sitter_json::HIGHLIGHTS_QUERY.into(), ""),
                    Lang::Toml => (tree_sitter_toml_ng::HIGHLIGHTS_QUERY.into(), ""),
                    Lang::Yaml => (tree_sitter_yaml::HIGHLIGHTS_QUERY.into(), ""),
                    Lang::Bash => (tree_sitter_bash::HIGHLIGHT_QUERY.into(), ""),
                    Lang::Css => (tree_sitter_css::HIGHLIGHTS_QUERY.into(), ""),
                    Lang::Html => (
                        tree_sitter_html::HIGHLIGHTS_QUERY.into(),
                        tree_sitter_html::INJECTIONS_QUERY,
                    ),
                    Lang::Markdown => (
                        tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into(),
                        tree_sitter_md::INJECTION_QUERY_BLOCK,
                    ),
                };
                (lang.ts_language(), lang.name(), highlights, injections)
            }
        };

        // Locals queries are intentionally skipped: they cost time and only refine variable
        // highlighting, which a diff viewer barely benefits from.
        let mut config =
            match HighlightConfiguration::new(language, name, &highlights, injections, "") {
                Ok(config) => config,
                Err(err) => {
                    // A query that fails to compile is a packaging bug, not a runtime condition;
                    // degrade to "no highlighting" for this language rather than panicking.
                    debug_assert!(false, "highlight query for {name} failed to compile: {err}");
                    return None;
                }
            };
        config.configure(&capture_names());
        Some(config)
    }
}

/// Returns the shared configuration for `key`, compiling it on first use.
fn config(key: ConfigKey) -> Option<&'static HighlightConfiguration> {
    static CONFIGS: [OnceLock<Option<HighlightConfiguration>>; ConfigKey::COUNT] =
        [const { OnceLock::new() }; ConfigKey::COUNT];
    CONFIGS[key.index()].get_or_init(|| key.build()).as_ref()
}

thread_local! {
    /// Per-thread highlighter (it owns a parser and query cursors, which are not `Sync`).
    static HIGHLIGHTER: RefCell<Highlighter> = RefCell::new(Highlighter::new());
}

/// Per-line highlight spans; see [`highlight`].
pub type LineHighlights = Vec<Vec<(Range<usize>, Highlight)>>;

/// Highlights `source` and returns one entry per line of `source` (the same count as
/// `source.lines()`; a trailing newline does not add a line).
///
/// Each entry holds non-overlapping, sorted byte ranges relative to the start of that line
/// (never including the line terminator) together with their [`Highlight`]. Unhighlighted text
/// is simply absent. Inputs larger than 2 MB yield empty entries.
///
/// Safe to call concurrently from any number of threads.
pub fn highlight(lang: Lang, source: &str) -> LineHighlights {
    let lines = line_ranges(source);
    let mut out: LineHighlights = vec![Vec::new(); lines.len()];
    if source.len() > MAX_SOURCE_LEN || lines.is_empty() {
        return out;
    }
    let Some(config) = config(ConfigKey::Lang(lang)) else {
        return out;
    };

    // Close blocks left open by a truncated fragment so the parser can recover; spans past the
    // original text are dropped by `distribute`, which only covers the original lines.
    let patched = closing_suffix(lang, source).map(|suffix| format!("{source}{suffix}"));
    let text = patched.as_deref().unwrap_or(source);

    // tree-sitter-highlight is well behaved, but this runs on arbitrary (often truncated) diff
    // fragments in background threads; never let a bug there take the caller down.
    let spans = std::panic::catch_unwind(AssertUnwindSafe(|| highlight_spans(config, text)))
        .unwrap_or_default();

    distribute(&spans, &lines, &mut out);
    out
}

/// Runs the highlighter and returns absolute, sorted, non-overlapping spans.
fn highlight_spans(
    config: &'static HighlightConfiguration,
    source: &str,
) -> Vec<(Range<usize>, Highlight)> {
    HIGHLIGHTER.with(|cell| match cell.try_borrow_mut() {
        Ok(mut highlighter) => run_highlighter(&mut highlighter, config, source),
        // Re-entrancy cannot happen in practice; if it ever did, use a throwaway highlighter
        // rather than panicking on the borrow.
        Err(_) => run_highlighter(&mut Highlighter::new(), config, source),
    })
}

fn run_highlighter(
    highlighter: &mut Highlighter,
    root: &'static HighlightConfiguration,
    source: &str,
) -> Vec<(Range<usize>, Highlight)> {
    let mut spans = Vec::new();
    let Ok(events) = highlighter.highlight(root, source.as_bytes(), None, |name| {
        ConfigKey::from_injection_name(name).and_then(config)
    }) else {
        return spans;
    };

    // Stack of active highlights; the innermost one wins for any source run.
    let mut stack: Vec<Highlight> = Vec::new();
    for event in events {
        match event {
            Ok(HighlightEvent::HighlightStart(h)) => {
                // Every config is configured with `CAPTURES`, so indices always resolve; the
                // fallback keeps the stack balanced regardless.
                stack.push(
                    CAPTURES
                        .get(h.0)
                        .map_or(Highlight::Variable, |(_, kind)| *kind),
                );
            }
            Ok(HighlightEvent::HighlightEnd) => {
                stack.pop();
            }
            Ok(HighlightEvent::Source { start, end }) => {
                if let (Some(&kind), true) = (stack.last(), start < end) {
                    push_merged(&mut spans, start..end, kind);
                }
            }
            // Cancellation or an internal error: keep whatever we have so far.
            Err(_) => break,
        }
    }
    spans
}

/// Appends a span, merging it with the previous one when adjacent and of the same kind.
fn push_merged(spans: &mut Vec<(Range<usize>, Highlight)>, range: Range<usize>, kind: Highlight) {
    if let Some((last, last_kind)) = spans.last_mut() {
        if *last_kind == kind && last.end == range.start {
            last.end = range.end;
            return;
        }
    }
    spans.push((range, kind));
}

/// Byte range of each line's content (excluding `\n` / `\r\n`), matching `str::lines()`.
fn line_ranges(source: &str) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let bytes = source.as_bytes();
    let mut start = 0;
    while start < bytes.len() {
        let end = memchr_newline(&bytes[start..]).map_or(bytes.len(), |i| start + i);
        let mut content_end = end;
        // Like `str::lines()`, strip the `\r` of a `\r\n` terminator (but not a lone `\r`).
        if end < bytes.len() && content_end > start && bytes[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        lines.push(start..content_end);
        start = end + 1;
    }
    lines
}

fn memchr_newline(bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|&b| b == b'\n')
}

/// Splits absolute spans into per-line, line-relative spans.
fn distribute(
    spans: &[(Range<usize>, Highlight)],
    lines: &[Range<usize>],
    out: &mut LineHighlights,
) {
    let mut line_idx = 0;
    for (range, kind) in spans {
        // Spans are sorted, so the line cursor only ever moves forward.
        while line_idx < lines.len() && lines[line_idx].end < range.start {
            // `<` (not `<=`) keeps empty-tail edge cases on the current line; clipping below
            // discards anything that falls outside the line content.
            line_idx += 1;
        }
        let mut i = line_idx;
        while i < lines.len() && lines[i].start < range.end {
            let line = &lines[i];
            let start = range.start.max(line.start);
            let end = range.end.min(line.end);
            if start < end {
                let rel = (start - line.start)..(end - line.start);
                let entry = &mut out[i];
                match entry.last_mut() {
                    Some((last, last_kind)) if *last_kind == *kind && last.end == rel.start => {
                        last.end = rel.end;
                    }
                    _ => entry.push((rel, *kind)),
                }
            }
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_ranges_match_str_lines() {
        for src in [
            "",
            "a",
            "a\n",
            "a\nb",
            "a\r\nb\r\n",
            "\n\n",
            "x\r",
            "a\n\nb\n",
        ] {
            let ranges = line_ranges(src);
            let expected: Vec<&str> = src.lines().collect();
            let got: Vec<&str> = ranges.iter().map(|r| &src[r.clone()]).collect();
            assert_eq!(got, expected, "source {src:?}");
        }
    }

    #[test]
    fn every_query_compiles() {
        for lang in Lang::ALL {
            assert!(config(ConfigKey::Lang(lang)).is_some(), "{lang:?}");
        }
        assert!(config(ConfigKey::MarkdownInline).is_some());
    }

    /// Returns the highlight covering `needle` (first occurrence on `line`), requiring the
    /// whole needle to be covered by a single span.
    fn kind_at(hl: &LineHighlights, src: &str, line: usize, needle: &str) -> Option<Highlight> {
        let text = src.lines().nth(line).unwrap();
        let start = text.find(needle).unwrap();
        let end = start + needle.len();
        hl[line]
            .iter()
            .find(|(r, _)| r.start <= start && end <= r.end)
            .map(|(_, h)| *h)
    }

    /// Asserts the per-line contract: count, bounds, ordering, no overlap.
    fn assert_well_formed(src: &str, hl: &LineHighlights) {
        let lines: Vec<&str> = src.lines().collect();
        assert_eq!(hl.len(), lines.len());
        for (line, spans) in lines.iter().zip(hl) {
            let mut prev_end = 0;
            for (range, _) in spans {
                assert!(range.start < range.end, "empty span {range:?} on {line:?}");
                assert!(
                    range.start >= prev_end,
                    "overlap/unsorted at {range:?} on {line:?}"
                );
                assert!(
                    range.end <= line.len(),
                    "{range:?} out of bounds on {line:?}"
                );
                assert!(line.is_char_boundary(range.start) && line.is_char_boundary(range.end));
                prev_end = range.end;
            }
        }
    }

    const RUST: &str = "// a comment\nfn main() {\n    let s = \"héllo\\n\";\n    let n = 42;\n    println!(\"{}\", s);\n}\n";

    #[test]
    fn rust_highlights() {
        let hl = highlight(Lang::Rust, RUST);
        assert_well_formed(RUST, &hl);
        assert_eq!(
            kind_at(&hl, RUST, 0, "// a comment"),
            Some(Highlight::Comment)
        );
        assert_eq!(kind_at(&hl, RUST, 1, "fn"), Some(Highlight::Keyword));
        assert_eq!(kind_at(&hl, RUST, 1, "main"), Some(Highlight::Function));
        assert_eq!(kind_at(&hl, RUST, 2, "let"), Some(Highlight::Keyword));
        assert_eq!(kind_at(&hl, RUST, 2, "\"héllo"), Some(Highlight::String));
        assert_eq!(kind_at(&hl, RUST, 2, "\\n"), Some(Highlight::Escape));
        assert_eq!(kind_at(&hl, RUST, 3, "42"), Some(Highlight::Number));
        assert_eq!(kind_at(&hl, RUST, 4, "println!"), Some(Highlight::Macro));
    }

    #[test]
    fn multiline_spans_are_split_per_line() {
        let src = "/* one\n   two */\r\nfn x() {}";
        let hl = highlight(Lang::Rust, src);
        assert_well_formed(src, &hl);
        assert_eq!(hl[0], vec![(0..6, Highlight::Comment)]);
        assert_eq!(hl[1][0], (0..9, Highlight::Comment));
    }

    #[test]
    fn fragments_and_garbage_are_safe() {
        let fragment =
            "        let x = \"a\";\n    }\n\n    pub fn apply(&mut self) {\n        if x {\n";
        let hl = highlight(Lang::Rust, fragment);
        assert_well_formed(fragment, &hl);
        assert_eq!(kind_at(&hl, fragment, 3, "fn"), Some(Highlight::Keyword));
        for lang in Lang::ALL {
            for src in [
                "",
                "\n",
                "}}}{{{ (((\n\"unterminated",
                fragment,
                "\u{feff}é\r\n\t",
            ] {
                assert_well_formed(src, &highlight(lang, src));
            }
        }
    }

    #[test]
    fn every_language_highlights_something() {
        let samples = [
            (Lang::Rust, "fn a() {}"),
            (Lang::Python, "def a():\n    return 1"),
            (Lang::JavaScript, "function a() { return 1; }"),
            (Lang::TypeScript, "let a: number = 1;"),
            (Lang::Tsx, "const a = <div className=\"x\">hi</div>;"),
            (Lang::Go, "package main\nfunc a() {}"),
            (Lang::C, "int a(void) { return 1; }"),
            (Lang::Cpp, "class A { public: int a; };"),
            (Lang::Java, "class A { void a() {} }"),
            (Lang::Swift, "func a() -> Int { return 1 }"),
            (Lang::Kotlin, "fun a(): Int = 1"),
            (Lang::Json, "{\"a\": 1}"),
            (Lang::Toml, "[a]\nb = \"c\""),
            (Lang::Yaml, "a: 1\nb: \"c\""),
            (Lang::Bash, "if true; then echo \"a\"; fi"),
            (Lang::Css, "a { color: red; }"),
            (Lang::Html, "<div class=\"a\">b</div>"),
            (Lang::Markdown, "# Title\n\n```rust\nfn a() {}\n```\n"),
        ];
        for (lang, src) in samples {
            let hl = highlight(lang, src);
            assert_well_formed(src, &hl);
            assert!(
                hl.iter().any(|l| !l.is_empty()),
                "{lang:?} produced no highlights"
            );
        }
    }

    #[test]
    fn markdown_code_fences_are_injected() {
        let src = "# Title\n\n```rust\nfn a() {}\n```\n";
        let hl = highlight(Lang::Markdown, src);
        assert_eq!(kind_at(&hl, src, 3, "fn"), Some(Highlight::Keyword));
    }

    #[test]
    fn concurrent_use() {
        let handles: Vec<_> = (0..8)
            .map(|i| {
                std::thread::spawn(move || {
                    let lang = Lang::ALL[i % Lang::ALL.len()];
                    for _ in 0..20 {
                        let hl = highlight(Lang::Rust, RUST);
                        assert_eq!(hl.len(), RUST.lines().count());
                        let _ = highlight(lang, RUST);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn oversized_input_yields_empty_lines() {
        let big = "fn a() {}\n".repeat(MAX_SOURCE_LEN / 10 + 1);
        let hl = highlight(Lang::Rust, &big);
        assert_eq!(hl.len(), big.lines().count());
        assert!(hl.iter().all(Vec::is_empty));
    }
}
