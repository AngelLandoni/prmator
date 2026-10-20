//! Center column: PR header and the virtualized diff (unified or side by side).

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, uniform_list, AnyElement, ClickEvent, Context, Div, FontWeight, Hsla, MouseButton,
    MouseDownEvent, MouseMoveEvent, SharedString, Stateful,
};

use crate::analysis::SymbolChange;
use crate::app::{symbol_at, CodeHit, Command, Prmator};
use crate::diff::{FileStatus, Line, LineKind};
use crate::model::{ExpandDir, Load, PrPage, PrView, Row, EXPAND_STEP};
use crate::store::DiffMode;
use crate::theme::{theme, Theme};
use crate::*;

use super::components::*;


impl Prmator {
    pub fn render_center(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let Some(pv) = &self.pr else {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .text_color(t.muted)
                .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).text_color(t.text).child("PRmator"))
                .child("Pick a pull request on the left.")
                .child(
                    div()
                        .mt_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_xs()
                        .children(SHORTCUTS.iter().map(|(k, what)| {
                            div().flex().gap_2().items_center().child(div().w(px(70.)).flex().justify_end().child(kbd(&t, k))).child(*what)
                        })),
                )
                .into_any_element();
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.diff_focus)
            .key_context("DiffPane")
            .on_action(cx.listener(Self::next_file))
            .on_action(cx.listener(Self::prev_file))
            .on_action(cx.listener(Self::toggle_viewed))
            .on_action(cx.listener(Self::ask_selection))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::next_step))
            .on_action(cx.listener(Self::prev_step))
            .on_action(cx.listener(Self::continue_review))
            .on_action(cx.listener(|this, _: &CopySelection, _, cx| this.copy_selection(cx)))
            .on_action(cx.listener(|this, _: &NextCommit, _, cx| this.next_commit(cx)))
            .on_action(cx.listener(|this, _: &AddComment, window, cx| {
                let row = this.pr.as_ref().and_then(|pv| pv.selection.map(|(_, head)| head));
                if let Some(row) = row {
                    this.open_comment_composer(row, window.mouse_position(), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PrevCommit, _, cx| this.prev_commit(cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
            .child(self.render_page_tabs(&t, pv, cx))
            .map(|d| match pv.page {
                PrPage::Conversation => d.child(self.render_overview(&t, pv, cx)),
                PrPage::Story => d.child(self.render_story_page(&t, pv, cx)),
                PrPage::Files => d
                    .child(self.render_diff_header(&t, pv, cx))
                    .when_some(self.render_step_banner(&t, pv, cx), |d, banner| d.child(banner))
                    .child(self.render_diff(&t, pv, cx)),
            })
            .into_any_element()
    }

    /// GitHub-style tabs: Conversation (overview) and Files changed (review mode).
    fn render_page_tabs(&self, t: &Theme, pv: &PrView, cx: &mut Context<Self>) -> impl IntoElement {
        let comments = pv.detail.as_ref().map_or(0, |d| d.comments.len())
            + match &pv.review_comments {
                Load::Ready(c) => c.len(),
                _ => 0,
            };
        let files = pv.files().len();
        let tab = |id: &'static str, label: String, count: usize, page: PrPage, cx: &mut Context<Self>| {
            let active = pv.page == page;
            div()
                .id(id)
                .h_full()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .text_sm()
                .border_b_2()
                .border_color(if active { t.accent } else { gpui::transparent_black() })
                .when(active, |d| d.text_color(t.text).font_weight(FontWeight::MEDIUM))
                .when(!active, |d| d.text_color(t.muted).hover(|s| s.text_color(t.text).cursor_pointer()))
                .on_click(cx.listener(move |this, _, window, cx| this.set_page(page, window, cx)))
                .child(SharedString::from(label))
                .when(count > 0, |d| d.child(badge(count.to_string(), t.muted)))
        };
        let in_review = pv.page == PrPage::Files;
        let drafts = pv.drafts.len();
        div()
            .flex_none()
            .h(px(38.))
            .flex()
            .items_end()
            .px_3()
            .gap_1()
            .border_b_1()
            .border_color(t.border)
            .child(tab("page-tab-conversation", "💬 Conversation".into(), comments, PrPage::Conversation, cx))
            .child(tab(
                "page-tab-story",
                "📖 Story".into(),
                pv.story.as_ref().map_or(0, |s| s.commits.len()),
                PrPage::Story,
                cx,
            ))
            .child(tab("page-tab-files", "± Files changed".into(), files, PrPage::Files, cx))
            .child(div().flex_1())
            .child(div().h_full().flex().items_center().pr_2().child(if drafts > 0 {
                primary_button(t, "submit-review", format!("Submit review · {drafts}"))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_review_panel(window, cx)))
            } else {
                button(t, "submit-review", "Review ▾").on_click(cx.listener(|this, _, window, cx| this.toggle_review_panel(window, cx)))
            }))
            .child(
                div().h_full().flex().items_center().child(
                    ghost_button(t, "review-mode", if in_review { "Exit review  ⌘E" } else { "Review mode  ⌘E" })
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_review_mode(&ToggleReviewMode, window, cx))),
                ),
            )
    }

    fn render_diff_header(&self, t: &Theme, pv: &PrView, cx: &mut Context<Self>) -> impl IntoElement {
        let (branch, stats) = match &pv.detail {
            Some(d) => (
                format!("{} → {}", d.head_ref_name, d.base_ref_name),
                Some((d.additions, d.deletions, d.changed_files)),
            ),
            None => (String::new(), None),
        };
        let decision = pv.detail.as_ref().map(|d| d.review_decision.clone()).unwrap_or_default();

        // Impact summary from the code analysis.
        let (mut added, mut modified, mut removed, mut tests) = (0, 0, 0, 0);
        for c in pv.analysis.values().flat_map(|a| &a.changed) {
            if matches!(c.symbol.kind, prmator_syntax::SymbolKind::Impl) {
                continue;
            }
            if matches!(c.symbol.kind, prmator_syntax::SymbolKind::Test) {
                tests += 1;
                continue;
            }
            match c.change {
                SymbolChange::Added => added += 1,
                SymbolChange::Modified => modified += 1,
                SymbolChange::Removed => removed += 1,
            }
        }

        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .pt_3()
            .pb_2()
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .child(SharedString::from(pv.summary.title.clone())),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(t.muted)
                    .when(!pv.summary.author.login.is_empty(), |d| {
                        d.child(avatar(t, &pv.summary.author.login, 16.))
                            .child(div().text_color(t.text).child(SharedString::from(pv.summary.author.login.clone())))
                    })
                    .when(!branch.is_empty(), |d| {
                        d.child(
                            div()
                                .px_1()
                                .rounded(px(4.))
                                .bg(t.raised)
                                .font_family(t.mono.clone())
                                .child(SharedString::from(branch)),
                        )
                    })
                    .when_some(stats, |d, (a, del, files)| {
                        d.child(div().text_color(t.add_fg).child(SharedString::from(format!("+{a}"))))
                            .child(div().text_color(t.del_fg).child(SharedString::from(format!("−{del}"))))
                            .child(SharedString::from(format!("{files} files")))
                    })
                    .when(!decision.is_empty(), |d| d.child(badge(decision.replace('_', " ").to_lowercase(), t.warn)))
                    .when(added + modified + removed + tests > 0, |d| {
                        d.child(div().text_color(t.dim).child("·"))
                            .when(added > 0, |d| d.child(badge(format!("{added} new"), t.add_fg)))
                            .when(modified > 0, |d| d.child(badge(format!("{modified} changed"), t.warn)))
                            .when(removed > 0, |d| d.child(badge(format!("{removed} removed"), t.del_fg)))
                            .when(tests > 0, |d| d.child(badge(format!("{tests} tests"), t.accent)))
                    })
                    .when(pv.analysis_running, |d| d.child(spinner_text(t, "analyzing")))
                    .child(div().flex_1())
                    .child(ghost_button(t, "collapse-all", "Collapse all").on_click(
                        cx.listener(|this, _, _, cx| this.set_all_collapsed(true, cx)),
                    ))
                    .child(ghost_button(t, "expand-all", "Expand all").on_click(
                        cx.listener(|this, _, _, cx| this.set_all_collapsed(false, cx)),
                    ))
                    .child(button(t, "review-pr", "✦ Overview").on_click(
                        cx.listener(|this, _, window, cx| this.run_command(Command::ReviewWithClaude, window, cx)),
                    ))
                    .child(button(t, "open-gh", "GitHub ↗").on_click(
                        cx.listener(|this, _, window, cx| this.run_command(Command::OpenOnGitHub, window, cx)),
                    )),
            )
    }

    /// "Step 2/7 · Entry point: BatchOperation" bar while following a plan.
    fn render_step_banner(&self, t: &Theme, pv: &PrView, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let plan = pv.plan.as_ref()?;
        let i = pv.current_step?;
        let step = plan.plan.steps.get(i)?;
        let total = plan.plan.steps.len();
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .px_4()
                .py(px(6.))
                .bg(t.accent_soft)
                .border_b_1()
                .border_color(t.border)
                .text_xs()
                .child(
                    div()
                        .flex()
                        .gap(px(3.))
                        .children((0..total).map(|k| {
                            let color = if plan.done.contains(&k) {
                                t.add_fg
                            } else if k == i {
                                t.accent
                            } else {
                                t.border_strong
                            };
                            div().w(px(14.)).h(px(4.)).rounded_full().bg(color)
                        })),
                )
                .child(div().text_color(t.accent).font_weight(FontWeight::SEMIBOLD).child(SharedString::from(format!("Step {}/{}", i + 1, total))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(t.text)
                        .child(SharedString::from(step.title.clone())),
                )
                .child(ghost_button(t, "banner-prev", "[ Prev").on_click(
                    cx.listener(|this, _, window, cx| this.prev_step(&PrevStep, window, cx)),
                ))
                .child(primary_button(t, "banner-next", "Done · next ]").on_click(
                    cx.listener(|this, _, window, cx| this.next_step(&NextStep, window, cx)),
                )),
        )
    }

    pub(super) fn render_diff(&self, t: &Theme, pv: &PrView, cx: &mut Context<Self>) -> AnyElement {
        match &pv.files {
            Load::Loading => return spinner_text(t, "Loading diff…").p_4().into_any_element(),
            Load::Failed(e) => return error_text(t, e).into_any_element(),
            Load::Ready(f) if f.is_empty() => return hint(t, "This PR has no changes.").into_any_element(),
            Load::Ready(_) => {}
        }
        uniform_list(
            "diff",
            pv.rows.len(),
            cx.processor(|this, range: Range<usize>, _window, cx| {
                let t = theme(cx).clone();
                range.map(|ix| this.render_row(&t, ix, cx)).collect::<Vec<_>>()
            }),
        )
        .track_scroll(pv.scroll.clone())
        .flex_1()
        .font_family(t.mono.clone())
        .text_size(px(t.code_size))
        .into_any_element()
    }

    /// A diff row. `hits` is the code text on it, used to find the identifier under the
    /// mouse for hover cards and the right-click menu.
    fn row_base(&self, ix: usize, hits: Rc<Vec<CodeHit>>, cx: &mut Context<Self>) -> Stateful<Div> {
        let hover_hits = hits.clone();
        let row_h = theme(cx).row_h;
        div()
            .id(("row", ix))
            .relative()
            .group("diff-row")
            .h(px(row_h))
            .w_full()
            .flex()
            .items_center()
            .whitespace_nowrap()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev, window, cx| this.on_row_mouse_down(ix, ev, window, cx)))
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                this.on_row_mouse_move(ix, ev, cx);
                this.on_code_hover(symbol_at(&hover_hits, ev.position), ev.position, cx);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                let symbol = symbol_at(&hits, ev.position).map(|(name, _)| name);
                this.on_row_right_click(ix, ev, symbol, window, cx)
            }))
    }

    fn render_row(&self, t: &Theme, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(pv) = &self.pr else { return div().into_any_element() };
        let files = pv.files();
        let selected = pv.selected_range().is_some_and(|r| r.contains(&ix));
        let no_hits = Rc::new(vec![]);

        match pv.rows[ix] {
            Row::File(fi) => {
                let file = &files[fi];
                let collapsed = pv.collapsed.contains(&fi);
                let viewed = pv.viewed.contains(&fi);
                let path = match (&file.old_path, file.status) {
                    (Some(old), FileStatus::Renamed) => format!("{old} → {}", file.path),
                    _ => file.path.clone(),
                };
                let lang = prmator_syntax::detect(&file.path).map(|l| l.name());
                self.row_base(ix, no_hits, cx)
                    .px_2()
                    .gap_2()
                    .bg(t.raised)
                    .border_t_1()
                    .border_b_1()
                    .border_color(t.border)
                    .text_color(t.text)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                        if !ev.modifiers().shift {
                            this.toggle_collapsed(fi, cx)
                        }
                    }))
                    .child(div().w(px(12.)).text_color(t.muted).child(if collapsed { "▸" } else { "▾" }))
                    .child(status_letter(t, file.status))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .font_weight(FontWeight::MEDIUM)
                            .when(viewed, |d| d.text_color(t.muted))
                            .child(SharedString::from(path)),
                    )
                    .when_some(lang, |d, l| d.child(div().text_xs().text_color(t.dim).child(l)))
                    .child(div().text_color(t.add_fg).child(SharedString::from(format!("+{}", file.additions))))
                    .child(div().text_color(t.del_fg).child(SharedString::from(format!("−{}", file.deletions))))
                    .child(
                        checkbox(t, ("row-viewed", fi), viewed)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.set_viewed(fi, true, cx)
                            })),
                    )
                    .child(div().text_xs().text_color(t.muted).child("Viewed"))
                    .when(pv.can_expand(fi) && !collapsed, |d| {
                        let full = self.file_fully_expanded(fi);
                        d.child(
                            ghost_button(t, ("expand-file", fi), if full { "⇣ Hide context" } else { "⇕ Full file" })
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if full {
                                        this.collapse_context(fi, cx)
                                    } else {
                                        this.expand_file(fi, cx)
                                    }
                                })),
                        )
                    })
                    .into_any_element()
            }
            Row::Note(fi) => {
                let file = &files[fi];
                let note = if file.binary {
                    "Binary file"
                } else if file.status == FileStatus::Renamed {
                    "File renamed without changes"
                } else {
                    "No textual changes"
                };
                self.row_base(ix, no_hits, cx).pl(px(t.line_no_width() * 2. + 18.)).text_color(t.dim).child(note).into_any_element()
            }
            Row::Hunk(fi, hi) => {
                let header = files[fi].hunks[hi].header.clone();
                let (range, context) = match header.match_indices("@@").nth(1) {
                    Some((i, _)) => (header[..i + 2].to_string(), header[i + 2..].trim().to_string()),
                    None => (header.clone(), String::new()),
                };
                self.row_base(ix, no_hits, cx)
                    .pl(px(t.line_no_width() * 2. + 18.))
                    .gap_3()
                    .bg(if selected { t.selected } else { t.hunk_bg })
                    .text_color(t.hunk_fg)
                    .child(SharedString::from(range))
                    .when(!context.is_empty(), |d| d.child(div().text_color(t.muted).child(SharedString::from(context))))
                    .into_any_element()
            }
            Row::Line(fi, hi, li) => {
                let line = &files[fi].hunks[hi].lines[li];
                let (bg, gutter) = line_colors(t, line.kind, selected);
                let (sign, sign_color) = sign(t, line.kind);
                let (code, hit) = self.code_text(t, fi, hi, li, line);
                self.row_base(ix, Rc::new(vec![hit]), cx)
                    .when_some(bg, |d, bg| d.bg(bg))
                    .when(bg.is_none(), |d| d.hover(|s| s.bg(t.hover.opacity(0.5))))
                    .cursor_text()
                    .child(add_comment_button(t, ix, t.row_h, cx))
                    .child(line_number(t, line.old_no, gutter))
                    .child(line_number(t, line.new_no, gutter))
                    .child(div().w(px(18.)).flex_none().pl_1().text_color(sign_color).child(sign))
                    .child(code)
                    .into_any_element()
            }
            Row::Comments(fi, new_side, line) => {
                let anchor = (fi, new_side, line);
                let refs = pv.comment_anchors().remove(&anchor).unwrap_or_default();
                let comments = match &pv.review_comments {
                    Load::Ready(c) => c.as_slice(),
                    _ => &[],
                };
                let threads = crate::comments::remote_threads(comments);
                let mut summary = String::new();
                let mut count = 0;
                let mut pending = 0;
                for r in &refs {
                    match r {
                        crate::comments::ThreadRef::Remote(root) => {
                            if let Some(th) = threads.iter().find(|t| t[0].id == *root) {
                                count += th.len();
                                if summary.is_empty() {
                                    let who = th[0].user.as_ref().map_or("ghost", |u| u.login.as_str());
                                    summary = format!("{who}: {}", first_line(&th[0].body));
                                }
                            }
                        }
                        crate::comments::ThreadRef::Draft(id) => {
                            pending += 1;
                            if summary.is_empty() {
                                if let Some(d) = pv.drafts.iter().find(|d| d.id == *id) {
                                    summary = format!("you (pending): {}", first_line(&d.body));
                                }
                            }
                        }
                    }
                }
                self.row_base(ix, no_hits, cx)
                    .pl(px(t.line_no_width() * 2. + 18.))
                    .gap_2()
                    .bg(t.accent_soft)
                    .border_l_2()
                    .border_color(if pending > 0 { t.warn } else { t.accent })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        let at = match ev {
                            ClickEvent::Mouse(m) => m.down.position,
                            _ => window.mouse_position(),
                        };
                        this.open_thread(anchor, at, window, cx)
                    }))
                    .child(div().text_color(t.accent).child("💬"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .font_family(t.ui_font.clone())
                            .text_color(t.text)
                            .child(SharedString::from(summary)),
                    )
                    .when(count > 1, |d| d.child(div().text_color(t.muted).child(SharedString::from(format!("{count} comments")))))
                    .when(pending > 0, |d| d.child(badge(format!("{pending} pending"), t.warn)))
                    .child(div().pr_2())
                    .into_any_element()
            }
            Row::Elsewhere(fi, hi, commit) => {
                let header = files[fi].hunks[hi].header.clone();
                let range = header.match_indices("@@").nth(1).map_or(header.clone(), |(i, _)| header[..i + 2].to_string());
                let title = pv
                    .story
                    .as_ref()
                    .and_then(|s| s.commits.get(commit))
                    .map(|c| c.title.clone())
                    .unwrap_or_default();
                self.row_base(ix, no_hits, cx)
                    .pl(px(t.line_no_width() * 2. + 18.))
                    .gap_2()
                    .bg(t.filler)
                    .text_color(t.dim)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.go_to_commit(commit, cx)))
                    .child(SharedString::from(range))
                    .child(
                        div()
                            .text_color(t.accent)
                            .child(SharedString::from(format!("↪ part of commit {}: {title}", commit + 1))),
                    )
                    .into_any_element()
            }
            Row::Expand(fi, k) => {
                let last = k == files[fi].hunks.len();
                let hidden = pv.gap_hidden(fi, k);
                let status = match pv.sources.get(&fi) {
                    Some(Load::Loading) => Some("loading…".to_string()),
                    Some(Load::Failed(e)) => Some(format!("couldn't load file: {e}")),
                    _ => None,
                };
                let label = match hidden {
                    Some(1) => "1 hidden line".to_string(),
                    Some(n) => format!("{n} hidden lines"),
                    None => "more lines".to_string(),
                };
                let few = hidden.is_some_and(|n| n <= EXPAND_STEP);
                let expander = |id: &'static str, label: &'static str, dir: ExpandDir, cx: &mut Context<Self>| {
                    ghost_button(t, (id, ix), label)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.expand_gap(fi, k, dir, cx)
                        }))
                };
                let mut buttons = div().w(px(t.line_no_width() * 2. + 18.)).flex_none().flex().justify_center().gap_1();
                if few {
                    buttons = buttons.child(expander("exp-all", "↕", ExpandDir::All, cx));
                } else {
                    if k > 0 {
                        buttons = buttons.child(expander("exp-down", "↓", ExpandDir::Down, cx));
                    }
                    if !last {
                        buttons = buttons.child(expander("exp-up", "↑", ExpandDir::Up, cx));
                    }
                }
                self.row_base(ix, no_hits, cx)
                    .bg(t.hunk_bg)
                    .text_color(t.hunk_fg)
                    .gap_2()
                    .child(buttons)
                    .child(div().text_xs().child(SharedString::from(status.unwrap_or(label))))
                    .when(!few && hidden.is_some(), |d| d.child(expander("exp-rest", "Expand all", ExpandDir::All, cx)))
                    .into_any_element()
            }
            Row::Context(fi, k, n) => {
                let line = pv.context_line(fi, k, n);
                match pv.mode {
                    DiffMode::Unified => {
                        let (code, hit) = self.code_text(t, fi, usize::MAX, 0, &line);
                        self.row_base(ix, Rc::new(vec![hit]), cx)
                            .when(selected, |d| d.bg(t.selected))
                            .when(!selected, |d| d.hover(|s| s.bg(t.hover.opacity(0.5))))
                            .cursor_text()
                            .child(line_number(t, line.old_no, None))
                            .child(line_number(t, line.new_no, None))
                            .child(div().w(px(18.)).flex_none())
                            .child(code)
                            .into_any_element()
                    }
                    DiffMode::Split => {
                        let (left_code, left_hit) = self.code_text(t, fi, usize::MAX, 0, &line);
                        let (right_code, right_hit) = self.code_text(t, fi, usize::MAX, 0, &line);
                        let half = |no: Option<u32>, code: Div| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .flex()
                                .items_center()
                                .overflow_hidden()
                                .child(line_number(t, no, None))
                                .child(div().w(px(16.)).flex_none())
                                .child(code)
                        };
                        self.row_base(ix, Rc::new(vec![left_hit, right_hit]), cx)
                            .when(selected, |d| d.bg(t.selected))
                            .cursor_text()
                            .child(half(line.old_no, left_code).border_r_1().border_color(t.border))
                            .child(half(line.new_no, right_code))
                            .into_any_element()
                    }
                }
            }
            Row::Split(fi, hi, l, r) => {
                let hunk = &files[fi].hunks[hi];
                let mut hits = vec![];
                let mut half = |side: usize, li: Option<usize>| -> Div {
                    let cell = div().flex_1().min_w_0().h_full().flex().items_center().overflow_hidden();
                    match li {
                        None => cell.bg(t.filler),
                        Some(li) => {
                            let line = &hunk.lines[li];
                            let (bg, gutter) = line_colors(t, line.kind, selected);
                            let (sign, sign_color) = sign(t, line.kind);
                            let no = if side == 0 { line.old_no } else { line.new_no };
                            let (code, hit) = self.code_text(t, fi, hi, li, line);
                            hits.push(hit);
                            cell.when_some(bg, |d, bg| d.bg(bg))
                                .child(line_number(t, no, gutter))
                                .child(div().w(px(16.)).flex_none().pl_1().text_color(sign_color).child(sign))
                                .child(code)
                        }
                    }
                };
                let left = half(0, l);
                let right = half(1, r);
                self.row_base(ix, Rc::new(hits), cx)
                    .cursor_text()
                    .child(add_comment_button(t, ix, t.row_h, cx))
                    .child(left.border_r_1().border_color(t.border))
                    .child(right)
                    .into_any_element()
            }
        }
    }

    /// Syntax-colored code, plus what's needed to hit-test it later.
    fn code_text(&self, t: &Theme, fi: usize, hi: usize, li: usize, line: &Line) -> (Div, CodeHit) {
        let pv = self.pr.as_ref().unwrap();
        let analysis = pv.analysis.get(&fi);
        let spans = analysis.and_then(|a| match line.kind {
            LineKind::Removed => line.old_no.and_then(|n| a.old.highlights.get(&n)),
            _ => line.new_no.and_then(|n| a.new.highlights.get(&n)),
        });
        let emphasis_bg = if line.kind == LineKind::Removed { t.del_word } else { t.add_word };
        let (display, styled, syntax) =
            code_line_with_spans(t, &line.text, spans, pv.emphasis.get(&(fi, hi, li)), emphasis_bg);
        let color = if line.kind == LineKind::Meta { t.dim } else { t.text };
        let hit = CodeHit { layout: styled.layout().clone(), text: display, syntax };
        (div().flex_1().min_w_0().overflow_hidden().text_color(color).child(styled), hit)
    }
}

fn line_colors(t: &Theme, kind: LineKind, selected: bool) -> (Option<Hsla>, Option<Hsla>) {
    if selected {
        return (Some(t.selected), Some(t.selected));
    }
    match kind {
        LineKind::Added => (Some(t.add_bg), Some(t.add_gutter)),
        LineKind::Removed => (Some(t.del_bg), Some(t.del_gutter)),
        _ => (None, None),
    }
}

fn sign(t: &Theme, kind: LineKind) -> (&'static str, Hsla) {
    match kind {
        LineKind::Added => ("+", t.add_fg),
        LineKind::Removed => ("−", t.del_fg),
        LineKind::Context => (" ", t.dim),
        LineKind::Meta => ("\\", t.dim),
    }
}

fn line_number(t: &Theme, n: Option<u32>, gutter: Option<Hsla>) -> Div {
    div()
        .w(px(t.line_no_width()))
        .h_full()
        .flex_none()
        .pr_2()
        .flex()
        .items_center()
        .justify_end()
        .when_some(gutter, |d, g| d.bg(g))
        .text_color(t.line_no)
        .text_size(px(t.code_size * 0.85))
        .child(SharedString::from(n.map(|n| n.to_string()).unwrap_or_default()))
}

const SHORTCUTS: &[(&str, &str)] = &[
    ("⇧⌘O", "search symbols in the PR"),
    ("⌘P", "go to file / PR"),
    ("⇧⌘P", "commands"),
    ("⌘F", "search the diff"),
    ("c", "continue review (plan)"),
    ("] / [", "next / previous plan step"),
    ("j / k", "next / previous file"),
    ("v", "mark file viewed"),
    ("s", "unified ⇄ side by side"),
    ("a", "ask Claude about selection"),
];

fn first_line(body: &str) -> String {
    body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("<!--")).unwrap_or_default().chars().take(140).collect()
}

/// The "+" in the gutter that appears when hovering a line: comment on it.
fn add_comment_button(t: &Theme, ix: usize, row_h: f32, cx: &mut Context<Prmator>) -> impl IntoElement {
    div()
        .id(("add-comment", ix))
        .absolute()
        .left(px(2.))
        .top(px(((row_h - 16.) / 2.).max(0.)))
        .size(px(16.))
        .rounded(px(4.))
        .bg(t.accent)
        .text_color(t.accent_fg)
        .text_size(px(12.))
        .font_weight(FontWeight::BOLD)
        .flex()
        .items_center()
        .justify_center()
        .opacity(0.)
        .group_hover("diff-row", |s| s.opacity(1.))
        .cursor_pointer()
        .child("+")
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
            let at = match ev {
                ClickEvent::Mouse(m) => m.down.position,
                _ => window.mouse_position(),
            };
            this.open_comment_composer(ix, at, window, cx)
        }))
}
