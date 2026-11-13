//! Right column: the review log. Every finished Claude conversation is filed here as a
//! collapsible entry that can be expanded, continued, jumped to, copied or deleted.

use gpui::{
    div, prelude::*, px, AnyElement, ClipboardItem, Context, Div, FontWeight, MouseButton, SharedString, StyledText,
};

use crate::app::Prmator;
use crate::chat::{ago, Conversation, Turn, TurnStatus};
use crate::theme::{theme, Theme};

use super::components::*;
use super::markdown::{render_in, MdSize};
use super::select::selectable;

impl Prmator {
    pub fn render_claude(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let entries: Vec<&Conversation> = self
            .pr
            .as_ref()
            .and_then(|pv| self.conversations.get(&pv.key))
            .into_iter()
            .flatten()
            .filter(|c| c.in_log)
            .rev() // newest first
            .collect();
        let (dot, status) = match (self.connected, &self.claude_version) {
            (true, Some(v)) => (t.add_fg, v.clone()),
            (true, None) => (t.add_fg, "connected".into()),
            (false, _) => (t.del_fg, "starting bridge…".into()),
        };
        let count = entries.len();
        let any_expanded = entries.iter().any(|c| c.expanded);

        let body: AnyElement = if self.pr.is_none() {
            div().p_4().text_sm().text_color(t.muted).child("Open a pull request to start a review log.").into_any_element()
        } else if entries.is_empty() {
            div()
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .text_sm()
                .text_color(t.muted)
                .child(div().text_color(t.text).font_weight(FontWeight::MEDIUM).child("Your review log is empty"))
                .child(tip(&t, "Right-click", "code → Ask Claude opens a chat right there."))
                .child(tip(&t, "Select + a", "to ask about several lines."))
                .child(tip(&t, "Done / esc", "files the conversation here to review later."))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .p_2()
                .gap_2()
                .children(entries.into_iter().map(|c| self.render_log_entry(&t, c, cx)))
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .h(px(41.))
                    .border_b_1()
                    .border_color(t.border)
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Review log"))
                    .when(count > 0, |d| d.child(badge(count.to_string(), t.muted)))
                    .child(div().flex_1())
                    .when(count > 0, |d| {
                        d.child(
                            ghost_button(&t, "log-fold", if any_expanded { "Collapse all" } else { "Expand all" })
                                .on_click(cx.listener(move |this, _, _, cx| this.set_log_expanded(!any_expanded, cx))),
                        )
                    })
                    .when(self.pr.is_some(), |d| {
                        d.child(primary_button(&t, "ask-pr", "✦ Ask").on_click(cx.listener(|this, _, window, cx| {
                            this.run_command(crate::app::Command::AskAboutPr, window, cx)
                        })))
                    }),
            )
            .when_some(self.bridge_error.clone(), |d, e| d.child(error_text(&t, &e)))
            .child(div().id("review-log").flex_1().overflow_y_scroll().track_scroll(&self.log_scroll).child(body))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(t.border)
                    .text_xs()
                    .text_color(t.dim)
                    .child(div().size(px(7.)).rounded_full().bg(dot))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(SharedString::from(status))),
            )
    }

    fn render_log_entry(&self, t: &Theme, c: &Conversation, cx: &mut Context<Self>) -> impl IntoElement {
        let id = c.id;
        let open_in_chat = self.chat.as_ref().is_some_and(|chat| chat.conversation == id);
        let running = c.is_running();
        let failed = c.turns.iter().any(|t| matches!(t.status, TurnStatus::Failed(_)));
        let turns = c.turns.len();
        let has_anchor = c.context.is_some();
        let transcript = c.transcript();

        let header = div()
            .id(("log-head", id))
            .flex()
            .items_start()
            .gap_2()
            .px_2()
            .py_2()
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_log_entry(id, cx)))
            .child(div().w(px(10.)).flex_none().pt(px(1.)).text_xs().text_color(t.dim).child(if c.expanded { "▾" } else { "▸" }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(match &c.context {
                                Some(ctx) => context_chip(t, ctx).into_any_element(),
                                None => badge("whole PR", t.accent).into_any_element(),
                            })
                            .child(div().flex_1())
                            .when(running, |d| d.child(div().size(px(6.)).rounded_full().bg(t.accent)))
                            .when(failed, |d| d.child(badge("error", t.del_fg)))
                            .when(open_in_chat, |d| d.child(badge("open", t.accent)))
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(t.dim)
                                    .child(SharedString::from(format!("{} · {}", plural(turns, "turn"), ago(c.created)))),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text)
                            .when(!c.expanded, |d| d.overflow_hidden().whitespace_nowrap())
                            .child(SharedString::from(c.title().to_string())),
                    ),
            );

        let body = c.expanded.then(|| {
            div()
                .flex()
                .flex_col()
                .gap_3()
                .px_3()
                .pb_2()
                .children(c.turns.iter().enumerate().map(|(i, turn)| render_turn(t, "log", id, i, turn, cx)))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .pt_1()
                        .border_t_1()
                        .border_color(t.border)
                        .child(
                            button(t, ("log-continue", id), "↩ Continue").on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                                let at = match ev {
                                    gpui::ClickEvent::Mouse(m) => m.down.position,
                                    _ => this.default_chat_position(window),
                                };
                                // Open to the left of the log, where the code is.
                                let at = gpui::point(at.x - px(crate::chat::CHAT_WIDTH + 24.), at.y);
                                this.continue_conversation(id, at, window, cx)
                            })),
                        )
                        .when(has_anchor, |d| {
                            d.child(button(t, ("log-jump", id), "Jump to code").on_click(
                                cx.listener(move |this, _, _, cx| this.jump_to_conversation(id, cx)),
                            ))
                        })
                        .child(ghost_button(t, ("log-copy", id), "Copy").on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(transcript.clone()))
                        }))
                        .child(div().flex_1())
                        .child(ghost_button(t, ("log-delete", id), "Delete").on_click(
                            cx.listener(move |this, _, _, cx| this.delete_conversation(id, cx)),
                        )),
                )
        });

        div()
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(if open_in_chat { t.accent } else { t.border })
            .bg(t.panel)
            .overflow_hidden()
            .child(header)
            .children(body)
    }

    pub fn set_log_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        if let Some(key) = self.pr.as_ref().map(|pv| pv.key.clone()) {
            for c in self.conversations.entry(key).or_default() {
                c.expanded = expanded;
            }
        }
        cx.notify();
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

/// One question and its answer (used in the floating chat and the review log).
/// `place` tells the chat and the log apart, so a mouse selection stays in one of them.
pub fn render_turn(t: &Theme, place: &str, conversation: u64, index: usize, turn: &Turn, cx: &mut Context<Prmator>) -> impl IntoElement {
    let scope = format!("{place}-{conversation}");
    let sel = t.accent.opacity(0.3);
    let request = turn.request;
    let follow_up = index > 0;
    let key = |what: &str| SharedString::from(format!("{what}-{conversation}-{index}"));
    let answer = turn.answer.trim().to_string();
    let copy_text = answer.clone();
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .gap_2()
                .child(div().flex_none().text_xs().pt(px(2.)).text_color(t.accent).child(if follow_up { "↳" } else { "Q" }))
                .child(
                    div()
                        .flex_1()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(t.accent_soft)
                        .text_sm()
                        .text_color(t.text)
                        .child(selectable(scope.clone(), StyledText::new(SharedString::from(turn.question.clone())), sel)),
                ),
        )
        .children(turn.activity.iter().map(|a| {
            div().text_xs().font_family(t.mono.clone()).text_color(t.tool).child(SharedString::from(format!("▸ {a}")))
        }))
        .when(!answer.is_empty(), |d| {
            d.child(render_in(t, &answer, MdSize::Small, &format!("{scope}-{index}"), &scope))
        })
        .child(
            div().flex().items_center().gap_2().text_xs().text_color(t.dim).map(|d| match &turn.status {
                TurnStatus::Running => d.child(spinner_text(t, "thinking…")).child(
                    ghost_button(t, key("stop"), "Stop")
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| this.stop_turn(request, cx))),
                ),
                TurnStatus::Done => d
                    .when(!copy_text.is_empty(), |d| {
                        d.child(ghost_button(t, key("copy"), "Copy answer").on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()))
                        }))
                    }),
                TurnStatus::Failed(e) => d.child(error_text(t, e)),
            }),
        )
}

fn tip(t: &Theme, key: &'static str, text: &'static str) -> Div {
    div()
        .flex()
        .gap_2()
        .child(div().flex_none().text_color(t.accent).font_weight(FontWeight::MEDIUM).child(key))
        .child(div().text_color(t.muted).child(text))
}
