//! The file view: any file of the repo at the PR's head, in full and without diff, next to
//! the project tree. Files changed by the PR are marked in the tree, and their added and
//! modified lines in the gutter.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{ScrollStrategy, UniformListScrollHandle};
use prmator_syntax::LineHighlights;

use crate::diff::{FileDiff, FileStatus, LineKind};
use crate::model::{Load, PrKey};
use crate::selection::{display_text, TokenSel};
use crate::tree::{self, TreeRow};

/// How a line of the head version relates to the PR's diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineMark {
    Added,
    /// Added in place of removed lines.
    Modified,
    /// Lines were removed just above this one.
    RemovedAbove,
}

pub struct SourceFile {
    /// Raw lines (`code_line_with_spans` expands tabs when drawing).
    pub lines: Vec<String>,
    /// Syntax spans per line, in raw byte offsets.
    pub highlights: LineHighlights,
    /// Changed lines by 1-based line number, for files in the PR.
    pub marks: HashMap<u32, LineMark>,
}

impl SourceFile {
    pub fn new(source: &str, path: &str, diff: Option<&FileDiff>) -> Self {
        let highlights = prmator_syntax::detect(path).map(|lang| prmator_syntax::highlight(lang, source)).unwrap_or_default();
        SourceFile {
            lines: source.lines().map(String::from).collect(),
            highlights,
            marks: diff.map(line_marks).unwrap_or_default(),
        }
    }
}

/// Head-side line marks from a file's diff.
pub fn line_marks(file: &FileDiff) -> HashMap<u32, LineMark> {
    let mut marks = HashMap::new();
    for hunk in &file.hunks {
        // Inside a run of removals, and whether lines were added since.
        let (mut removed, mut added) = (false, false);
        for line in &hunk.lines {
            match (line.kind, line.new_no) {
                (LineKind::Removed, _) => {
                    removed = true;
                    added = false;
                }
                (LineKind::Added, Some(n)) => {
                    marks.insert(n, if removed { LineMark::Modified } else { LineMark::Added });
                    added = true;
                }
                (LineKind::Context, Some(n)) => {
                    if removed && !added {
                        marks.insert(n, LineMark::RemovedAbove);
                    }
                    (removed, added) = (false, false);
                }
                _ => {}
            }
        }
    }
    marks
}

pub struct FileBrowser {
    pub key: PrKey,
    /// The PR's head commit: what is shown.
    pub sha: String,
    /// Every file path in the repo at `sha`, sorted like the tree, and whether GitHub cut
    /// the list short.
    pub tree: Load<(Vec<String>, bool)>,
    /// Open folders, by path.
    pub expanded: HashSet<String>,
    /// Visible tree rows (file indices point into `tree`), rebuilt when folders open/close.
    pub tree_rows: Vec<TreeRow>,
    /// Status of the files the PR changes, by path.
    pub changed: HashMap<String, FileStatus>,
    /// Every folder holding a changed file (at any depth).
    pub changed_dirs: HashSet<String>,
    pub path: Option<String>,
    pub files: HashMap<String, Load<Arc<SourceFile>>>,
    /// Line (1-based) to bring into view once the file is loaded.
    pub pending_line: Option<u32>,
    pub scroll: UniformListScrollHandle,
    pub tree_scroll: UniformListScrollHandle,
    /// Selected rows (anchor, head) and the token selection within them.
    pub selection: Option<(usize, usize)>,
    pub tokens: Option<TokenSel>,
}

impl FileBrowser {
    pub fn new(key: PrKey, sha: String, changed: HashMap<String, FileStatus>) -> Self {
        let changed_dirs = changed
            .keys()
            .flat_map(|p| {
                let parts: Vec<&str> = p.split('/').collect();
                (1..parts.len()).map(move |i| parts[..i].join("/"))
            })
            .collect();
        FileBrowser {
            changed_dirs,
            key,
            sha,
            tree: Load::Loading,
            expanded: HashSet::new(),
            tree_rows: vec![],
            changed,
            path: None,
            files: HashMap::new(),
            pending_line: None,
            scroll: UniformListScrollHandle::new(),
            tree_scroll: UniformListScrollHandle::new(),
            selection: None,
            tokens: None,
        }
    }

    pub fn paths(&self) -> &[String] {
        match &self.tree {
            Load::Ready((paths, _)) => paths,
            _ => &[],
        }
    }

    pub fn set_tree(&mut self, mut paths: Vec<String>, truncated: bool) {
        // The PR's own files are always listed, even when GitHub truncates the tree.
        for p in self.changed.iter().filter(|(_, s)| **s != FileStatus::Deleted).map(|(p, _)| p) {
            if !paths.contains(p) {
                paths.push(p.clone());
            }
        }
        paths.sort_by(|a, b| tree::tree_order(a, b));
        self.tree = Load::Ready((paths, truncated));
        self.rebuild_tree();
    }

    pub fn rebuild_tree(&mut self) {
        let paths: Vec<&str> = self.paths().iter().map(String::as_str).collect();
        let expanded = &self.expanded;
        self.tree_rows = tree::rows_where(&paths, &|dir| expanded.contains(dir));
    }

    pub fn toggle_dir(&mut self, path: &str) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_string());
        }
        self.rebuild_tree();
    }

    /// Opens every folder above `path` and scrolls the tree to it.
    pub fn reveal(&mut self, path: &str) {
        let parts: Vec<&str> = path.split('/').collect();
        for i in 1..parts.len() {
            self.expanded.insert(parts[..i].join("/"));
        }
        self.rebuild_tree();
        let paths = self.paths();
        if let Some(row) = self.tree_rows.iter().position(|r| matches!(r, TreeRow::File { fi, .. } if paths[*fi] == path)) {
            self.tree_scroll.scroll_to_item(row, ScrollStrategy::Center);
        }
    }

    pub fn current(&self) -> Option<&Arc<SourceFile>> {
        match self.files.get(self.path.as_ref()?) {
            Some(Load::Ready(f)) => Some(f),
            _ => None,
        }
    }

    pub fn line_count(&self) -> usize {
        self.current().map_or(0, |f| f.lines.len())
    }

    pub fn row_text(&self, row: usize, side: usize) -> Option<String> {
        (side == 0).then(|| Some(display_text(self.current()?.lines.get(row)?))).flatten()
    }

    pub fn token_sel(&self) -> Option<TokenSel> {
        self.tokens.filter(|t| self.selection == Some((t.anchor.row, t.head.row)))
    }

    pub fn set_token_sel(&mut self, sel: TokenSel) {
        self.selection = Some((sel.anchor.row, sel.head.row));
        self.tokens = Some(sel);
    }

    pub fn selected_range(&self) -> Option<std::ops::Range<usize>> {
        self.selection.map(|(a, b)| a.min(b)..a.max(b) + 1)
    }

    /// Scrolls to the pending line, once the file is there.
    pub fn apply_pending_line(&mut self) {
        if self.current().is_none() {
            return;
        }
        if let Some(line) = self.pending_line.take() {
            let row = (line as usize).saturating_sub(1).min(self.line_count().saturating_sub(1));
            self.scroll.scroll_to_item_strict(row, ScrollStrategy::Center);
            self.selection = Some((row, row));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse;

    #[test]
    fn marks_added_modified_and_removed_lines() {
        let diff = "diff --git a/f.rs b/f.rs\n--- a/f.rs\n+++ b/f.rs\n\
@@ -1,6 +1,6 @@\n a\n-b\n+B\n+new\n c\n-d\n e\n f\n";
        let marks = line_marks(&parse(diff)[0]);
        assert_eq!(marks.get(&2), Some(&LineMark::Modified));
        assert_eq!(marks.get(&3), Some(&LineMark::Modified));
        assert_eq!(marks.get(&4), None);
        assert_eq!(marks.get(&5), Some(&LineMark::RemovedAbove));
        assert_eq!(marks.len(), 3);
    }
}
