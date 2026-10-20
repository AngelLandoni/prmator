//! Definition lookup for hover cards: given an identifier, find where its type / function
//! is defined and return that code.
//!
//! Sources, in order: the PR's own files (already fetched by `analysis`), a local clone
//! under `PRMATOR_CODE_DIR` (`git grep`), then GitHub code search (rate limited, so last).
//! All functions block; call them from the background executor.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use prmator_syntax::{Lang, Symbol, SymbolKind};

use crate::analysis::{is_type_kind, Spans};
use crate::github;

/// Longest definition shown in a hover card.
const MAX_LINES: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefSource {
    ThisPr,
    LocalClone,
    GitHub,
}

impl DefSource {
    pub fn label(self) -> &'static str {
        match self {
            DefSource::ThisPr => "in this PR",
            DefSource::LocalClone => "local clone",
            DefSource::GitHub => "GitHub",
        }
    }
}

pub struct Definition {
    pub name: String,
    pub kind: SymbolKind,
    pub container: Option<String>,
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub lines: Vec<String>,
    pub highlights: Vec<Spans>,
    /// More lines than `MAX_LINES` were cut.
    pub truncated: bool,
    pub source: DefSource,
    pub url: Option<String>,
}

/// A file already loaded for this PR (head side).
pub struct KnownFile {
    pub path: String,
    pub lang: Lang,
    pub source: Arc<String>,
    pub symbols: Vec<Symbol>,
}

pub struct Query {
    pub name: String,
    pub lang: Lang,
    pub repo: String,
    pub head_sha: Option<String>,
    pub known: Vec<KnownFile>,
}

pub fn lookup(q: &Query) -> Option<Definition> {
    for file in &q.known {
        if let Some(sym) = best_match(&file.symbols, &q.name) {
            return Some(build(q, file.lang, &file.path, &file.source, sym, DefSource::ThisPr));
        }
    }
    if let Some(def) = lookup_local(q) {
        return Some(def);
    }
    lookup_github(q)
}

/// The definition of `name` in `symbols`, preferring types over functions.
fn best_match<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
    let candidates = || symbols.iter().filter(|s| s.name == name && s.kind != SymbolKind::Impl);
    candidates()
        .find(|s| is_type_kind(s.kind))
        .or_else(|| candidates().find(|s| !matches!(s.kind, SymbolKind::Test)))
}

fn build(q: &Query, lang: Lang, path: &str, source: &str, sym: &Symbol, origin: DefSource) -> Definition {
    let start = sym.start_line.max(1) as usize - 1;
    let end = (sym.end_line as usize).max(start + 1);
    // Include doc comments / attributes right above the definition.
    let all: Vec<&str> = source.lines().collect();
    let mut first = start;
    while first > 0 && first + 40 > start {
        let prev = all[first - 1].trim_start();
        if prev.starts_with("///") || prev.starts_with("#[") || prev.starts_with("//!") || prev.starts_with('@') || prev.starts_with("/**") || prev.starts_with('*') {
            first -= 1;
        } else {
            break;
        }
    }
    let shown_end = end.min(first + MAX_LINES).min(all.len());
    let lines: Vec<String> = all[first.min(all.len())..shown_end].iter().map(|l| l.to_string()).collect();
    let highlights = prmator_syntax::highlight(lang, &lines.join("\n"));
    let url = q
        .head_sha
        .as_ref()
        .map(|sha| format!("https://github.com/{}/blob/{sha}/{path}#L{}-L{}", q.repo, sym.start_line, sym.end_line));
    Definition {
        name: sym.name.clone(),
        kind: sym.kind,
        container: sym.container.clone(),
        path: path.to_string(),
        start_line: first as u32 + 1,
        end_line: sym.end_line,
        lines,
        highlights,
        truncated: end > shown_end,
        source: origin,
        url,
    }
}

/// Keywords that introduce a definition, for grepping.
fn def_pattern(name: &str) -> String {
    format!(
        r"(struct|enum|trait|type|union|class|interface|protocol|fn|def|func|function|const|static|macro_rules!|mod|object|record)\s+{name}\b"
    )
}

fn extensions(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::Rust => &["rs"],
        Lang::Python => &["py"],
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => &["ts", "tsx", "js", "jsx", "mjs"],
        Lang::Go => &["go"],
        Lang::C | Lang::Cpp => &["h", "hpp", "hh", "c", "cc", "cpp", "mm", "m"],
        Lang::Java => &["java"],
        Lang::Kotlin => &["kt"],
        Lang::Swift => &["swift"],
        _ => &[],
    }
}

fn local_checkout(repo: &str) -> Option<PathBuf> {
    let base = PathBuf::from(std::env::var("PRMATOR_CODE_DIR").ok()?);
    let dir = base.join(repo.rsplit('/').next()?);
    dir.join(".git").exists().then_some(dir)
}

fn lookup_local(q: &Query) -> Option<Definition> {
    let dir = local_checkout(&q.repo)?;
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(&dir).args(["grep", "-l", "-I", "-E", &def_pattern(&q.name), "--"]);
    for ext in extensions(q.lang) {
        cmd.arg(format!("*.{ext}"));
    }
    let out = cmd.output().ok()?;
    let paths = String::from_utf8_lossy(&out.stdout).into_owned();
    for path in paths.lines().take(20) {
        let Ok(source) = std::fs::read_to_string(dir.join(path)) else { continue };
        let lang = prmator_syntax::detect(path).unwrap_or(q.lang);
        let symbols = prmator_syntax::symbols(lang, &source);
        if let Some(sym) = best_match(&symbols, &q.name) {
            return Some(build(q, lang, path, &source, sym, DefSource::LocalClone));
        }
    }
    None
}

fn lookup_github(q: &Query) -> Option<Definition> {
    let mut args = vec![
        "search".to_string(),
        "code".into(),
        q.name.clone(),
        "--repo".into(),
        q.repo.clone(),
        "--limit".into(),
        "15".into(),
        "--json".into(),
        "path".into(),
        "--jq".into(),
        ".[].path".into(),
    ];
    if let Some(ext) = extensions(q.lang).first() {
        args.extend(["--extension".into(), ext.to_string()]);
    }
    let out = Command::new("gh").args(&args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let paths = String::from_utf8_lossy(&out.stdout).into_owned();
    let sha = q.head_sha.clone().unwrap_or_else(|| "HEAD".into());
    let pattern = regex_lite_match(&q.name);
    for path in paths.lines().take(8) {
        let Ok(source) = github::file_content(&q.repo, &sha, path) else { continue };
        if !pattern(&source) {
            continue;
        }
        let lang = prmator_syntax::detect(path).unwrap_or(q.lang);
        let symbols = prmator_syntax::symbols(lang, &source);
        if let Some(sym) = best_match(&symbols, &q.name) {
            return Some(build(q, lang, path, &source, sym, DefSource::GitHub));
        }
    }
    None
}

/// Cheap pre-filter: does the source contain `<keyword> name`?
fn regex_lite_match(name: &str) -> impl Fn(&str) -> bool + '_ {
    const KEYWORDS: &[&str] = &[
        "struct", "enum", "trait", "type", "union", "class", "interface", "protocol", "fn", "def", "func",
        "function", "const", "static", "mod", "object", "record",
    ];
    move |source: &str| {
        source.match_indices(name).any(|(i, _)| {
            let before = source[..i].trim_end();
            let after = source[i + name.len()..].chars().next();
            let boundary = !after.is_some_and(|c| c.is_alphanumeric() || c == '_');
            boundary && KEYWORDS.iter().any(|k| before.ends_with(k))
        })
    }
}

/// The identifier around byte `ix` of `text`, as (range, name).
pub fn identifier_at(text: &str, ix: usize) -> Option<(std::ops::Range<usize>, String)> {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    if ix >= text.len() || !text.is_char_boundary(ix) {
        return None;
    }
    if !text[ix..].chars().next().is_some_and(is_ident) {
        return None;
    }
    let start = text[..ix].char_indices().rev().take_while(|(_, c)| is_ident(*c)).last().map_or(ix, |(i, _)| i);
    let end = text[ix..].char_indices().find(|(_, c)| !is_ident(*c)).map_or(text.len(), |(i, _)| ix + i);
    let name = &text[start..end];
    if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((start..end, name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_identifier() {
        let text = "let op: BatchOperation = x;";
        let (range, name) = identifier_at(text, 10).unwrap();
        assert_eq!(name, "BatchOperation");
        assert_eq!(&text[range], "BatchOperation");
        assert!(identifier_at(text, 6).is_none());
    }

    #[test]
    fn prefilter_matches_definitions_only() {
        let m = regex_lite_match("Foo");
        assert!(m("pub struct Foo {"));
        assert!(m("enum Foo{"));
        assert!(!m("let x = Foo::new();"));
        assert!(!m("struct FooBar"));
    }
}
