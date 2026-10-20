//! Code analysis of a PR's changed files with tree-sitter: syntax highlighting for every
//! diff line and the symbols (functions, types, ...) each file adds, modifies or removes.
//!
//! Each file is analyzed on worker threads from its full contents at the merge base and at
//! the head commit. When the contents can't be fetched, the diff hunks themselves are parsed
//! as fragments (less accurate, but still useful).

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::sync::{Arc, Mutex};

use prmator_syntax::{Highlight, Lang, Symbol, SymbolKind};

use crate::diff::{FileDiff, FileStatus, LineKind};
use crate::github;

const WORKERS: usize = 8;
/// Larger files are not fetched in full (fragments are used instead).
const MAX_SOURCE_BYTES: usize = 1_500_000;

pub type Spans = Vec<(Range<usize>, Highlight)>;

#[derive(Default)]
pub struct Side {
    /// Highlights of the lines that appear in the diff, by line number on this side.
    pub highlights: HashMap<u32, Spans>,
    pub symbols: Vec<Symbol>,
    /// Full file contents, when they could be fetched.
    pub source: Option<Arc<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolChange {
    Added,
    Modified,
    Removed,
}

impl SymbolChange {
    pub fn label(self) -> &'static str {
        match self {
            SymbolChange::Added => "added",
            SymbolChange::Modified => "modified",
            SymbolChange::Removed => "removed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChangedSymbol {
    pub symbol: Symbol,
    pub change: SymbolChange,
    /// Whether line numbers refer to the new (head) side or the old (base) side.
    pub new_side: bool,
}

impl ChangedSymbol {
    pub fn display_name(&self) -> String {
        match &self.symbol.container {
            Some(c) if !matches!(self.symbol.kind, SymbolKind::Impl) => format!("{c}::{}", self.symbol.name),
            _ => self.symbol.name.clone(),
        }
    }
}

pub struct FileAnalysis {
    pub lang: Lang,
    pub old: Side,
    pub new: Side,
    pub changed: Vec<ChangedSymbol>,
}

pub struct Revisions {
    pub repo: String,
    /// Merge base of the PR (the diff's "old" side).
    pub base: Option<String>,
    pub head: Option<String>,
}

/// Analyzes `files` on worker threads, sending `(file index, analysis)` as each finishes.
pub fn spawn(revs: Revisions, files: Vec<(usize, FileDiff)>, tx: async_channel::Sender<(usize, FileAnalysis)>) {
    let revs = Arc::new(revs);
    let queue = Arc::new(Mutex::new(files));
    for _ in 0..WORKERS {
        let (revs, queue, tx) = (revs.clone(), queue.clone(), tx.clone());
        std::thread::spawn(move || loop {
            let Some((fi, file)) = queue.lock().unwrap().pop() else { break };
            if let Some(analysis) = analyze(&revs, &file) {
                if tx.send_blocking((fi, analysis)).is_err() {
                    break;
                }
            }
        });
    }
}

fn fetch(revs: &Revisions, sha: &Option<String>, path: &str) -> Option<Arc<String>> {
    let source = github::file_content(&revs.repo, sha.as_deref()?, path).ok()?;
    (source.len() <= MAX_SOURCE_BYTES).then(|| Arc::new(source))
}

fn analyze(revs: &Revisions, file: &FileDiff) -> Option<FileAnalysis> {
    let lang = prmator_syntax::detect(&file.path)?;
    if file.binary || file.hunks.is_empty() {
        return None;
    }

    // Old-side line numbers that appear in the diff, and the lines that changed.
    let mut old_lines = BTreeSet::new();
    let mut removed = BTreeSet::new();
    let mut added = BTreeSet::new();
    for line in file.hunks.iter().flat_map(|h| &h.lines) {
        if let Some(n) = line.old_no {
            old_lines.insert(n);
        }
        match line.kind {
            LineKind::Added => added.extend(line.new_no),
            LineKind::Removed => removed.extend(line.old_no),
            _ => {}
        }
    }

    let new_source = (file.status != FileStatus::Deleted).then(|| fetch(revs, &revs.head, &file.path)).flatten();
    let old_path = file.old_path.as_deref().unwrap_or(&file.path);
    let old_source = (file.status != FileStatus::Added).then(|| fetch(revs, &revs.base, old_path)).flatten();
    let new = match new_source {
        Some(src) => full_side(lang, src, None),
        None => fragment_side(lang, file, true),
    };
    let old = match old_source {
        Some(src) => full_side(lang, src, Some(&old_lines)),
        None => fragment_side(lang, file, false),
    };

    let changed = changed_symbols(&old, &new, &removed, &added);
    Some(FileAnalysis { lang, old, new, changed })
}

/// `wanted`: the line numbers to keep highlights for (`None` keeps all, so context lines
/// revealed by expanding the diff are colored too).
fn full_side(lang: Lang, source: Arc<String>, wanted: Option<&BTreeSet<u32>>) -> Side {
    let mut highlights = HashMap::new();
    for (i, spans) in prmator_syntax::highlight(lang, &source).into_iter().enumerate() {
        let n = i as u32 + 1;
        if wanted.is_none_or(|w| w.contains(&n)) && !spans.is_empty() {
            highlights.insert(n, spans);
        }
    }
    let symbols = prmator_syntax::symbols(lang, &source);
    Side { highlights, symbols, source: Some(source) }
}

/// Parses each hunk's lines for one side as a standalone snippet.
fn fragment_side(lang: Lang, file: &FileDiff, new_side: bool) -> Side {
    let mut side = Side::default();
    for hunk in &file.hunks {
        let lines: Vec<(u32, &str)> = hunk
            .lines
            .iter()
            .filter_map(|l| {
                let n = if new_side { l.new_no } else { l.old_no }?;
                (l.kind != LineKind::Meta).then_some((n, l.text.as_str()))
            })
            .collect();
        let Some(&(first, _)) = lines.first() else { continue };
        let text = lines.iter().map(|(_, t)| *t).collect::<Vec<_>>().join("\n");
        for (i, spans) in prmator_syntax::highlight(lang, &text).into_iter().enumerate() {
            if let Some(&(n, _)) = lines.get(i) {
                if !spans.is_empty() {
                    side.highlights.insert(n, spans);
                }
            }
        }
        for mut sym in prmator_syntax::symbols(lang, &text) {
            sym.start_line += first - 1;
            sym.end_line += first - 1;
            side.symbols.push(sym);
        }
    }
    side
}

fn touches(sym: &Symbol, lines: &BTreeSet<u32>) -> bool {
    lines.range(sym.start_line..=sym.end_line).next().is_some()
}

fn same_symbol(a: &Symbol, b: &Symbol) -> bool {
    a.name == b.name && a.kind == b.kind && a.container == b.container
}

fn changed_symbols(old: &Side, new: &Side, removed: &BTreeSet<u32>, added: &BTreeSet<u32>) -> Vec<ChangedSymbol> {
    let mut out = vec![];
    for sym in &new.symbols {
        if !touches(sym, added) {
            continue;
        }
        let existed = old.symbols.iter().any(|o| same_symbol(o, sym));
        let change = if existed { SymbolChange::Modified } else { SymbolChange::Added };
        out.push(ChangedSymbol { symbol: sym.clone(), change, new_side: true });
    }
    for sym in &old.symbols {
        if !touches(sym, removed) {
            continue;
        }
        match new.symbols.iter().find(|n| same_symbol(n, sym)) {
            // Only deletions inside it: still a modification, reported on the new side.
            Some(n) if !touches(n, added) => {
                out.push(ChangedSymbol { symbol: n.clone(), change: SymbolChange::Modified, new_side: true });
            }
            Some(_) => {}
            None => out.push(ChangedSymbol { symbol: sym.clone(), change: SymbolChange::Removed, new_side: false }),
        }
    }
    out.sort_by_key(|c| (c.symbol.start_line, !c.new_side));
    out
}

/// One-line description of what changed in a file, for the review plan prompt.
pub fn outline_line(file: &FileDiff, analysis: Option<&FileAnalysis>) -> String {
    let mut line = format!("- {} (+{} -{})", file.path, file.additions, file.deletions);
    if let Some(a) = analysis {
        let symbols: Vec<String> = a
            .changed
            .iter()
            .filter(|c| !matches!(c.symbol.kind, SymbolKind::Impl))
            .take(20)
            .map(|c| format!("{} {} {}", c.change.label(), kind_label(c.symbol.kind), c.display_name()))
            .collect();
        if !symbols.is_empty() {
            line.push_str(": ");
            line.push_str(&symbols.join(", "));
        }
    }
    line
}

pub fn kind_label(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "fn",
        SymbolKind::Method => "method",
        SymbolKind::Struct => "struct",
        SymbolKind::Class => "class",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Interface => "interface",
        SymbolKind::Impl => "impl",
        SymbolKind::Module => "mod",
        SymbolKind::Const => "const",
        SymbolKind::TypeAlias => "type",
        SymbolKind::Macro => "macro",
        SymbolKind::Test => "test",
    }
}

/// Short glyph for symbol lists.
pub fn kind_icon(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function | SymbolKind::Method => "ƒ",
        SymbolKind::Struct | SymbolKind::Class => "S",
        SymbolKind::Enum => "E",
        SymbolKind::Trait | SymbolKind::Interface => "T",
        SymbolKind::Impl => "I",
        SymbolKind::Module => "M",
        SymbolKind::Const => "C",
        SymbolKind::TypeAlias => "τ",
        SymbolKind::Macro => "!",
        SymbolKind::Test => "✓",
    }
}

pub fn is_type_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Struct
            | SymbolKind::Class
            | SymbolKind::Enum
            | SymbolKind::Trait
            | SymbolKind::Interface
            | SymbolKind::TypeAlias
    )
}
