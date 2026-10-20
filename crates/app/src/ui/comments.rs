//! Review comment UI: the floating comment box on diff lines (new comment, or a thread
//! with replies and drafts) and the "Submit review" panel.

use gpui::{
    anchored, canvas, deferred, div, point, prelude::*, px, AnyElement, Context, CursorStyle, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Point, Pixels, SharedString, Window,
};

use crate::app::Prmator;
use crate::comments::{remote_threads, CommentTarget, ReviewEvent, ThreadRef};
use crate::github;
use crate::model::Load;
use crate::theme::{theme, Theme};
use crate::{
    CloseCommentPopover, CloseReviewPanel, CommentBoxDown, CommentBoxGrow, CommentBoxLeft, CommentBoxNarrower,
    CommentBoxReset, CommentBoxRight, CommentBoxShorter, CommentBoxShrink, CommentBoxTaller, CommentBoxUp,
    CommentBoxWider, CommentPostNow, CommentScrollDown, CommentScrollUp,
};

use super::components::*;
use super::markdown::{self, MdSize};

const POPOVER_WIDTH: f32 = 540.;

impl Prmator {
    pub fn render_comment_popover(&self, window: &Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let t = theme(cx).clone();
        let p = self.comment_popover.as_ref()?;
        let pv = self.pr.as_ref()?;

        // What the box is about, and the reply target if it's an existing thread.
        let (title, body, reply_to): (String, AnyElement, Option<u64>) = match &p.target {
            CommentTarget::New { label, snippet, .. } => (label.clone(), snippet_box(&t, snippet).into_any_element(), None),
            CommentTarget::Thread { anchor } => {
                let (fi, _, line) = *anchor;
                let file = pv.files().get(fi).map(|f| f.path.clone()).unwrap_or_default();
                let refs = pv.comment_anchors().remove(anchor).unwrap_or_default();
                let comments = match &pv.review_comments {
                    Load::Ready(c) => c.as_slice(),
                    _ => &[],
                };
                let threads = remote_threads(comments);
                let mut first_remote = None;
                let mut items: Vec<AnyElement> = vec![];
                for (i, r) in refs.iter().enumerate() {
                    match r {
                        ThreadRef::Remote(root) => {
                            first_remote.get_or_insert(*root);
                            if let Some(thread) = threads.iter().find(|t| t[0].id == *root) {
                                for (j, c) in thread.iter().enumerate() {
                                    let author = c.user.as_ref().map_or("ghost", |u| u.login.as_str());
                                    items.push(comment_block(&t, author, &github::relative_time(&c.created_at), None, &c.body, &format!("cp-{i}-{j}")).into_any_element());
                                }
                            }
                        }
                        ThreadRef::Draft(id) => {
                            if let Some(d) = pv.drafts.iter().find(|d| d.id == *id) {
                                let id = *id;
                                items.push(
                                    comment_block(&t, "you", "", Some(badge("Pending", t.warn).into_any_element()), &d.body, &format!("cpd-{i}"))
                                        .child(div().flex().justify_end().child(
                                            ghost_button(&t, SharedString::from(format!("draft-del-{id}")), "Delete draft")
                                                .on_click(cx.listener(move |this, _, _, cx| this.delete_draft(id, cx))),
                                        ))
                                        .into_any_element(),
                                );
                            }
                        }
                    }
                }
                let short = file.rsplit('/').next().unwrap_or(&file).to_string();
                (format!("{short} · line {line}"), div().flex().flex_col().children(items).into_any_element(), first_remote)
            }
        };

        let busy = p.busy;
        // Placed by the reviewer (dragged, resized or moved with keys), or next to the line.
        let placed = self.store.comment_box.and_then(|_| self.comment_box(window));
        let drawn = p.drawn.clone();
        let card = div()
            .id("comment-popover")
            .key_context("CommentPopover")
            .on_action(cx.listener(|this, _: &CloseCommentPopover, window, cx| this.close_comment_popover(window, cx)))
            .on_action(cx.listener(|this, _: &CommentPostNow, _, cx| this.post_now(this.popover_reply_root(), cx)))
            .on_action(cx.listener(|this, _: &CommentBoxLeft, window, cx| this.nudge_comment_box(-1., 0., 0., 0., window, cx)))
            .on_action(cx.listener(|this, _: &CommentBoxRight, window, cx| this.nudge_comment_box(1., 0., 0., 0., window, cx)))
            .on_action(cx.listener(|this, _: &CommentBoxUp, window, cx| this.nudge_comment_box(0., -1., 0., 0., window, cx)))
            .on_action(cx.listener(|this, _: &CommentBoxDown, window, cx| this.nudge_comment_box(0., 1., 0., 0., window, cx)))
            .on_action(cx.listener(|this, _: &CommentBoxWider, window, cx| this.nudge_comment_box(0., 0., 1., 0., window, cx)))
            .on_action(cx.listener(|this, _: &CommentBoxNarrower, window, cx| this.nudge_comment_box(0., 0., -1., 0., window, cx)))
            .on_action(cx.listener(|this, _: &CommentBoxTaller, window, cx| this.nudge_comment_box(0., 0., 0., 1., window, cx)))
            .on_action(cx.listener(|this, _: &CommentBoxShorter, window, cx| this.nudge_comment_box(0., 0., 0., -1., window, cx)))
            .on_action(cx.listener(|this, _: &CommentBoxGrow, window, cx| this.nudge_comment_box(-0.5, -0.5, 1., 1., window, cx)))
            .on_action(cx.listener(|this, _: &CommentBoxShrink, window, cx| this.nudge_comment_box(0.5, 0.5, -1., -1., window, cx)))
            .on_action(cx.listener(|this, _: &CommentBoxReset, _, cx| this.reset_comment_box(cx)))
            .on_action(cx.listener(|this, _: &CommentScrollUp, _, cx| this.scroll_comment_box(-1., cx)))
            .on_action(cx.listener(|this, _: &CommentScrollDown, _, cx| this.scroll_comment_box(1., cx)))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .relative()
            .map(|d| match placed {
                Some(b) => d.w(b.size.width).h(b.size.height),
                None => d.w(px(POPOVER_WIDTH)).max_h(px(640.)),
            })
            .flex()
            .flex_col()
            .rounded_xl()
            .bg(t.panel)
            .border_1()
            .border_color(t.border_strong)
            .shadow_lg()
            .overflow_hidden()
            // Remembers where the box is on screen, for moving and resizing.
            .child(canvas(move |b, _, _| drawn.set(Some(b)), |_, _, _, _| {}).absolute().size_full())
            // While dragging, follow the mouse anywhere in the window.
            .when(p.drag.is_some(), |d| {
                let this = cx.entity().downgrade();
                d.child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let moved = this.clone();
                            window.on_mouse_event(move |ev: &MouseMoveEvent, _, window, cx| {
                                let _ = moved.update(cx, |this, cx| match ev.pressed_button {
                                    Some(MouseButton::Left) => this.drag_box_to(ev.position, window, cx),
                                    // Released outside the window.
                                    _ => this.end_box_drag(cx),
                                });
                            });
                            let ended = this.clone();
                            window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                                let _ = ended.update(cx, |this, cx| this.end_box_drag(cx));
                            });
                        },
                    )
                    .absolute()
                    .size_0(),
                )
            })
            .child(
                div()
                    .id("comment-header")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(t.raised)
                    .border_b_1()
                    .border_color(t.border)
                    .cursor(CursorStyle::OpenHand)
                    // Drag to move; double-click puts it back next to the line.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                            if ev.click_count == 2 {
                                this.reset_comment_box(cx);
                            } else {
                                this.start_box_drag(None, ev.position, window);
                            }
                        }),
                    )
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("💬"))
                    .child(div().flex_1().text_sm().font_family(t.mono.clone()).text_color(t.accent).child(SharedString::from(title)))
                    .child(
                        ghost_button(&t, "comment-close", "✕")
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, window, cx| this.close_comment_popover(window, cx))),
                    ),
            )
            .child(div().id("comment-body").flex_1().min_h(px(0.)).overflow_y_scroll().track_scroll(&p.scroll).child(body))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_t_1()
                    .border_color(t.border)
                    .child(div().rounded_lg().bg(t.input_bg).border_1().border_color(t.border_strong).child(self.comment_input.clone()))
                    .when_some(p.error.clone(), |d, e| d.child(error_text(&t, &e)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().text_xs().text_color(t.dim).child("Markdown · ⌘↵ add to review · ⇧⌘↵ post now"))
                            .when(busy, |d| d.child(spinner_text(&t, "Posting…")))
                            .map(|d| match reply_to {
                                Some(root) => d.child(button(&t, "comment-reply", "Reply now").on_click(
                                    cx.listener(move |this, _, _, cx| this.post_now(Some(root), cx)),
                                )),
                                None => d.child(button(&t, "comment-now", "Comment now").on_click(
                                    cx.listener(|this, _, _, cx| this.post_now(None, cx)),
                                )),
                            })
                            .child(primary_button(&t, "comment-add", "Add to review").on_click(
                                cx.listener(|this, _, window, cx| this.add_to_review(window, cx)),
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.dim)
                            .child("⌥↑↓ scroll · ⌥⌘ arrows move · ⇧⌥⌘ arrows resize · ⌘= ⌘− size · ⌘0 reset"),
                    ),
            )
            // Resize handles: right edge, bottom edge, bottom-right corner.
            .child(resize_handle("comment-resize-w", (true, false), CursorStyle::ResizeLeftRight, cx).top_0().right_0().w(px(6.)).h_full())
            .child(resize_handle("comment-resize-h", (false, true), CursorStyle::ResizeUpDown, cx).bottom_0().left_0().h(px(6.)).w_full())
            .child(
                resize_handle("comment-resize-wh", (true, true), CursorStyle::ResizeUpLeftDownRight, cx)
                    .bottom_0()
                    .right_0()
                    .size(px(14.))
                    .flex()
                    .items_end()
                    .justify_end()
                    .pr(px(3.))
                    .text_xs()
                    .text_color(t.dim)
                    .child("◢"),
            );
        let at = match placed {
            Some(b) => b.origin,
            None => p.position + point(px(0.), px(10.)),
        };
        Some(deferred(anchored().position(at).snap_to_window_with_margin(px(12.)).child(card)).with_priority(1))
    }

    pub fn render_review_panel(&self, at: Point<Pixels>, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let t = theme(cx).clone();
        let panel = self.review_panel.as_ref()?;
        let pv = self.pr.as_ref()?;
        let event = panel.event;
        // Can't approve or request changes on your own PR; GitHub would refuse.
        let own_pr = pv.detail.as_ref().is_some_and(|d| self.viewer.as_deref() == Some(d.author.login.as_str()));
        let card = div()
            .id("review-panel")
            .key_context("ReviewPanel")
            .on_action(cx.listener(|this, _: &CloseReviewPanel, _, cx| {
                this.review_panel = None;
                cx.notify();
            }))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .w(px(520.))
            .max_h(px(620.))
            .flex()
            .flex_col()
            .rounded_xl()
            .bg(t.panel)
            .border_1()
            .border_color(t.border_strong)
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(t.border)
                    .child(div().flex_1().text_base().font_weight(FontWeight::SEMIBOLD).child("Finish your review"))
                    .child(ghost_button(&t, "review-close", "✕").on_click(cx.listener(|this, _, _, cx| {
                        this.review_panel = None;
                        cx.notify();
                    }))),
            )
            .child(
                div()
                    .id("review-drafts")
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .px_4()
                    .py_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(t.muted).child(SharedString::from(match pv.drafts.len() {
                        0 => "No pending comments".to_string(),
                        1 => "1 pending comment".to_string(),
                        n => format!("{n} pending comments"),
                    })))
                    .children(pv.drafts.iter().map(|d| {
                        let id = d.id;
                        let (path, line, new_side) = (d.path.clone(), d.line, d.new_side);
                        div()
                            .id(SharedString::from(format!("review-draft-{id}")))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .hover(|s| s.bg(t.hover).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.review_panel = None;
                                if let Some(pv) = &mut this.pr {
                                    if let Some(fi) = pv.file_index(&path) {
                                        pv.jump_to_lines(fi, new_side, line..line + 1);
                                    }
                                }
                                cx.notify();
                            }))
                            .child(div().flex_none().text_xs().font_family(t.mono.clone()).text_color(t.accent).child(SharedString::from(d.label())))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_sm()
                                    .child(SharedString::from(d.body.lines().next().unwrap_or_default().to_string())),
                            )
                            .child(ghost_button(&t, SharedString::from(format!("review-draft-del-{id}")), "✕").on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.delete_draft(id, cx)
                                },
                            )))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(t.border)
                    .child(div().rounded_lg().bg(t.input_bg).border_1().border_color(t.border_strong).child(self.review_input.clone()))
                    .child(
                        segmented(&t)
                            .child(segment(&t, "event-comment", "💬 Comment", event == ReviewEvent::Comment).on_click(cx.listener(
                                |this, _, _, cx| {
                                    if let Some(p) = &mut this.review_panel {
                                        p.event = ReviewEvent::Comment;
                                    }
                                    cx.notify();
                                },
                            )))
                            .when(!own_pr, |d| {
                                d.child(segment(&t, "event-approve", "✓ Approve", event == ReviewEvent::Approve).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        if let Some(p) = &mut this.review_panel {
                                            p.event = ReviewEvent::Approve;
                                        }
                                        cx.notify();
                                    },
                                )))
                                .child(segment(&t, "event-changes", "± Request changes", event == ReviewEvent::RequestChanges).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(p) = &mut this.review_panel {
                                            p.event = ReviewEvent::RequestChanges;
                                        }
                                        cx.notify();
                                    }),
                                ))
                            }),
                    )
                    .when_some(panel.error.clone(), |d, e| d.child(error_text(&t, &e)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().text_xs().text_color(t.dim).child("Posted to GitHub as one review · ⌘↵ submit"))
                            .when(panel.busy, |d| d.child(spinner_text(&t, "Submitting…")))
                            .child(primary_button(&t, "review-submit", "Submit review").on_click(cx.listener(|this, _, _, cx| this.submit_review(cx)))),
                    ),
            );
        Some(deferred(anchored().position(at).snap_to_window_with_margin(px(12.)).child(card)).with_priority(1))
    }
}

/// A strip along the comment box's edge that resizes it when dragged.
fn resize_handle(id: &'static str, sides: (bool, bool), cursor: CursorStyle, cx: &mut Context<Prmator>) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .absolute()
        .cursor(cursor)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, window, _| {
                this.start_box_drag(Some(sides), ev.position, window)
            }),
        )
}

/// The code a new comment is about.
fn snippet_box(t: &Theme, snippet: &str) -> impl IntoElement {
    div()
        .px_3()
        .py_2()
        .bg(t.code_bg)
        .font_family(t.mono.clone())
        .text_size(px(t.code_size * 0.9))
        .children(snippet.lines().take(500).map(|l| {
            let (color, bg) = match l.as_bytes().first() {
                Some(b'+') => (t.add_fg, Some(t.add_bg)),
                Some(b'-') => (t.del_fg, Some(t.del_bg)),
                Some(b'@') => (t.hunk_fg, None),
                _ => (t.text, None),
            };
            div()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_color(color)
                .when_some(bg, |d, bg| d.bg(bg))
                .child(SharedString::from(l.replace('\t', "    ")))
        }))
}

fn comment_block(t: &Theme, author: &str, when: &str, badge: Option<AnyElement>, body: &str, id: &str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .child(avatar(t, author, 20.))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(SharedString::from(author.to_string())))
                .child(div().text_color(t.dim).child(SharedString::from(when.to_string())))
                .children(badge),
        )
        .child(div().pl(px(28.)).child(markdown::render(t, &super::overview::clean_github_markdown(body), MdSize::Small, id)))
}
