//! The floating Claude chat that opens next to the code.

use gpui::{anchored, deferred, div, point, prelude::*, px, Context, FontWeight, MouseButton, SharedString};

use crate::app::Prmator;
use crate::chat::CHAT_WIDTH;
use crate::theme::theme;
use crate::FinishChat;

use super::claude_panel::render_turn;
use super::components::*;

impl Prmator {
    pub fn render_chat(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let t = theme(cx).clone();
        let chat = self.chat.as_ref()?;
        let conv = self.conversation(chat.conversation)?;
        let id = conv.id;
        let empty = conv.turns.is_empty();
        let running = conv.is_running();

        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .bg(t.raised)
            .border_b_1()
            .border_color(t.border)
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(t.accent).child("✦ Claude"))
            .child(match &conv.context {
                Some(ctx) => context_chip(&t, ctx).into_any_element(),
                None => badge("whole PR", t.accent).into_any_element(),
            })
            .child(div().flex_1())
            .child(ghost_button(&t, "chat-discard", "Discard").on_click(cx.listener(|this, _, window, cx| this.discard_chat(window, cx))))
            .child(
                primary_button(&t, "chat-done", if empty { "Close" } else { "Done · add to log" })
                    .on_click(cx.listener(|this, _, window, cx| this.finish_chat(window, cx))),
            );

        // The code being discussed, so the chat makes sense on its own.
        let snippet = conv.context.as_ref().filter(|_| empty).map(|ctx| {
            div()
                .id("chat-snippet")
                .max_h(px(160.))
                .overflow_y_scroll()
                .px_3()
                .py_2()
                .bg(t.code_bg)
                .border_b_1()
                .border_color(t.border)
                .font_family(t.mono.clone())
                .text_xs()
                .children(ctx.snippet.lines().take(80).map(|l| {
                    let color = match l.as_bytes().first() {
                        Some(b'+') => t.add_fg,
                        Some(b'-') => t.del_fg,
                        Some(b'@') => t.hunk_fg,
                        _ => t.text,
                    };
                    div().whitespace_nowrap().text_color(color).child(SharedString::from(l.replace('\t', "    ")))
                }))
        });

        let turns = div()
            .id("chat-turns")
            .flex_1()
            .min_h(px(0.))
            .max_h(px(420.))
            .overflow_y_scroll()
            .track_scroll(&chat.scroll)
            .when(!empty, |d| d.p_3())
            .flex()
            .flex_col()
            .gap_4()
            .children(conv.turns.iter().enumerate().map(|(i, turn)| render_turn(&t, "chat", id, i, turn, cx)));

        let footer = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_t_1()
            .border_color(t.border)
            .child(div().rounded_lg().bg(t.input_bg).border_1().border_color(t.border_strong).child(self.input.clone()))
            .child(
                div()
                    .flex()
                    .px_1()
                    .text_xs()
                    .text_color(t.dim)
                    .child(if running { "Claude is answering…" } else { "↵ send · ⇧↵ new line" })
                    .child(div().flex_1())
                    .child("esc / ⌘↵ done — files it into the review log"),
            );

        Some(
            deferred(
                anchored().position(chat.position + point(px(0.), px(12.))).snap_to_window_with_margin(px(12.)).child(
                    div()
                        .id("chat")
                        .key_context("Chat")
                        .on_action(cx.listener(|this, _: &FinishChat, window, cx| this.finish_chat(window, cx)))
                        .occlude()
                        // Clicks inside the chat shouldn't reach the diff below.
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .w(px(CHAT_WIDTH))
                        .max_h(px(680.))
                        .flex()
                        .flex_col()
                        .rounded_xl()
                        .bg(t.panel)
                        .border_1()
                        .border_color(t.border_strong)
                        .shadow_lg()
                        .overflow_hidden()
                        .child(header)
                        .children(snippet)
                        .child(turns)
                        .child(footer),
                ),
            )
            .with_priority(1),
        )
    }
}
