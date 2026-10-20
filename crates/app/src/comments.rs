//! Review comments on the code, like GitHub's: comments are drafted locally ("pending")
//! and posted together when the review is submitted, or posted right away ("Comment now").
//! Existing threads and drafts show up in the diff under the line they're on.
//!
//! Set `PRMATOR_DRY_RUN=1` to print the GitHub requests instead of sending them.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{point, px, size, Bounds, Context, Pixels, Point, ScrollHandle, Size, Window};
use serde::{Deserialize, Serialize};

use crate::app::Prmator;
use crate::diff::LineKind;
use crate::github::{self, NewComment, ReviewComment};
use crate::model::{Load, PrKey, PrView, Row};

/// A comment written in PRmator, not yet posted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DraftComment {
    pub id: u64,
    pub path: String,
    /// Last line commented on, and whether it's on the new (head) side.
    pub line: u32,
    pub new_side: bool,
    /// First line of a multi-line comment.
    #[serde(default)]
    pub start: Option<(u32, bool)>,
    pub body: String,
}

impl DraftComment {
    pub fn to_github(&self) -> NewComment {
        let side = |new: bool| if new { "RIGHT" } else { "LEFT" };
        NewComment {
            path: self.path.clone(),
            body: self.body.clone(),
            line: self.line,
            side: side(self.new_side),
            start_line: self.start.map(|(l, _)| l),
            start_side: self.start.map(|(_, new)| side(new)),
        }
    }

    pub fn label(&self) -> String {
        let file = self.path.rsplit('/').next().unwrap_or(&self.path);
        match self.start {
            Some((s, _)) if s != self.line => format!("{file}:{s}–{}", self.line),
            _ => format!("{file}:{}", self.line),
        }
    }
}

/// Where comments attach in the diff: (file, new side?, line number on that side).
pub type Anchor = (usize, bool, u32);

#[derive(Clone, Debug, PartialEq)]
pub enum ThreadRef {
    /// A GitHub thread, by its root comment id.
    Remote(u64),
    Draft(u64),
}

#[derive(Clone, Debug)]
pub enum CommentTarget {
    /// A new comment on a range of diff lines (all in one hunk).
    New { path: String, start: Option<(u32, bool)>, end: (u32, bool), label: String, snippet: String },
    /// The threads and drafts on one line.
    Thread { anchor: Anchor },
}

pub struct CommentPopover {
    pub position: Point<Pixels>,
    pub target: CommentTarget,
    pub busy: bool,
    pub error: Option<String>,
    pub scroll: ScrollHandle,
    /// Where the box was last drawn, so moving or resizing starts from what's on screen.
    pub drawn: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub drag: Option<BoxDrag>,
}

/// The comment box being moved or resized with the mouse.
#[derive(Clone, Copy, Debug)]
pub enum BoxDrag {
    /// Grabbed by the header: the grab point relative to the box's top-left.
    Move(Point<Pixels>),
    /// Grabbed by an edge or the corner: which sides follow the mouse, where the mouse
    /// started and the box's bounds then.
    Resize { width: bool, height: bool, from: Point<Pixels>, start: Bounds<Pixels> },
}

/// Smallest comment box, and how far one key press moves or resizes it.
pub const BOX_MIN: Size<Pixels> = Size { width: px(320.), height: px(200.) };
pub const BOX_STEP: f32 = 40.;
const BOX_MARGIN: f32 = 12.;

/// Keeps a box inside the window, at least `BOX_MIN` big.
pub fn clamp_box(b: Bounds<Pixels>, viewport: Size<Pixels>) -> Bounds<Pixels> {
    let m = px(BOX_MARGIN);
    let w = b.size.width.min(viewport.width - m * 2.).max(BOX_MIN.width);
    let h = b.size.height.min(viewport.height - m * 2.).max(BOX_MIN.height);
    let x = b.origin.x.min(viewport.width - w - m).max(m);
    let y = b.origin.y.min(viewport.height - h - m).max(m);
    Bounds { origin: point(x, y), size: size(w, h) }
}

impl From<crate::store::BoxGeometry> for Bounds<Pixels> {
    fn from(g: crate::store::BoxGeometry) -> Self {
        Bounds { origin: point(px(g.x), px(g.y)), size: size(px(g.w), px(g.h)) }
    }
}

impl From<Bounds<Pixels>> for crate::store::BoxGeometry {
    fn from(b: Bounds<Pixels>) -> Self {
        let f = |p: Pixels| f32::from(p).round();
        crate::store::BoxGeometry { x: f(b.origin.x), y: f(b.origin.y), w: f(b.size.width), h: f(b.size.height) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewEvent {
    Comment,
    Approve,
    RequestChanges,
}

impl ReviewEvent {
    pub fn api(self) -> &'static str {
        match self {
            ReviewEvent::Comment => "COMMENT",
            ReviewEvent::Approve => "APPROVE",
            ReviewEvent::RequestChanges => "REQUEST_CHANGES",
        }
    }
}

pub struct ReviewPanel {
    pub event: ReviewEvent,
    pub busy: bool,
    pub error: Option<String>,
}

/// Threads by their root comment, in order.
pub fn remote_threads(comments: &[ReviewComment]) -> Vec<Vec<&ReviewComment>> {
    let by_id: HashMap<u64, &ReviewComment> = comments.iter().map(|c| (c.id, c)).collect();
    let mut roots: Vec<(u64, Vec<&ReviewComment>)> = vec![];
    for c in comments {
        let mut root = c;
        while let Some(parent) = root.in_reply_to_id.and_then(|id| by_id.get(&id)) {
            root = parent;
        }
        match roots.iter_mut().find(|(id, _)| *id == root.id) {
            Some((_, list)) => list.push(c),
            None => roots.push((root.id, vec![c])),
        }
    }
    roots
        .into_iter()
        .map(|(_, mut list)| {
            list.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            list
        })
        .collect()
}

impl PrView {
    /// Threads and drafts by the diff line they're on.
    pub fn comment_anchors(&self) -> HashMap<Anchor, Vec<ThreadRef>> {
        let mut anchors: HashMap<Anchor, Vec<ThreadRef>> = HashMap::new();
        if let Load::Ready(comments) = &self.review_comments {
            for thread in remote_threads(comments) {
                let root = thread[0];
                // Outdated threads (no current line) only show on the Conversation page.
                let (Some(line), Some(fi)) = (root.line, self.file_index_exact(&root.path)) else { continue };
                let new_side = root.side.as_deref() != Some("LEFT");
                anchors.entry((fi, new_side, line)).or_default().push(ThreadRef::Remote(root.id));
            }
        }
        for d in &self.drafts {
            if let Some(fi) = self.file_index_exact(&d.path) {
                anchors.entry((fi, d.new_side, d.line)).or_default().push(ThreadRef::Draft(d.id));
            }
        }
        anchors
    }

    fn file_index_exact(&self, path: &str) -> Option<usize> {
        self.files().iter().position(|f| f.path == path)
    }

    /// The (line, new side?) pairs a row shows, for matching comments.
    pub fn row_lines(&self, row: Row) -> Vec<(bool, u32)> {
        let files = self.files();
        let of = |fi: usize, hi: usize, li: usize| {
            let l = &files[fi].hunks[hi].lines[li];
            match l.kind {
                LineKind::Added => l.new_no.map(|n| (true, n)).into_iter().collect::<Vec<_>>(),
                LineKind::Removed => l.old_no.map(|n| (false, n)).into_iter().collect(),
                LineKind::Context => l.new_no.map(|n| (true, n)).into_iter().chain(l.old_no.map(|n| (false, n))).collect(),
                LineKind::Meta => vec![],
            }
        };
        match row {
            Row::Line(fi, hi, li) => of(fi, hi, li),
            Row::Split(fi, hi, l, r) => {
                let mut v: Vec<(bool, u32)> = l.map(|li| of(fi, hi, li)).unwrap_or_default().into_iter().filter(|(new, _)| !new).collect();
                v.extend(r.map(|li| of(fi, hi, li)).unwrap_or_default().into_iter().filter(|(new, _)| *new));
                v
            }
            _ => vec![],
        }
    }

    /// Adds a comment row after each line that has threads or drafts.
    pub fn insert_comment_rows(&self, rows: Vec<Row>) -> Vec<Row> {
        let anchors = self.comment_anchors();
        if anchors.is_empty() {
            return rows;
        }
        let mut out = Vec::with_capacity(rows.len() + anchors.len());
        for row in rows {
            out.push(row);
            let fi = row.file();
            let mut added = vec![];
            for (new_side, line) in self.row_lines(row) {
                let key = (fi, new_side, line);
                if anchors.contains_key(&key) && !added.contains(&key) {
                    added.push(key);
                    out.push(Row::Comments(fi, new_side, line));
                }
            }
        }
        out
    }

    /// GitHub-commentable line of a row: (line, new side?, hunk). Context lines comment
    /// on the new side.
    fn commentable(&self, row: Row) -> Option<(u32, bool, usize)> {
        let files = self.files();
        let line = |fi: usize, hi: usize, li: usize| {
            let l = &files[fi].hunks[hi].lines[li];
            match l.kind {
                LineKind::Removed => l.old_no.map(|n| (n, false, hi)),
                LineKind::Added | LineKind::Context => l.new_no.map(|n| (n, true, hi)),
                LineKind::Meta => None,
            }
        };
        match row {
            Row::Line(fi, hi, li) => line(fi, hi, li),
            Row::Split(fi, hi, l, r) => r.and_then(|li| line(fi, hi, li)).or_else(|| l.and_then(|li| line(fi, hi, li))),
            _ => None,
        }
    }

    /// The comment target for the selection around row `ix` (or just that row). GitHub
    /// only takes ranges inside one hunk, so the range is cut to the last line's hunk.
    pub fn comment_target(&self, ix: usize) -> Option<CommentTarget> {
        let range = match self.selected_range() {
            Some(r) if r.contains(&ix) => r,
            _ => ix..ix + 1,
        };
        let lines: Vec<(usize, (u32, bool, usize))> =
            range.clone().filter_map(|i| Some((i, self.commentable(*self.rows.get(i)?)?))).collect();
        let &(last_ix, (end_line, end_new, hunk)) = lines.last()?;
        let in_hunk: Vec<&(usize, (u32, bool, usize))> = lines.iter().filter(|(_, (_, _, h))| *h == hunk).collect();
        let &(first_ix, (start_line, start_new, _)) = *in_hunk.first()?;
        let fi = self.rows[last_ix].file();
        let path = self.files()[fi].path.clone();
        let start = (first_ix != last_ix).then_some((start_line, start_new));
        let file = path.rsplit('/').next().unwrap_or(&path).to_string();
        let label = match start {
            Some((s, _)) => format!("{file} · lines {s}–{end_line}"),
            None => format!("{file} · line {end_line}"),
        };
        let snippet = self.rows_context(first_ix..last_ix + 1).map(|c| c.snippet).unwrap_or_default();
        Some(CommentTarget::New { path, start, end: (end_line, end_new), label, snippet })
    }
}

impl Prmator {
    /// Opens the comment box for the line (or selection) at row `ix`.
    pub fn open_comment_composer(&mut self, ix: usize, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.pr.as_ref().and_then(|pv| pv.comment_target(ix)) else { return };
        self.show_comment_popover(target, position, window, cx);
    }

    pub fn open_thread(&mut self, anchor: Anchor, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.show_comment_popover(CommentTarget::Thread { anchor }, position, window, cx);
    }

    fn show_comment_popover(&mut self, target: CommentTarget, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.hover = None;
        let placeholder = match &target {
            CommentTarget::New { .. } => "Leave a comment…",
            CommentTarget::Thread { .. } => "Reply…",
        };
        self.comment_input.update(cx, |i, cx| {
            i.set_placeholder(placeholder, cx);
        });
        self.comment_popover = Some(CommentPopover {
            position,
            target,
            busy: false,
            error: None,
            scroll: ScrollHandle::new(),
            drawn: Rc::default(),
            drag: None,
        });
        self.comment_input.read(cx).focus(window);
        cx.notify();
    }

    pub fn close_comment_popover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.comment_popover = None;
        window.focus(&self.diff_focus);
        cx.notify();
    }

    /// The comment box's bounds: where the reviewer put it, or where it was last drawn.
    pub fn comment_box(&self, window: &Window) -> Option<Bounds<Pixels>> {
        let drawn = self.comment_popover.as_ref()?.drawn.get();
        let b = self.store.comment_box.map(Bounds::from).or(drawn)?;
        Some(clamp_box(b, window.viewport_size()))
    }

    fn set_comment_box(&mut self, b: Bounds<Pixels>, window: &Window, cx: &mut Context<Self>) {
        self.store.comment_box = Some(clamp_box(b, window.viewport_size()).into());
        cx.notify();
    }

    /// Moves (`dx`, `dy`) and resizes (`dw`, `dh`) the comment box, in key-press steps.
    pub fn nudge_comment_box(&mut self, dx: f32, dy: f32, dw: f32, dh: f32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut b) = self.comment_box(window) else { return };
        b.origin += point(px(dx * BOX_STEP), px(dy * BOX_STEP));
        b.size.width += px(dw * BOX_STEP);
        b.size.height += px(dh * BOX_STEP);
        self.set_comment_box(b, window, cx);
        self.store.save();
    }

    /// Puts the box back next to its line, at its default size.
    pub fn reset_comment_box(&mut self, cx: &mut Context<Self>) {
        self.store.comment_box = None;
        self.store.save();
        cx.notify();
    }

    pub fn start_box_drag(&mut self, resize: Option<(bool, bool)>, mouse: Point<Pixels>, window: &Window) {
        let Some(b) = self.comment_box(window) else { return };
        let Some(p) = &mut self.comment_popover else { return };
        p.drag = Some(match resize {
            None => BoxDrag::Move(mouse - b.origin),
            Some((width, height)) => BoxDrag::Resize { width, height, from: mouse, start: b },
        });
    }

    pub fn drag_box_to(&mut self, mouse: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        let Some(drag) = self.comment_popover.as_ref().and_then(|p| p.drag) else { return };
        let Some(mut b) = self.comment_box(window) else { return };
        match drag {
            BoxDrag::Move(grab) => b.origin = mouse - grab,
            BoxDrag::Resize { width, height, from, start } => {
                b = start;
                if width {
                    b.size.width += mouse.x - from.x;
                }
                if height {
                    b.size.height += mouse.y - from.y;
                }
            }
        }
        self.set_comment_box(b, window, cx);
    }

    pub fn end_box_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.comment_popover {
            if p.drag.take().is_some() {
                self.store.save();
                cx.notify();
            }
        }
    }

    /// Scrolls the code and thread in the box by `pages` (negative: up).
    pub fn scroll_comment_box(&mut self, pages: f32, cx: &mut Context<Self>) {
        let Some(p) = &self.comment_popover else { return };
        let step = p.scroll.bounds().size.height * 0.8 * pages;
        let max = p.scroll.max_offset().height;
        let mut offset = p.scroll.offset();
        offset.y = (offset.y - step).min(px(0.)).max(-max);
        p.scroll.set_offset(offset);
        cx.notify();
    }

    /// The GitHub thread a reply goes to, when the box shows one.
    pub fn popover_reply_root(&self) -> Option<u64> {
        let CommentTarget::Thread { anchor } = &self.comment_popover.as_ref()?.target else { return None };
        let refs = self.pr.as_ref()?.comment_anchors().remove(anchor)?;
        refs.into_iter().find_map(|r| match r {
            ThreadRef::Remote(root) => Some(root),
            ThreadRef::Draft(_) => None,
        })
    }

    fn comment_text(&self, cx: &Context<Self>) -> String {
        self.comment_input.read(cx).text().trim().to_string()
    }

    /// The new comment the popover would make (a reply in a thread becomes a single-line
    /// comment on that line).
    fn popover_comment(&self, body: String) -> Option<DraftComment> {
        let pv = self.pr.as_ref()?;
        let target = &self.comment_popover.as_ref()?.target;
        let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0);
        Some(match target {
            CommentTarget::New { path, start, end, .. } => {
                DraftComment { id, path: path.clone(), line: end.0, new_side: end.1, start: *start, body }
            }
            CommentTarget::Thread { anchor: (fi, new_side, line) } => {
                DraftComment { id, path: pv.files()[*fi].path.clone(), line: *line, new_side: *new_side, start: None, body }
            }
        })
    }

    /// Saves the comment as a draft of the pending review.
    pub fn add_to_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.comment_text(cx);
        self.add_to_review_body(body, window, cx);
    }

    /// Same, with the text given (⌘↵ in the box clears it before we get here).
    pub fn add_to_review_body(&mut self, body: String, window: &mut Window, cx: &mut Context<Self>) {
        let body = body.trim().to_string();
        if body.is_empty() {
            return;
        }
        let Some(draft) = self.popover_comment(body) else { return };
        let Some(pv) = &mut self.pr else { return };
        pv.drafts.push(draft);
        pv.rebuild_rows();
        self.save_drafts();
        self.comment_input.update(cx, |i, cx| i.set_text("", cx));
        self.close_comment_popover(window, cx);
    }

    pub fn delete_draft(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        pv.drafts.retain(|d| d.id != id);
        pv.rebuild_rows();
        self.save_drafts();
        cx.notify();
    }

    fn save_drafts(&mut self) {
        let Some(pv) = &self.pr else { return };
        let key = pv.key.store_key();
        self.store.progress(&key).drafts = pv.drafts.clone();
        self.store.save();
    }

    /// Posts the comment right away ("Comment now"), or replies to the thread.
    pub fn post_now(&mut self, reply_to: Option<u64>, cx: &mut Context<Self>) {
        let body = self.comment_text(cx);
        if body.is_empty() {
            return;
        }
        let Some(pv) = &self.pr else { return };
        let Some(sha) = pv.detail.as_ref().map(|d| d.head_ref_oid.clone()) else { return };
        let (key, repo, number) = (pv.key.clone(), pv.key.repo.clone(), pv.key.number);
        let comment = self.popover_comment(body.clone()).map(|d| d.to_github());
        if let Some(p) = &mut self.comment_popover {
            p.busy = true;
            p.error = None;
        }
        let task = cx.background_executor().spawn(async move {
            match reply_to {
                Some(id) => github::reply_to_comment(&repo, number, id, &body),
                None => match comment {
                    Some(c) => github::post_comment(&repo, number, &sha, &c),
                    None => Ok(()),
                },
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.comment_popover = None;
                        this.comment_input.update(cx, |i, cx| i.set_text("", cx));
                        this.reload_review_data(key, cx);
                    }
                    Err(e) => {
                        if let Some(p) = &mut this.comment_popover {
                            p.busy = false;
                            p.error = Some(format!("{e:#}"));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn toggle_review_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.review_panel.take().is_none() {
            self.review_panel = Some(ReviewPanel { event: ReviewEvent::Comment, busy: false, error: None });
            self.review_input.read(cx).focus(window);
        }
        cx.notify();
    }

    /// Posts the pending review: summary, verdict and all drafts.
    pub fn submit_review(&mut self, cx: &mut Context<Self>) {
        let body = self.review_input.read(cx).text().to_string();
        self.submit_review_with(body, cx);
    }

    pub fn submit_review_with(&mut self, body: String, cx: &mut Context<Self>) {
        let Some(panel) = &self.review_panel else { return };
        let event = panel.event;
        let body = body.trim().to_string();
        let Some(pv) = &self.pr else { return };
        let Some(sha) = pv.detail.as_ref().map(|d| d.head_ref_oid.clone()) else { return };
        let comments: Vec<NewComment> = pv.drafts.iter().map(|d| d.to_github()).collect();
        if comments.is_empty() && body.is_empty() && event == ReviewEvent::Comment {
            if let Some(p) = &mut self.review_panel {
                p.error = Some("Write a summary or add comments first.".into());
            }
            return cx.notify();
        }
        let (key, repo, number) = (pv.key.clone(), pv.key.repo.clone(), pv.key.number);
        let posted: Vec<u64> = pv.drafts.iter().map(|d| d.id).collect();
        let summary = body.clone();
        if let Some(p) = &mut self.review_panel {
            p.busy = true;
            p.error = None;
        }
        let task =
            cx.background_executor().spawn(async move { github::submit_review(&repo, number, &sha, &body, event.api(), &comments) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.review_panel = None;
                        this.review_input.update(cx, |i, cx| i.set_text("", cx));
                        if let Some(pv) = this.pr.as_mut().filter(|pv| pv.key == key) {
                            pv.drafts.retain(|d| !posted.contains(&d.id));
                            pv.rebuild_rows();
                        }
                        this.save_drafts();
                        this.reload_review_data(key, cx);
                        this.reload_pr_lists(cx);
                    }
                    Err(e) => {
                        if let Some(p) = &mut this.review_panel {
                            p.busy = false;
                            p.error = Some(format!("{e:#}"));
                        }
                        // ⌘↵ cleared the box; give the summary back.
                        this.review_input.update(cx, |i, cx| i.set_text(summary.clone(), cx));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Refetches comments and reviews after posting.
    pub fn reload_review_data(&mut self, key: PrKey, cx: &mut Context<Self>) {
        let (repo, number) = (key.repo.clone(), key.number);
        let task = cx.background_executor().spawn(async move {
            (github::review_comments(&repo, number), github::pr_detail(&repo, number))
        });
        cx.spawn(async move |this, cx| {
            let (comments, detail) = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(pv) = this.pr.as_mut().filter(|pv| pv.key == key) else { return };
                if let Ok(c) = comments {
                    pv.review_comments = Load::Ready(c);
                }
                if let Ok(d) = detail {
                    pv.detail = Some(d);
                }
                pv.rebuild_rows();
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_box_stays_in_the_window() {
        let window = size(px(1000.), px(800.));
        let b = |x: f32, y: f32, w: f32, h: f32| Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) };
        // Fits: unchanged.
        assert_eq!(clamp_box(b(100., 100., 540., 400.), window), b(100., 100., 540., 400.));
        // Dragged past the bottom-right corner: pulled back in.
        assert_eq!(clamp_box(b(900., 700., 540., 400.), window), b(448., 388., 540., 400.));
        // Too small, or bigger than the window.
        assert_eq!(clamp_box(b(-50., 10., 100., 50.), window), b(12., 12., 320., 200.));
        assert_eq!(clamp_box(b(0., 0., 5000., 5000.), window), b(12., 12., 976., 776.));
    }

    #[test]
    fn drafts_become_github_comments() {
        let single = DraftComment { id: 1, path: "a.rs".into(), line: 12, new_side: true, start: None, body: "hm".into() };
        let c = single.to_github();
        assert_eq!((c.line, c.side, c.start_line, c.start_side), (12, "RIGHT", None, None));
        let json = serde_json::to_value(&c).unwrap();
        assert!(json.get("start_line").is_none(), "absent, not null: {json}");

        let multi = DraftComment { id: 2, path: "a.rs".into(), line: 20, new_side: true, start: Some((15, false)), body: "x".into() };
        let c = multi.to_github();
        assert_eq!((c.line, c.side, c.start_line, c.start_side), (20, "RIGHT", Some(15), Some("LEFT")));
        assert_eq!(multi.label(), "a.rs:15–20");

        let payload = github::review_payload("sha", "LGTM", "COMMENT", &[c]);
        assert_eq!(payload["commit_id"], "sha");
        assert_eq!(payload["event"], "COMMENT");
        assert_eq!(payload["comments"][0]["start_line"], 15);
        assert_eq!(payload["comments"][0]["path"], "a.rs");
    }
}
