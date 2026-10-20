//! Floating UI over the diff: definition hover cards and the right-click menu.

use std::sync::Arc;

use gpui::{anchored, deferred, div, point, prelude::*, px, ClipboardItem, Context, FontWeight, SharedString};

use crate::analysis::kind_label;
use crate::app::{DefState, Menu, Prmator, QUICK_ASKS};
use crate::github;
use crate::theme::theme;

use super::components::*;
use crate::theme::Theme;

fn symbol_item(t: &Theme, id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    let (accent, accent_fg) = (t.accent, t.accent_fg);
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_md()
        .text_sm()
        .text_color(t.text)
        .hover(move |s| s.bg(accent).text_color(accent_fg).cursor_pointer())
        .child(label)
}

impl Prmator {
    pub fn render_hover_card(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let t = theme(cx).clone();
        let (hover, state) = self.current_definition()?;
        let card = div()
            .id("hover-card")
            .occlude()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| this.on_card_hover(*hovered, cx)))
            .min_w(px(320.))
            .max_w(px(760.))
            .rounded_lg()
            .bg(t.panel)
            .border_1()
            .border_color(t.border_strong)
            .shadow_lg()
            .overflow_hidden()
            .text_xs();

        let card = match state {
            DefState::Loading => card.child(spinner_text(&t, format!("Looking up `{}`…", hover.name)).p_3()),
            DefState::NotFound => card.child(
                div()
                    .p_3()
                    .text_color(t.muted)
                    .child(SharedString::from(format!("No definition found for `{}`.", hover.name)))
                    .child(div().mt_1().text_color(t.dim).child(
                        "Searched this PR, $PRMATOR_CODE_DIR and GitHub code search (default branch).",
                    )),
            ),
            DefState::Found(def) => {
                let def: Arc<_> = def.clone();
                let in_diff = self.definition_in_diff(&def).is_some();
                let (d1, d2) = (def.clone(), def.clone());
                let url = def.url.clone();
                let lang = prmator_syntax::detect(&def.path);
                card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .bg(t.raised)
                        .border_b_1()
                        .border_color(t.border)
                        .child(badge(kind_label(def.kind), t.accent))
                        .child(
                            div()
                                .font_family(t.mono.clone())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child(SharedString::from(match &def.container {
                                    Some(c) => format!("{c}::{}", def.name),
                                    None => def.name.clone(),
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(t.dim)
                                .child(SharedString::from(format!("{}:{}", def.path, def.start_line))),
                        )
                        .child(badge(def.source.label(), if in_diff { t.add_fg } else { t.muted })),
                )
                .child(
                    div()
                        .id("hover-code")
                        .max_h(px(340.))
                        .overflow_y_scroll()
                        .p_2()
                        .bg(t.code_bg)
                        .font_family(t.mono.clone())
                        .text_size(px(t.code_size))
                        .children(def.lines.iter().enumerate().map(|(i, line)| {
                            let spans = def.highlights.get(i);
                            let (_, styled) = code_line(&t, if line.is_empty() { " " } else { line }, spans, None, t.code_bg);
                            div()
                                .flex()
                                .whitespace_nowrap()
                                .child(
                                    div()
                                        .w(px(38.))
                                        .flex_none()
                                        .pr_2()
                                        .flex()
                                        .justify_end()
                                        .text_color(t.line_no)
                                        .child(SharedString::from((def.start_line as usize + i).to_string())),
                                )
                                .child(div().text_color(t.text).child(styled))
                        }))
                        .when(def.truncated, |d| d.child(div().pl(px(38.)).text_color(t.dim).child("…"))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py(px(6.))
                        .border_t_1()
                        .border_color(t.border)
                        .when(lang.is_none(), |d| d)
                        .child(primary_button(&t, "hover-ask", "✦ Ask Claude").on_click(
                            cx.listener(move |this, _, window, cx| this.ask_about_definition(d1.clone(), window, cx)),
                        ))
                        .when(in_diff, |d| {
                            d.child(button(&t, "hover-jump", "Jump to it").on_click(
                                cx.listener(move |this, _, _, cx| this.jump_to_definition(d2.clone(), cx)),
                            ))
                        })
                        .child(button(&t, "hover-copy", "Copy").on_click({
                            let code = def.lines.join("\n");
                            move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(code.clone()))
                        }))
                        .when_some(url, |d, url| {
                            d.child(ghost_button(&t, "hover-gh", "GitHub ↗").on_click(move |_, _, _| github::open_in_browser(&url)))
                        }),
                )
            }
        };
        Some(
            deferred(
                anchored()
                    .position(hover.position + point(px(0.), px(14.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(card),
            )
            .with_priority(1),
        )
    }

    pub fn render_menu(&self, menu: &Menu, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let item = |id: &'static str, label: &'static str| {
            let accent_fg = t.accent_fg;
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_md()
                .text_sm()
                .text_color(t.text)
                .hover(move |s| s.bg(t.accent).text_color(accent_fg).cursor_pointer())
                .child(label)
        };
        let ctx = menu.context.clone();
        let accent_fg = t.accent_fg;
        deferred(
            anchored().position(menu.position).snap_to_window_with_margin(px(8.)).child(
                div()
                    .occlude()
                    .min_w(px(250.))
                    .p_1()
                    .flex()
                    .flex_col()
                    .rounded_lg()
                    .bg(t.panel)
                    .border_1()
                    .border_color(t.border_strong)
                    .shadow_lg()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .text_xs()
                            .font_family(t.mono.clone())
                            .text_color(t.dim)
                            .child(SharedString::from(format!("{} · {}", short_path(&ctx.file), ctx.location))),
                    )
                    .when_some(menu.symbol.clone(), |d, symbol| {
                        let (s1, s2, s3) = (symbol.clone(), symbol.clone(), symbol.clone());
                        let position = menu.position;
                        d.child(
                            div()
                                .px_3()
                                .pt_1()
                                .text_xs()
                                .font_family(t.mono.clone())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.accent)
                                .child(SharedString::from(symbol.clone())),
                        )
                        .child(symbol_item(&t, "sym-def", "Show definition").on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = None;
                            this.show_hover(s1.clone(), position, cx);
                        })))
                        .child(symbol_item(&t, "sym-ask", "✦ Ask Claude about it").on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(menu) = this.menu.take() {
                                let q = format!(
                                    "What is `{s2}` (find its definition with `gh pr diff` or in the repo), and how is it used or changed here?"
                                );
                                this.open_chat(Some(menu.context), menu.position, Some(q), window, cx);
                            }
                        })))
                        .child(symbol_item(&t, "sym-usages", "Find usages in this PR").on_click(cx.listener(move |this, _, window, cx| {
                            this.menu = None;
                            this.open_palette(super::palette::PaletteMode::Text, window, cx);
                            if let Some(p) = &this.palette {
                                let input = p.input.clone();
                                input.update(cx, |i, cx| i.set_text(format!("/{s3}"), cx));
                            }
                        })))
                        .child(div().my_1().h(px(1.)).bg(t.border))
                    })
                    .when_some(menu.row, |d, row| {
                        let position = menu.position;
                        d.child(item("add-comment", "💬 Add comment").on_click(cx.listener(move |this, _, window, cx| {
                            this.menu = None;
                            this.open_comment_composer(row, position, window, cx);
                        })))
                    })
                    .child(item("ask", "✦ Ask Claude…").on_click(cx.listener(|this, _, window, cx| {
                        if let Some(menu) = this.menu.take() {
                            this.open_chat(Some(menu.context), menu.position, None, window, cx);
                        }
                        cx.notify();
                    })))
                    .children(QUICK_ASKS.iter().enumerate().map(|(i, (label, question))| {
                        div()
                            .id(("quick", i))
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .text_color(t.text)
                            .hover(move |s| s.bg(t.accent).text_color(accent_fg).cursor_pointer())
                            .child(*label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(menu) = this.menu.take() {
                                    this.open_chat(Some(menu.context), menu.position, Some(question.to_string()), window, cx);
                                }
                            }))
                    }))
                    .child(div().my_1().h(px(1.)).bg(t.border))
                    .child(item("copy", "Copy diff").on_click(cx.listener(|this, _, _, cx| {
                        if let Some(menu) = this.menu.take() {
                            cx.write_to_clipboard(ClipboardItem::new_string(menu.context.snippet));
                        }
                        cx.notify();
                    }))),
            ),
        )
        .with_priority(1)
    }
}
