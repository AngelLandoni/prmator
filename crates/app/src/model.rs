//! State of an opened PR: files, the flattened rows the diff list renders (unified or
//! side-by-side), selection, analysis results and review progress.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use gpui::{ScrollStrategy, UniformListScrollHandle};
use protocol::ChangeContext;

use crate::analysis::FileAnalysis;
use crate::diff::{FileDiff, FileStatus, Hunk, Line, LineKind};
use crate::github::{PrDetail, PrSummary};
use crate::selection::{display_text, TokenSel};
use crate::store::{DiffMode, SavedPlan};

const MAX_SNIPPET_LINES: usize = 800;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PrKey {
    pub repo: String,
    pub number: u64,
}

impl PrKey {
    pub fn store_key(&self) -> String {
        crate::store::pr_key(&self.repo, self.number)
    }
}

pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    File(usize),
    /// "binary file" / "no changes" placeholder.
    Note(usize),
    Hunk(usize, usize),
    /// Unified: (file, hunk, line).
    Line(usize, usize, usize),
    /// Side by side: (file, hunk, old line index, new line index).
    Split(usize, usize, Option<usize>, Option<usize>),
    /// Buttons to reveal hidden unchanged lines of gap `k` (the gap before hunk `k`;
    /// `k == hunks.len()` is the end of the file): (file, gap).
    Expand(usize, usize),
    /// A revealed unchanged line: (file, gap, new-side line number).
    Context(usize, usize, u32),
    /// Story mode: a hunk of this file that belongs to another virtual commit:
    /// (file, hunk, commit).
    Elsewhere(usize, usize, usize),
    /// Review comments (GitHub threads and drafts) on a line: (file, new side?, line).
    Comments(usize, bool, u32),
}

/// Lines revealed per click on an expander.
pub const EXPAND_STEP: u32 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpandDir {
    /// Reveal lines above the hunk below the gap.
    Up,
    /// Reveal lines below the hunk above the gap.
    Down,
    All,
}

/// Unchanged lines hidden between two hunks, in new-side line numbers (inclusive).
#[derive(Clone, Copy, Debug)]
pub struct Gap {
    pub start: u32,
    /// `None` at the end of the file while its length is unknown.
    pub end: Option<u32>,
    /// old line number = new line number + delta.
    pub delta: i64,
}

impl Gap {
    pub fn size(&self) -> Option<u32> {
        self.end.map(|e| (e + 1).saturating_sub(self.start))
    }
}

impl Row {
    pub fn file(self) -> usize {
        match self {
            Row::File(f)
            | Row::Note(f)
            | Row::Hunk(f, _)
            | Row::Line(f, _, _)
            | Row::Split(f, ..)
            | Row::Expand(f, _)
            | Row::Context(f, ..)
            | Row::Elsewhere(f, ..)
            | Row::Comments(f, ..) => f,
        }
    }
}

/// The two pages of an open PR, like GitHub's tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrPage {
    /// Description, comments, reviews, checks.
    Conversation,
    /// The diff: review mode.
    Files,
    /// The diff retold as ordered virtual commits.
    Story,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LeftTab {
    Files,
    Plan,
    Story,
}

pub struct PlanRun {
    pub id: u64,
    pub activity: Vec<String>,
}

pub struct PrView {
    pub page: PrPage,
    /// Review comments written here, not posted yet.
    pub drafts: Vec<crate::comments::DraftComment>,
    /// Virtual commits (story mode), and reading state.
    pub story: Option<crate::story::SavedStory>,
    pub story_run: Option<PlanRun>,
    pub story_error: Option<String>,
    /// Set while reading the story: which commit, and which commit owns each hunk.
    pub story_focus: Option<crate::story::StoryFocus>,
    /// The saved story no longer matches the diff (new commits were pushed).
    pub story_outdated: bool,
    /// Inline review comments (threads on diff lines).
    pub review_comments: Load<Vec<crate::github::ReviewComment>>,
    /// Long comments expanded on the Conversation page.
    pub expanded_comments: HashSet<String>,
    pub key: PrKey,
    pub summary: PrSummary,
    pub detail: Option<PrDetail>,
    /// The head commit the loaded diff is from. Comments must use it: their line numbers
    /// come from this diff, and `detail` may since have moved to a newer push.
    pub diff_sha: Option<String>,
    pub files: Load<Vec<FileDiff>>,
    pub rows: Vec<Row>,
    pub mode: DiffMode,
    pub collapsed: HashSet<usize>,
    pub viewed: HashSet<usize>,
    /// (anchor, head) row indices, always inside one file.
    pub selection: Option<(usize, usize)>,
    /// Token-wise selection inside `selection`'s rows; see [`PrView::token_sel`].
    pub tokens: Option<TokenSel>,
    pub current_file: usize,
    pub scroll: UniformListScrollHandle,
    pub analysis: HashMap<usize, FileAnalysis>,
    pub analysis_running: bool,
    /// Changed character ranges of paired removed/added lines, by (file, hunk, line).
    pub emphasis: HashMap<(usize, usize, usize), Vec<Range<usize>>>,
    pub plan: Option<SavedPlan>,
    pub plan_run: Option<PlanRun>,
    pub plan_error: Option<String>,
    pub current_step: Option<usize>,
    pub left_tab: LeftTab,
    /// Files whose symbol outline is expanded in the file list.
    pub outline_open: HashSet<usize>,
    /// Folders collapsed in the file tree, by path.
    pub collapsed_dirs: HashSet<String>,
    /// Revealed lines per gap: (file, gap) -> (below the previous hunk, above the next one).
    pub expanded: HashMap<(usize, usize), (u32, u32)>,
    /// Head-side file contents, split in lines, for revealing context.
    pub sources: HashMap<usize, Load<Arc<Vec<String>>>>,
}

impl PrView {
    pub fn new(key: PrKey, summary: PrSummary, mode: DiffMode) -> Self {
        PrView {
            page: PrPage::Conversation,
            drafts: vec![],
            story: None,
            story_run: None,
            story_error: None,
            story_focus: None,
            story_outdated: false,
            review_comments: Load::Loading,
            expanded_comments: HashSet::new(),
            key,
            summary,
            detail: None,
            diff_sha: None,
            files: Load::Loading,
            rows: vec![],
            mode,
            collapsed: HashSet::new(),
            viewed: HashSet::new(),
            selection: None,
            tokens: None,
            current_file: 0,
            scroll: UniformListScrollHandle::new(),
            analysis: HashMap::new(),
            analysis_running: false,
            emphasis: HashMap::new(),
            plan: None,
            plan_run: None,
            plan_error: None,
            current_step: None,
            left_tab: LeftTab::Files,
            outline_open: HashSet::new(),
            collapsed_dirs: HashSet::new(),
            expanded: HashMap::new(),
            sources: HashMap::new(),
        }
    }

    /// The commit the diff shows (falls back to the PR head before the diff is in).
    pub fn diff_sha(&self) -> Option<String> {
        self.diff_sha.clone().or_else(|| self.detail.as_ref().map(|d| d.head_ref_oid.clone()))
    }

    pub fn files(&self) -> &[FileDiff] {
        match &self.files {
            Load::Ready(f) => f,
            _ => &[],
        }
    }

    pub fn file_index(&self, path: &str) -> Option<usize> {
        let files = self.files();
        files
            .iter()
            .position(|f| f.path == path)
            .or_else(|| files.iter().position(|f| f.path.ends_with(path) || path.ends_with(&f.path)))
    }

    pub fn set_files(&mut self, files: Vec<FileDiff>) {
        self.emphasis = files
            .iter()
            .enumerate()
            .flat_map(|(fi, f)| f.hunks.iter().enumerate().map(move |(hi, h)| (fi, hi, h)))
            .flat_map(|(fi, hi, h)| word_emphasis(h).into_iter().map(move |(li, r)| ((fi, hi, li), r)))
            .collect();
        self.files = Load::Ready(files);
    }

    pub fn rebuild_rows(&mut self) {
        if self.page == PrPage::Story {
            if let Some(focus) = &self.story_focus {
                self.rows = self.insert_comment_rows(self.story_rows(focus));
                self.selection = None;
                self.tokens = None;
                return;
            }
        }
        let mut rows = vec![];
        for (fi, file) in self.files().iter().enumerate() {
            rows.push(Row::File(fi));
            if self.collapsed.contains(&fi) {
                continue;
            }
            if file.binary || file.hunks.is_empty() {
                rows.push(Row::Note(fi));
                continue;
            }
            for (hi, hunk) in file.hunks.iter().enumerate() {
                self.push_gap_rows(&mut rows, fi, hi);
                rows.push(Row::Hunk(fi, hi));
                match self.mode {
                    DiffMode::Unified => rows.extend((0..hunk.lines.len()).map(|li| Row::Line(fi, hi, li))),
                    DiffMode::Split => {
                        rows.extend(split_pairs(hunk).into_iter().map(|(l, r)| Row::Split(fi, hi, l, r)))
                    }
                }
            }
            self.push_gap_rows(&mut rows, fi, file.hunks.len());
        }
        self.rows = self.insert_comment_rows(rows);
        self.selection = None;
        self.tokens = None;
    }

    /// Rows for one virtual commit: only files it touches; its hunks in full, other
    /// commits' hunks in those files as one-line markers, and the unchanged lines
    /// between them behind expanders, as in the full diff.
    fn story_rows(&self, focus: &crate::story::StoryFocus) -> Vec<Row> {
        let mut rows = vec![];
        for (fi, file) in self.files().iter().enumerate() {
            let mine = (0..file.hunks.len()).any(|hi| focus.owner.get(&(fi, hi)) == Some(&focus.current));
            if !mine {
                continue;
            }
            rows.push(Row::File(fi));
            for (hi, hunk) in file.hunks.iter().enumerate() {
                self.push_gap_rows(&mut rows, fi, hi);
                match focus.owner.get(&(fi, hi)) {
                    Some(&c) if c == focus.current => {
                        rows.push(Row::Hunk(fi, hi));
                        match self.mode {
                            DiffMode::Unified => rows.extend((0..hunk.lines.len()).map(|li| Row::Line(fi, hi, li))),
                            DiffMode::Split => rows
                                .extend(split_pairs(hunk).into_iter().map(|(l, r)| Row::Split(fi, hi, l, r))),
                        }
                    }
                    Some(&c) => rows.push(Row::Elsewhere(fi, hi, c)),
                    None => {}
                }
            }
            self.push_gap_rows(&mut rows, fi, file.hunks.len());
        }
        rows
    }

    /// Leaves story reading (any jump into the code shows the full diff).
    fn show_files_page(&mut self) {
        let was_story = self.page == PrPage::Story;
        self.page = PrPage::Files;
        if was_story {
            self.rebuild_rows();
        }
    }

    /// Whether unchanged lines of this file can be revealed (needs a head version).
    pub fn can_expand(&self, fi: usize) -> bool {
        let file = &self.files()[fi];
        !file.binary && !file.hunks.is_empty() && file.status != FileStatus::Deleted
    }

    pub fn source_lines(&self, fi: usize) -> Option<&Arc<Vec<String>>> {
        match self.sources.get(&fi) {
            Some(Load::Ready(lines)) => Some(lines),
            _ => None,
        }
    }

    /// Gap `k` of file `fi`: the unchanged lines before hunk `k` (or after the last one).
    pub fn gap(&self, fi: usize, k: usize) -> Option<Gap> {
        let hunks = &self.files()[fi].hunks;
        let total = self.source_lines(fi).map(|l| l.len() as u32);
        let (start, delta) = match k.checked_sub(1).and_then(|p| hunks.get(p)) {
            Some(prev) => {
                let (old_end, new_end) = prev.end();
                (new_end + 1, old_end as i64 - new_end as i64)
            }
            None => {
                let (old_before, new_before) = hunks.first()?.before();
                (1, old_before as i64 - new_before as i64)
            }
        };
        let end = match hunks.get(k) {
            Some(next) => Some(next.before().1),
            None => total,
        };
        Some(Gap { start, end, delta })
    }

    /// Hidden lines left in a gap (`None` when unknown).
    pub fn gap_hidden(&self, fi: usize, k: usize) -> Option<u32> {
        let gap = self.gap(fi, k)?;
        let (down, up) = self.expanded.get(&(fi, k)).copied().unwrap_or_default();
        gap.size().map(|s| s.saturating_sub(down + up))
    }

    fn push_gap_rows(&self, rows: &mut Vec<Row>, fi: usize, k: usize) {
        if !self.can_expand(fi) {
            return;
        }
        let Some(gap) = self.gap(fi, k) else { return };
        let (down, up) = self.expanded.get(&(fi, k)).copied().unwrap_or_default();
        let Some(end) = gap.end else {
            // End of file, length unknown until the source is fetched.
            rows.push(Row::Expand(fi, k));
            return;
        };
        if end < gap.start {
            return;
        }
        let size = end - gap.start + 1;
        if down + up >= size {
            rows.extend((gap.start..=end).map(|n| Row::Context(fi, k, n)));
            return;
        }
        rows.extend((gap.start..gap.start + down).map(|n| Row::Context(fi, k, n)));
        rows.push(Row::Expand(fi, k));
        rows.extend((end + 1 - up..=end).map(|n| Row::Context(fi, k, n)));
    }

    /// Reveals more of gap `k`.
    pub fn expand(&mut self, fi: usize, k: usize, dir: ExpandDir) {
        let last = k == self.files()[fi].hunks.len();
        let entry = self.expanded.entry((fi, k)).or_default();
        match dir {
            // Before the first hunk there is nothing above, and after the last one nothing
            // below: both directions mean the only one that makes sense.
            ExpandDir::Up if !last => entry.1 += EXPAND_STEP,
            ExpandDir::Up | ExpandDir::Down if k == 0 => entry.1 += EXPAND_STEP,
            ExpandDir::Up | ExpandDir::Down => entry.0 += EXPAND_STEP,
            ExpandDir::All => entry.0 = u32::MAX / 4,
        }
        self.clamp_expansion(fi, k);
    }

    /// Keeps a gap's counters within its size (once known).
    pub fn clamp_expansion(&mut self, fi: usize, k: usize) {
        let Some(size) = self.gap(fi, k).and_then(|g| g.size()) else { return };
        if let Some((down, up)) = self.expanded.get_mut(&(fi, k)) {
            *up = (*up).min(size);
            *down = (*down).min(size - *up);
        }
    }

    /// The text and old/new numbers of a revealed context line.
    pub fn context_line(&self, fi: usize, k: usize, new_no: u32) -> Line {
        let delta = self.gap(fi, k).map_or(0, |g| g.delta);
        let text = self
            .source_lines(fi)
            .and_then(|l| l.get(new_no as usize - 1))
            .cloned()
            .unwrap_or_default();
        Line {
            kind: LineKind::Context,
            old_no: u32::try_from(new_no as i64 + delta).ok(),
            new_no: Some(new_no),
            text,
        }
    }

    pub fn file_row(&self, fi: usize) -> Option<usize> {
        self.rows.iter().position(|r| matches!(r, Row::File(f) if *f == fi))
    }

    pub fn selected_range(&self) -> Option<Range<usize>> {
        self.selection.map(|(a, b)| a.min(b)..a.max(b) + 1)
    }

    /// The token selection, while it still matches the selected rows (anything that sets
    /// `selection` directly, like rebuilding rows or jumping to a line, drops it).
    pub fn token_sel(&self) -> Option<TokenSel> {
        self.tokens.filter(|t| self.selection == Some((t.anchor.row, t.head.row)))
    }

    pub fn set_token_sel(&mut self, sel: TokenSel) {
        self.selection = Some((sel.anchor.row, sel.head.row));
        self.current_file = self.rows[sel.head.row].file();
        self.tokens = Some(sel);
    }

    /// The code shown on one side of a row (0 = left/unified, 1 = right), tabs expanded.
    /// `None` for rows without code there.
    pub fn row_text(&self, row: usize, side: usize) -> Option<String> {
        let files = self.files();
        let line = |f: usize, h: usize, l: usize| {
            let line = &files[f].hunks[h].lines[l];
            (line.kind != LineKind::Meta).then(|| display_text(&line.text))
        };
        match *self.rows.get(row)? {
            Row::Line(f, h, l) if side == 0 => line(f, h, l),
            Row::Split(f, h, l, r) => line(f, h, if side == 0 { l? } else { r? }),
            Row::Context(f, k, n) => Some(display_text(&self.context_line(f, k, n).text)),
            _ => None,
        }
    }

    pub fn jump_to_file(&mut self, fi: usize) {
        self.show_files_page();
        self.scroll_to_file(fi);
    }

    /// Scrolls to a file's header on the current page (story reading included).
    pub fn scroll_to_file(&mut self, fi: usize) {
        if let Some(row) = self.file_row(fi) {
            self.current_file = fi;
            self.scroll.scroll_to_item(row, ScrollStrategy::Top);
        }
    }

    /// Scrolls to the first diff row showing a line in `lines` on the given side of file
    /// `fi` (expanding the file if needed). Returns false when no such row exists.
    pub fn jump_to_lines(&mut self, fi: usize, new_side: bool, lines: Range<u32>) -> bool {
        self.show_files_page();
        if self.collapsed.remove(&fi) {
            self.rebuild_rows();
        }
        let files = self.files();
        let line_no = |hi: usize, li: usize| {
            let l = &files[fi].hunks[hi].lines[li];
            if new_side { l.new_no } else { l.old_no }
        };
        let hit = self.rows.iter().position(|row| match *row {
            Row::Line(f, hi, li) if f == fi => line_no(hi, li).is_some_and(|n| lines.contains(&n)),
            Row::Split(f, hi, l, r) if f == fi => {
                let li = if new_side { r } else { l };
                li.and_then(|li| line_no(hi, li)).is_some_and(|n| lines.contains(&n))
            }
            _ => false,
        });
        // Not in the diff: look among revealed context lines.
        let hit = hit.or_else(|| {
            self.rows.iter().position(|row| match *row {
                Row::Context(f, k, n) if f == fi => {
                    let n = if new_side { Some(n) } else { self.context_line(fi, k, n).old_no };
                    n.is_some_and(|n| lines.contains(&n))
                }
                _ => false,
            })
        });
        self.current_file = fi;
        match hit {
            Some(row) => {
                // Show a little context above the target.
                self.scroll.scroll_to_item(row.saturating_sub(3), ScrollStrategy::Top);
                self.selection = Some((row, row));
                self.tokens = None;
                true
            }
            None => {
                self.jump_to_file(fi);
                false
            }
        }
    }

    fn context(&self, fi: usize, location: String, snippet: String) -> ChangeContext {
        ChangeContext {
            repo: self.key.repo.clone(),
            pr_number: self.key.number,
            pr_title: self.summary.title.clone(),
            file: self.files()[fi].path.clone(),
            location,
            snippet,
        }
    }

    /// Context for a range of rows (all in one file), in unified form either way.
    pub fn rows_context(&self, range: Range<usize>) -> Option<ChangeContext> {
        let fi = self.rows.get(range.start)?.file();
        let file = &self.files()[fi];
        // (hunk or gap order, line within it) so lines come out in file order; gap `k`
        // sorts just before hunk `k`.
        let mut picked: Vec<((usize, u8), Option<usize>, Option<Line>)> = vec![];
        for row in &self.rows[range] {
            match *row {
                Row::Hunk(_, h) => picked.push(((h, 1), None, None)),
                Row::Line(_, h, l) => picked.push(((h, 1), Some(l), None)),
                Row::Split(_, h, l, r) => {
                    picked.extend([l, r].into_iter().flatten().map(|l| ((h, 1), Some(l), None)))
                }
                Row::Context(f, k, n) => picked.push(((k, 0), Some(n as usize), Some(self.context_line(f, k, n)))),
                _ => {}
            }
        }
        picked.sort_by_key(|(order, l, _)| (*order, *l));
        picked.dedup_by_key(|(order, l, _)| (*order, *l));
        let mut snippet = vec![];
        let mut numbers = vec![];
        for (order, l, context) in picked {
            let line = match (context, l) {
                (Some(line), _) => line,
                (None, None) => {
                    snippet.push(file.hunks[order.0].header.clone());
                    continue;
                }
                (None, Some(l)) => file.hunks[order.0].lines[l].clone(),
            };
            numbers.extend(line.new_no.or(line.old_no));
            snippet.push(line.as_diff());
        }
        if snippet.is_empty() {
            return None;
        }
        let location = match (numbers.iter().min(), numbers.iter().max()) {
            (Some(a), Some(b)) if a == b => format!("L{a}"),
            (Some(a), Some(b)) => format!("L{a}–L{b}"),
            _ => "hunk".into(),
        };
        Some(self.context(fi, location, truncate_snippet(snippet)))
    }

    /// Rows of hunk `hi` of file `fi`, including its header row.
    pub fn hunk_rows(&self, header_row: usize) -> Range<usize> {
        let Row::Hunk(fi, hi) = self.rows[header_row] else { return header_row..header_row + 1 };
        let end = self.rows[header_row + 1..]
            .iter()
            .position(|r| !matches!(r, Row::Line(f, h, _) | Row::Split(f, h, ..) if *f == fi && *h == hi))
            .map_or(self.rows.len(), |n| header_row + 1 + n);
        header_row..end
    }

    pub fn file_context(&self, fi: usize) -> ChangeContext {
        let file = &self.files()[fi];
        let mut snippet = vec![];
        for hunk in &file.hunks {
            snippet.push(hunk.header.clone());
            snippet.extend(hunk.lines.iter().map(|l| l.as_diff()));
        }
        if file.binary {
            snippet.push("(binary file)".into());
        }
        self.context(fi, "whole file".into(), truncate_snippet(snippet))
    }

    /// Changed files and symbols, for the review plan prompt.
    pub fn outline(&self) -> String {
        self.files()
            .iter()
            .enumerate()
            .map(|(fi, f)| crate::analysis::outline_line(f, self.analysis.get(&fi)))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn truncate_snippet(mut lines: Vec<String>) -> String {
    if lines.len() > MAX_SNIPPET_LINES {
        let cut = lines.len() - MAX_SNIPPET_LINES;
        lines.truncate(MAX_SNIPPET_LINES);
        lines.push(format!("… ({cut} more lines truncated; use `gh pr diff` for the rest)"));
    }
    lines.join("\n")
}

/// Pairs a hunk's lines for side-by-side display: context lines face themselves, and each
/// run of removals is zipped with the run of additions that follows it.
pub fn split_pairs(hunk: &Hunk) -> Vec<(Option<usize>, Option<usize>)> {
    let lines = &hunk.lines;
    let mut out = vec![];
    let mut i = 0;
    while i < lines.len() {
        match lines[i].kind {
            LineKind::Context => {
                out.push((Some(i), Some(i)));
                i += 1;
            }
            LineKind::Meta => {
                let prev_removed = i > 0 && lines[i - 1].kind == LineKind::Removed;
                out.push(if prev_removed { (Some(i), None) } else { (None, Some(i)) });
                i += 1;
            }
            LineKind::Removed | LineKind::Added => {
                let removed: Vec<usize> = (i..lines.len()).take_while(|&j| lines[j].kind == LineKind::Removed).collect();
                let after = i + removed.len();
                let added: Vec<usize> = (after..lines.len()).take_while(|&j| lines[j].kind == LineKind::Added).collect();
                for k in 0..removed.len().max(added.len()) {
                    out.push((removed.get(k).copied(), added.get(k).copied()));
                }
                i = after + added.len();
            }
        }
    }
    out
}

/// For each removed line paired with an added line, the byte ranges that differ.
fn word_emphasis(hunk: &Hunk) -> Vec<(usize, Vec<Range<usize>>)> {
    let mut out = vec![];
    for (l, r) in split_pairs(hunk) {
        let (Some(l), Some(r)) = (l, r) else { continue };
        let (old, new) = (&hunk.lines[l], &hunk.lines[r]);
        if old.kind != LineKind::Removed || new.kind != LineKind::Added {
            continue;
        }
        if let Some((a, b)) = token_diff(&old.text, &new.text) {
            out.push((l, a));
            out.push((r, b));
        }
    }
    out
}

fn tokenize(s: &str) -> Vec<Range<usize>> {
    let mut tokens = vec![];
    let mut chars = s.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        let class = |c: char| if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
        let k = class(c);
        let mut end = start + c.len_utf8();
        if k != 2 {
            while let Some(&(i, n)) = chars.peek() {
                if class(n) != k {
                    break;
                }
                end = i + n.len_utf8();
                chars.next();
            }
        }
        tokens.push(start..end);
    }
    tokens
}

/// Token-level LCS diff of two lines. Returns the changed ranges on each side, or `None`
/// when the lines are too different (or too long) for word highlights to help.
fn token_diff(a: &str, b: &str) -> Option<(Vec<Range<usize>>, Vec<Range<usize>>)> {
    let (ta, tb) = (tokenize(a), tokenize(b));
    let (n, m) = (ta.len(), tb.len());
    if n == 0 || m == 0 || n * m > 40_000 {
        return None;
    }
    let mut dp = vec![0u16; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[at(i, j)] = if a[ta[i].clone()] == b[tb[j].clone()] {
                dp[at(i + 1, j + 1)] + 1
            } else {
                dp[at(i + 1, j)].max(dp[at(i, j + 1)])
            };
        }
    }
    let (mut keep_a, mut keep_b) = (vec![false; n], vec![false; m]);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[ta[i].clone()] == b[tb[j].clone()] {
            keep_a[i] = true;
            keep_b[j] = true;
            i += 1;
            j += 1;
        } else if dp[at(i + 1, j)] >= dp[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let changed = |tokens: &[Range<usize>], keep: &[bool], text: &str| {
        let mut ranges: Vec<Range<usize>> = vec![];
        let mut changed_bytes = 0;
        for (t, k) in tokens.iter().zip(keep) {
            if *k {
                continue;
            }
            changed_bytes += t.len();
            match ranges.last_mut() {
                Some(last) if last.end == t.start => last.end = t.end,
                _ => ranges.push(t.clone()),
            }
        }
        let significant = text.trim().len().max(1);
        (ranges, changed_bytes as f32 / significant as f32)
    };
    let (ra, fa) = changed(&ta, &keep_a, a);
    let (rb, fb) = changed(&tb, &keep_b, b);
    if fa.max(fb) > 0.6 || (ra.is_empty() && rb.is_empty()) {
        return None;
    }
    Some((ra, rb))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse;

    /// A 31-line file (head) with a hunk adding a line near the top and one changing a
    /// line further down.
    fn view() -> PrView {
        let diff = "diff --git a/f.rs b/f.rs\n--- a/f.rs\n+++ b/f.rs\n\
@@ -3,3 +3,4 @@\n l3\n+new4\n l4\n l5\n\
@@ -20,2 +21,2 @@\n-old20\n+new21\n l21\n";
        let key = PrKey { repo: "o/r".into(), number: 1 };
        let mut pv = PrView::new(key, crate::github::PrSummary::stub("o/r".into(), 1), DiffMode::Unified);
        pv.set_files(parse(diff));
        let source: Vec<String> = (1..=31).map(|n| format!("n{n}")).collect();
        pv.sources.insert(0, Load::Ready(Arc::new(source)));
        pv.rebuild_rows();
        pv
    }

    #[test]
    fn gaps_and_line_numbers() {
        let pv = view();
        let g0 = pv.gap(0, 0).unwrap();
        assert_eq!((g0.start, g0.end, g0.delta), (1, Some(2), 0));
        let g1 = pv.gap(0, 1).unwrap();
        assert_eq!((g1.start, g1.end, g1.delta), (7, Some(20), -1));
        let g2 = pv.gap(0, 2).unwrap();
        assert_eq!((g2.start, g2.end, g2.delta), (23, Some(31), -1));
        let line = pv.context_line(0, 1, 7);
        assert_eq!((line.old_no, line.new_no, line.text.as_str()), (Some(6), Some(7), "n7"));
        // Every gap starts hidden behind one expander.
        assert_eq!(pv.rows.iter().filter(|r| matches!(r, Row::Expand(..))).count(), 3);
    }

    #[test]
    fn expanding_reveals_and_merges() {
        let mut pv = view();
        pv.expand(0, 1, ExpandDir::Up); // 20 lines above hunk 2, gap has 14: all shown
        pv.rebuild_rows();
        let ctx: Vec<u32> = pv.rows.iter().filter_map(|r| match r {
            Row::Context(_, 1, n) => Some(*n),
            _ => None,
        }).collect();
        assert_eq!(ctx, (7..=20).collect::<Vec<_>>());
        assert!(!pv.rows.contains(&Row::Expand(0, 1)));
        assert_eq!(pv.gap_hidden(0, 1), Some(0));

        pv.expand(0, 0, ExpandDir::Up);
        pv.expand(0, 2, ExpandDir::Down);
        pv.rebuild_rows();
        assert!(!pv.rows.iter().any(|r| matches!(r, Row::Expand(..))));
        // Whole file visible: 31 new-side lines in order, each once.
        let mut seen: Vec<u32> = pv.rows.iter().filter_map(|r| match *r {
            Row::Context(_, _, n) => Some(n),
            Row::Line(f, h, l) => pv.files()[f].hunks[h].lines[l].new_no,
            _ => None,
        }).collect();
        let sorted = { let mut s = seen.clone(); s.sort(); s };
        assert_eq!(seen, sorted);
        seen.dedup();
        assert_eq!(seen, (1..=31).collect::<Vec<_>>());
    }

    #[test]
    fn context_rows_go_into_claude_context() {
        let mut pv = view();
        pv.expand(0, 0, ExpandDir::Up);
        pv.rebuild_rows();
        // File row, then the two revealed lines above hunk 1.
        assert_eq!(&pv.rows[1..3], &[Row::Context(0, 0, 1), Row::Context(0, 0, 2)]);
        let ctx = pv.rows_context(1..4).unwrap();
        assert_eq!(ctx.snippet, " n1\n n2\n@@ -3,3 +3,4 @@");
        assert_eq!(ctx.location, "L1–L2");
    }

    #[test]
    fn story_rows_can_expand_context() {
        let mut pv = view();
        pv.page = PrPage::Story;
        let owner = [((0, 0), 0), ((0, 1), 1)].into_iter().collect();
        pv.story_focus = Some(crate::story::StoryFocus { current: 0, owner });
        pv.rebuild_rows();
        // Expanders around the commit's hunk and the other commit's marker.
        assert_eq!(pv.rows.iter().filter(|r| matches!(r, Row::Expand(..))).count(), 3);
        assert!(pv.rows.contains(&Row::Elsewhere(0, 1, 1)));
        pv.expand(0, 0, ExpandDir::Up);
        pv.expand(0, 1, ExpandDir::Down);
        pv.rebuild_rows();
        assert_eq!(pv.page, PrPage::Story);
        assert_eq!(&pv.rows[1..3], &[Row::Context(0, 0, 1), Row::Context(0, 0, 2)]);
        let below: Vec<u32> = pv.rows.iter().filter_map(|r| match r {
            Row::Context(_, 1, n) => Some(*n),
            _ => None,
        }).collect();
        assert_eq!(below, (7..=20).collect::<Vec<_>>());
    }

    #[test]
    fn pairs_and_word_diff() {
        let diff = "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -1,3 +1,3 @@\n a\n-let total = items.len();\n+let total = items.count();\n c\n";
        let files = parse(diff);
        let hunk = &files[0].hunks[0];
        assert_eq!(split_pairs(hunk), vec![(Some(0), Some(0)), (Some(1), Some(2)), (Some(3), Some(3))]);
        let emph = word_emphasis(hunk);
        assert_eq!(emph.len(), 2);
        assert_eq!(&hunk.lines[1].text[emph[0].1[0].clone()], "len");
        assert_eq!(&hunk.lines[2].text[emph[1].1[0].clone()], "count");
    }
}
