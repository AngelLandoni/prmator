//! Rendering of the root view: toolbar on top, then [sidebar | diff | Claude].

pub mod browser;
pub mod chat;
pub mod claude_panel;
pub mod comments;
pub mod components;
pub mod diff_view;
pub mod hover;
pub mod markdown;
pub mod overview;
pub mod palette;
pub mod select;
pub mod settings_window;
pub mod sidebar;
pub mod story;

use gpui::{div, prelude::*, px, Context, Div, FontWeight, MouseButton, MouseDownEvent, SharedString, Stateful, Window};

use crate::app::{Command, Prmator};
use crate::store::DiffMode;
use crate::theme::theme;
use crate::*;
use components::*;
use palette::PaletteMode;

const TOOLBAR_H: f32 = 44.;

impl Prmator {
    /// The toolbar is also the window's titlebar: its empty areas drag the window and
    /// double-clicking them zooms it, like a native titlebar. Traffic lights sit on its left.
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let mode = self.store.diff_mode;
        let theme_name = self.active_theme_name().to_string();
        let crumb = self.pr.as_ref().map(|pv| {
            format!("{} #{}", pv.key.repo.rsplit('/').next().unwrap_or_default(), pv.key.number)
        });
        let has_pr = self.pr.is_some();
        let plan_label = match self.pr.as_ref().and_then(|pv| pv.plan.as_ref()) {
            Some(plan) if plan.next_step().is_some() => "▶ Continue review",
            Some(_) => "✓ Review complete",
            None => "✦ Plan review",
        };
        let planning = self.pr.as_ref().is_some_and(|pv| pv.plan_run.is_some());

        div()
            .flex_none()
            .h(px(TOOLBAR_H))
            .flex()
            .items_center()
            .pr_3()
            .bg(t.title_bar)
            .border_b_1()
            .border_color(t.border)
            // Room for the traffic lights.
            .child(self.drag_region("drag-lights", cx).w(px(78.)))
            .child(
                self.drag_region("drag-brand", cx)
                    .flex()
                    .items_center()
                    .gap_2()
                    .pr_3()
                    .child(
                        div()
                            .size(px(20.))
                            .rounded(px(5.))
                            .bg(t.accent)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.accent_fg)
                            .child("P"),
                    )
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("PRmator"))
                    .when_some(crumb, |d, crumb| {
                        d.child(div().text_color(t.dim).child("/")).child(
                            div().text_sm().text_color(t.muted).font_family(t.mono.clone()).child(SharedString::from(crumb)),
                        )
                    }),
            )
            .child(self.drag_region("drag-left", cx).flex_1())
            .child(
                div()
                    .id("search")
                    .w(px(320.))
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .bg(t.input_bg)
                    .border_1()
                    .border_color(t.border)
                    .text_xs()
                    .text_color(t.dim)
                    .hover(|s| s.border_color(t.border_strong).cursor_pointer())
                    .on_click(cx.listener(|this, _, window, cx| this.open_palette(PaletteMode::Files, window, cx)))
                    .child("⌕")
                    .child(div().flex_1().child(if has_pr { "Search files, symbols, code…" } else { "Search pull requests…" }))
                    .child(kbd(&t, "⇧⌘O")),
            )
            .child(self.drag_region("drag-right", cx).flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .when(has_pr, |d| {
                        d.child(
                            primary_button(&t, "toolbar-plan", if planning { "◌ Planning…" } else { plan_label }).on_click(
                                cx.listener(|this, _, window, cx| this.run_command(Command::ContinueReview, window, cx)),
                            ),
                        )
                    })
                    .child(
                        segmented(&t)
                            .child(segment(&t, "unified", "Unified", mode == DiffMode::Unified).on_click(
                                cx.listener(|this, _, _, cx| this.set_diff_mode(DiffMode::Unified, cx)),
                            ))
                            .child(segment(&t, "split", "Split", mode == DiffMode::Split).on_click(
                                cx.listener(|this, _, _, cx| this.set_diff_mode(DiffMode::Split, cx)),
                            )),
                    )
                    .child(
                        ghost_button(&t, "toolbar-settings", "⚙")
                            .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
                    )
                    .child(
                        button(&t, "theme", format!("{} {}", self.settings.theme_mode.icon(), theme_name))
                            .on_click(cx.listener(|this, _, window, cx| this.open_palette(PaletteMode::Themes, window, cx))),
                    ),
            )
    }

    /// An empty toolbar area that moves the window when dragged.
    fn drag_region(&self, id: &'static str, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id(id)
            .h_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, _| {
                    if ev.click_count == 2 {
                        this.titlebar_drag = false;
                        window.titlebar_double_click();
                    } else {
                        this.titlebar_drag = true;
                    }
                }),
            )
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.titlebar_drag = false))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if std::mem::take(&mut this.titlebar_drag) {
                    crate::mac::start_window_drag(window);
                }
            }))
    }
}

impl Render for Prmator {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        // Like Zed's ui_font_size: rem-based sizes across the UI scale with it.
        window.set_rem_size(px(t.ui_size));
        self.refresh_find();
        let left = match (&self.pr, &self.browser) {
            (Some(_), Some(_)) => self.render_browser_tree(cx).into_any_element(),
            (Some(_), None) => self.render_pr_sidebar(cx).into_any_element(),
            (None, _) => self.render_pr_list(cx).into_any_element(),
        };
        let center = match (&self.pr, &self.browser) {
            (Some(_), Some(_)) => self.render_browser_source(cx).into_any_element(),
            _ => self.render_center(cx),
        };
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(t.ui_font.clone())
            .track_focus(&self.root_focus)
            .key_context("Prmator")
            .on_action(cx.listener(Self::refresh))
            .on_action(cx.listener(Self::back_to_list))
            .on_action(cx.listener(Self::toggle_diff_mode_action))
            .on_action(cx.listener(Self::cycle_theme))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.open_settings(cx)))
            .on_action(cx.listener(Self::toggle_review_mode))
            .on_action(cx.listener(|this, _: &OpenFiles, window, cx| this.open_palette(PaletteMode::Files, window, cx)))
            .on_action(cx.listener(|this, _: &OpenSymbols, window, cx| this.open_palette(PaletteMode::Symbols, window, cx)))
            .on_action(cx.listener(|this, _: &OpenCommands, window, cx| this.open_palette(PaletteMode::Commands, window, cx)))
            .on_action(cx.listener(|this, _: &SearchDiff, window, cx| this.open_find(window, cx)))
            .on_action(cx.listener(|this, _: &FindNext, _, cx| this.find_step(1, cx)))
            .on_action(cx.listener(|this, _: &FindPrev, _, cx| this.find_step(-1, cx)))
            .on_action(cx.listener(|this, _: &PlanReview, _, cx| this.plan_review(cx)))
            // ⌘C with nothing more specific focused: copy selected text.
            .on_action(cx.listener(|_, _: &CopySelection, _, cx| {
                select::copy(cx);
            }))
            // Must paint first: it resets the selectable-text registry each frame.
            .child(select::root())
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(320.))
                            .flex_none()
                            .h_full()
                            .bg(t.sidebar)
                            .border_r_1()
                            .border_color(t.border)
                            .child(left),
                    )
                    .child(div().flex_1().min_w_0().h_full().child(center))
                    .child(
                        div()
                            .w(px(420.))
                            .flex_none()
                            .h_full()
                            .bg(t.panel)
                            .border_l_1()
                            .border_color(t.border)
                            .child(self.render_claude(cx)),
                    ),
            )
            .when_some(self.menu.as_ref(), |d, menu| d.child(self.render_menu(menu, cx)))
            .when(self.hover.is_some(), |d| d.children(self.render_hover_card(cx)))
            .when(self.chat.is_some(), |d| d.children(self.render_chat(cx)))
            .when(self.comment_popover.is_some(), |d| d.children(self.render_comment_popover(window, cx)))
            .when(self.review_panel.is_some(), |d| {
                // Under the tab bar, right-aligned to the diff.
                let at = gpui::point(window.viewport_size().width - px(420. + 540.), px(88.));
                d.children(self.render_review_panel(at, cx))
            })
            .when(self.find.is_some(), |d| d.children(self.render_find_bar(cx)))
            .when(self.palette.is_some(), |d| d.child(self.render_palette(cx)))
    }
}

impl Prmator {
    /// `s` in the diff toggles; also bound globally from the toolbar.
    fn toggle_diff_mode_action(&mut self, a: &ToggleDiffMode, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_diff_mode(a, window, cx);
    }
}
