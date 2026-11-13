//! The file view: the project tree on the left and a whole file (PR head, no diff) in the
//! center. Code rows select by token and have the same hover cards and right-click menu as
//! the diff.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, uniform_list, AnyElement, Context, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent,
    SharedString,
};

use crate::app::{symbol_at, CodeHit, CodeView, Prmator};
use crate::browser::{FileBrowser, LineMark};
use crate::model::Load;
use crate::theme::{theme, Theme};
use crate::tree::TreeRow;
use crate::*;

use super::components::*;
use super::diff_view::line_number;
use super::sidebar::{folder_icon, tree_indent};

impl Prmator {
    /// Left column while the file view is open: the repo's files at the PR head.
    pub fn render_browser_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let b = self.browser.as_ref().unwrap();
        let repo = b.key.repo.rsplit('/').next().unwrap_or_default().to_string();
        let body: AnyElement = match &b.tree {
            Load::Loading => spinner_text(&t, "Loading project tree…").p_4().into_any_element(),
            Load::Failed(e) => error_text(&t, e).into_any_element(),
            Load::Ready(_) => uniform_list(
                "project-tree",
                b.tree_rows.len(),
                cx.processor(|this, range: Range<usize>, _window, cx| {
                    let t = theme(cx).clone();
                    range.map(|ix| this.render_tree_row(&t, ix, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(b.tree_scroll.clone())
            .flex_1()
            .py_1()
            .into_any_element(),
        };
        let truncated = matches!(&b.tree, Load::Ready((_, true)));
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .border_b_1()
                    .border_color(t.border)
                    .child(ghost_button(&t, "browser-back", "←").on_click(
                        cx.listener(|this, _, window, cx| this.close_browser(window, cx)),
                    ))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(SharedString::from(repo)))
                    .child(
                        div()
                            .px_1()
                            .rounded(px(4.))
                            .bg(t.raised)
                            .text_xs()
                            .font_family(t.mono.clone())
                            .text_color(t.muted)
                            .child(SharedString::from(b.sha.chars().take(7).collect::<String>())),
                    ),
            )
            .when(truncated, |d| d.child(hint(&t, "Large repo: GitHub returned part of the tree.").px_3().py_1()))
            .child(body)
    }

    fn render_tree_row(&self, t: &Theme, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(b) = &self.browser else { return div().into_any_element() };
        let paths = b.paths();
        let row = div()
            .id(("tree-row", ix))
            .flex()
            .items_center()
            .gap_2()
            .mx_1()
            .pr_2()
            .py(px(3.))
            .rounded_md()
            .text_xs()
            .whitespace_nowrap()
            .hover(|s| s.bg(t.hover).cursor_pointer());
        match &b.tree_rows[ix] {
            TreeRow::Dir { path, name, depth, .. } => {
                let open = b.expanded.contains(path);
                // Folders holding changed files get a dot, so the PR is easy to find.
                let changed = b.changed_dirs.contains(path);
                let path = path.clone();
                row.pl(px(tree_indent(*depth)))
                    .on_click(cx.listener(move |this, _, _, cx| this.browser_toggle_dir(&path, cx)))
                    .child(div().w(px(10.)).flex_none().text_color(t.dim).child(if open { "▾" } else { "▸" }))
                    .child(folder_icon(t, open))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_color(t.text).child(SharedString::from(name.clone())))
                    .when(changed, |d| d.child(div().size(px(6.)).flex_none().rounded_full().bg(t.warn)))
                    .into_any_element()
            }
            TreeRow::File { fi, name, depth } => {
                let path = paths[*fi].clone();
                let current = b.path.as_deref() == Some(path.as_str());
                let status = b.changed.get(&path).copied();
                row.pl(px(tree_indent(*depth) + 18.))
                    .when(current, |d| d.bg(t.accent_soft))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.diff_focus);
                        this.browser_open_file(path.clone(), None, cx)
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_color(if status.is_some() { t.text } else { t.muted })
                            .when(current || status.is_some(), |d| d.font_weight(FontWeight::MEDIUM))
                            .child(SharedString::from(name.clone())),
                    )
                    .when_some(status, |d, s| d.child(status_letter(t, s)))
                    .into_any_element()
            }
        }
    }

    /// Center column while the file view is open.
    pub fn render_browser_source(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let b = self.browser.as_ref().unwrap();
        let body: AnyElement = match b.path.as_ref().and_then(|p| b.files.get(p)) {
            None => hint(&t, "Pick a file in the project tree.").p_4().into_any_element(),
            Some(Load::Loading) => spinner_text(&t, "Loading file…").p_4().into_any_element(),
            Some(Load::Failed(e)) => error_text(&t, e).into_any_element(),
            Some(Load::Ready(file)) => uniform_list(
                "source",
                file.lines.len(),
                cx.processor(|this, range: Range<usize>, _window, cx| {
                    let t = theme(cx).clone();
                    range.map(|ix| this.render_source_row(&t, ix, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(b.scroll.clone())
            .flex_1()
            .font_family(t.mono.clone())
            .text_size(px(t.code_size))
            .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.diff_focus)
            .key_context("SourceView")
            .on_action(cx.listener(Self::ask_selection))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(|this, _: &CopySelection, _, cx| this.copy_selection(cx)))
            .on_action(cx.listener(|this, _: &ToggleFileView, window, cx| this.close_browser(window, cx)))
            .on_action(cx.listener(|this, _: &TokenLeft, _, cx| this.move_tokens(crate::selection::Dir::Left, false, cx)))
            .on_action(cx.listener(|this, _: &TokenRight, _, cx| this.move_tokens(crate::selection::Dir::Right, false, cx)))
            .on_action(cx.listener(|this, _: &TokenUp, _, cx| this.move_tokens(crate::selection::Dir::Up, false, cx)))
            .on_action(cx.listener(|this, _: &TokenDown, _, cx| this.move_tokens(crate::selection::Dir::Down, false, cx)))
            .on_action(cx.listener(|this, _: &ExtendLeft, _, cx| this.move_tokens(crate::selection::Dir::Left, true, cx)))
            .on_action(cx.listener(|this, _: &ExtendRight, _, cx| this.move_tokens(crate::selection::Dir::Right, true, cx)))
            .on_action(cx.listener(|this, _: &ExtendUp, _, cx| this.move_tokens(crate::selection::Dir::Up, true, cx)))
            .on_action(cx.listener(|this, _: &ExtendDown, _, cx| this.move_tokens(crate::selection::Dir::Down, true, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
            .child(self.render_source_header(&t, b, cx))
            .child(body)
    }

    fn render_source_header(&self, t: &Theme, b: &FileBrowser, cx: &mut Context<Self>) -> impl IntoElement {
        let path = b.path.clone().unwrap_or_default();
        let status = b.changed.get(&path).copied();
        let diff = self.pr.as_ref().and_then(|pv| pv.files().iter().find(|f| f.path == path));
        let lines = b.current().map(|f| f.lines.len());
        let url = self.pr.as_ref().map(|pv| {
            let line = b.selected_range().map(|r| format!("#L{}", r.start + 1)).unwrap_or_default();
            format!("https://github.com/{}/blob/{}/{}{line}", pv.key.repo, b.sha, path)
        });
        div()
            .flex_none()
            .h(px(38.))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(t.border)
            .text_xs()
            .child(
                ghost_button(t, "source-back", "← Back to review  f")
                    .on_click(cx.listener(|this, _, window, cx| this.close_browser(window, cx))),
            )
            .child(div().w(px(1.)).h(px(16.)).bg(t.border))
            .when_some(status, |d, s| d.child(status_letter(t, s)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_family(t.mono.clone())
                    .text_color(t.text)
                    .child(SharedString::from(path)),
            )
            .when_some(diff, |d, f| {
                d.child(div().text_color(t.add_fg).child(SharedString::from(format!("+{}", f.additions))))
                    .child(div().text_color(t.del_fg).child(SharedString::from(format!("−{}", f.deletions))))
            })
            .when(status.is_none() && b.path.is_some(), |d| d.child(badge("unchanged", t.muted)))
            .when_some(lines, |d, n| d.child(div().text_color(t.dim).child(SharedString::from(format!("{n} lines")))))
            .when_some(url.filter(|_| b.path.is_some()), |d, url| {
                d.child(ghost_button(t, "source-gh", "GitHub ↗").on_click(move |_, _, _| crate::github::open_in_browser(&url)))
            })
    }

    fn render_source_row(&self, t: &Theme, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(file) = self.browser.as_ref().and_then(|b| b.current()) else { return div().into_any_element() };
        let b = self.browser.as_ref().unwrap();
        let text = file.lines.get(ix).map(String::as_str).unwrap_or_default();
        let tokens = b.token_sel();
        let row_selected = tokens.is_none() && b.selected_range().is_some_and(|r| r.contains(&ix));
        let (display, styled, syntax) = code_line_with_spans(t, text, file.highlights.get(ix), None, t.add_word);
        let shape = tokens.and_then(|s| s.shape_on(ix, 0, |r| b.row_text(r, 0)));
        let bubble = shape.map(|shape| selection_bubble(t, styled.layout().clone(), shape));
        let hits = Rc::new(vec![CodeHit { layout: styled.layout().clone(), text: display, syntax, side: 0 }]);
        let (down_hits, move_hits) = (hits.clone(), hits.clone());
        let mark = file.marks.get(&(ix as u32 + 1)).copied();
        let mark_bar = div().w(px(3.)).h_full().flex_none().map(|d| match mark {
            Some(LineMark::Added) => d.bg(t.add_fg),
            Some(LineMark::Modified) => d.bg(t.warn),
            Some(LineMark::RemovedAbove) => d.child(div().w_full().h(px(3.)).bg(t.del_fg)),
            None => d,
        });
        div()
            .id(("src-row", ix))
            .h(px(t.row_h))
            .w_full()
            .flex()
            .items_center()
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_text()
            .when(row_selected, |d| d.bg(t.selected))
            .when(!row_selected && matches!(mark, Some(LineMark::Added | LineMark::Modified)), |d| d.bg(t.add_bg.opacity(0.45)))
            .when(!row_selected, |d| d.hover(|s| s.bg(t.hover.opacity(0.5))))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev, window, cx| {
                this.on_code_mouse_down(CodeView::Source, ix, ev, &down_hits, window, cx)
            }))
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                this.on_code_mouse_move(CodeView::Source, ix, ev, &move_hits, cx);
                this.on_code_hover(symbol_at(&move_hits, ev.position), ev.position, cx);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                this.on_row_right_click(CodeView::Source, ix, ev, &hits, window, cx)
            }))
            .child(mark_bar)
            .child(line_number(t, Some(ix as u32 + 1), None))
            .child(div().w(px(9.)).flex_none())
            .child(div().relative().h_full().flex().items_center().flex_1().min_w_0().pl(px(3.)).overflow_hidden().text_color(t.text).children(bubble).child(styled))
            .into_any_element()
    }
}
