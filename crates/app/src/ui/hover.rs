//! Floating UI over the diff: definition hover cards and the right-click menu.

use std::sync::Arc;

use gpui::{anchored, deferred, div, point, prelude::*, px, ClipboardItem, Context, FontWeight, SharedString};

use crate::analysis::kind_label;
use crate::app::{CodeView, DefState, Menu, Prmator, QUICK_ASKS, QUICK_ASKS_SOURCE};
use crate::github;
use crate::theme::theme;

use super::components::*;
use crate::theme::Theme;

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
                let can_open = in_diff || self.definition_in_repo(&def);
                let jump_label = if in_diff && self.browser.is_none() { "Jump to it" } else { "Open file" };
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
                        .when(can_open, |d| {
                            d.child(button(&t, "hover-jump", jump_label).on_click(
                                cx.listener(move |this, _, window, cx| this.jump_to_definition(d2.clone(), window, cx)),
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

    /// The right-click menu, in sections: where to go, review actions, questions for
    /// Claude, and copying. Sections that don't apply are left out.
    pub fn render_menu(&self, menu: &Menu, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let ctx = menu.context.clone();
        let position = menu.position;
        let in_diff = menu.view == CodeView::Diff;
        let symbol = menu.symbol.clone();

        // Navigate: the identifier under the mouse, and the file view.
        let mut navigate = vec![];
        if let Some(symbol) = symbol.clone() {
            let s = symbol.clone();
            navigate.push(
                menu_item(&t, "menu-def", format!("Show definition of `{symbol}`"), None).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.menu = None;
                        this.show_hover(s.clone(), position, cx);
                    },
                )),
            );
            let s = symbol.clone();
            navigate.push(menu_item(&t, "menu-usages", format!("Find `{symbol}` in the diff"), None).on_click(
                cx.listener(move |this, _, window, cx| {
                    this.menu = None;
                    this.open_palette(super::palette::PaletteMode::Text, window, cx);
                    if let Some(p) = &this.palette {
                        let input = p.input.clone();
                        input.update(cx, |i, cx| i.set_text(format!("/{s}"), cx));
                    }
                }),
            ));
        }
        if let Some(at) = menu.open_at.clone() {
            navigate.push(menu_item(&t, "menu-open-file", "Open in file view".into(), Some("f")).on_click(cx.listener(
                move |this, _, window, cx| this.open_browser(Some(at.clone()), window, cx),
            )));
        }

        // Review: only on diff lines.
        let review = menu.row.map(|row| {
            menu_item(&t, "menu-comment", "💬 Add review comment".into(), Some("m")).on_click(cx.listener(
                move |this, _, window, cx| {
                    this.menu = None;
                    this.open_comment_composer(row, position, window, cx);
                },
            ))
        });

        // Ask Claude: free question, the quick ones, and about the symbol.
        let quick: &[(&str, &str)] = if in_diff { QUICK_ASKS } else { QUICK_ASKS_SOURCE };
        let mut ask = vec![menu_item(&t, "menu-ask", "Ask a question…".into(), Some("a")).on_click(cx.listener(
            |this, _, window, cx| {
                if let Some(menu) = this.menu.take() {
                    this.open_chat(Some(menu.context), menu.position, None, window, cx);
                }
            },
        ))];
        ask.extend(quick.iter().enumerate().map(|(i, (label, question))| {
            menu_item(&t, ("menu-quick", i), (*label).into(), None).on_click(cx.listener(move |this, _, window, cx| {
                if let Some(menu) = this.menu.take() {
                    this.open_chat(Some(menu.context), menu.position, Some(question.to_string()), window, cx);
                }
            }))
        }));
        if let Some(symbol) = symbol.clone() {
            ask.push(menu_item(&t, "menu-ask-symbol", format!("What is `{symbol}`?"), None).on_click(cx.listener(
                move |this, _, window, cx| {
                    if let Some(menu) = this.menu.take() {
                        let q = format!(
                            "What is `{symbol}` (find its definition with `gh pr diff` or in the repo), and how is it used or changed here?"
                        );
                        this.open_chat(Some(menu.context), menu.position, Some(q), window, cx);
                    }
                },
            )));
        }

        // Copy: the selected text, and the lines (as a diff in the diff view).
        let mut copy = vec![];
        if let Some(text) = menu.selected_text.clone() {
            copy.push(menu_item(&t, "menu-copy-sel", "Copy selection".into(), Some("⌘C")).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.menu = None;
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    cx.notify();
                },
            )));
        }
        let lines_label = if in_diff { "Copy as diff" } else { "Copy lines" };
        let shortcut = menu.selected_text.is_none().then_some("⌘C");
        copy.push(menu_item(&t, "menu-copy-lines", lines_label.into(), shortcut).on_click(cx.listener(
            move |this, _, _, cx| {
                if let Some(menu) = this.menu.take() {
                    let text = if in_diff {
                        menu.context.snippet
                    } else {
                        // Drop the context marker the snippet has for Claude.
                        menu.context.snippet.lines().map(|l| l.strip_prefix(' ').unwrap_or(l)).collect::<Vec<_>>().join("\n")
                    };
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                cx.notify();
            },
        )));

        let sections: Vec<(&str, Vec<gpui::Stateful<gpui::Div>>)> = vec![
            ("Navigate", navigate),
            ("Review", review.into_iter().collect()),
            ("Ask Claude", ask),
            ("Copy", copy),
        ];
        deferred(
            anchored().position(menu.position).snap_to_window_with_margin(px(8.)).child(
                div()
                    .occlude()
                    .min_w(px(260.))
                    .max_w(px(380.))
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
                    // What the menu acts on.
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .text_xs()
                            .font_family(t.mono.clone())
                            .text_color(t.dim)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(SharedString::from(format!("{} · {}", short_path(&ctx.file), ctx.location))),
                    )
                    .children(sections.into_iter().filter(|(_, items)| !items.is_empty()).map(|(title, items)| {
                        div()
                            .flex()
                            .flex_col()
                            .mt_1()
                            .pt_1()
                            .border_t_1()
                            .border_color(t.border)
                            .child(
                                div()
                                    .px_3()
                                    .pb(px(2.))
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.muted)
                                    .child(title.to_uppercase()),
                            )
                            .children(items)
                    })),
            ),
        )
        .with_priority(1)
    }
}

/// A menu entry with an optional shortcut shown on the right.
fn menu_item(t: &Theme, id: impl Into<gpui::ElementId>, label: String, shortcut: Option<&'static str>) -> gpui::Stateful<gpui::Div> {
    let (accent, accent_fg) = (t.accent, t.accent_fg);
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .py_1()
        .rounded_md()
        .text_sm()
        .text_color(t.text)
        .hover(move |s| s.bg(accent).text_color(accent_fg).cursor_pointer())
        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(label))
        .when_some(shortcut, |d, k| d.child(kbd(t, k)))
}
