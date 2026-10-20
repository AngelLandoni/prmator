//! The Settings window (⌘,): theme, interface font and code font. Defaults come from Zed's
//! settings; changing anything here makes the settings PRmator's own until reset.

use gpui::{
    div, prelude::*, px, size, AppContext, Bounds, Context, Entity, FontWeight, SharedString, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use crate::app::Prmator;
use crate::settings::{zed_settings_path, Settings, MAX_FONT_SIZE, MIN_FONT_SIZE, SYSTEM_UI_FONT};
use crate::text_input::{Changed, TextInput};
use crate::theme::{theme, Theme, ThemeMode, SF_MONO};
use crate::themes::Themes;

use super::components::*;

const MAX_FONT_ROWS: usize = 150;

pub struct SettingsView {
    app: Entity<Prmator>,
    ui_query: Entity<TextInput>,
    code_query: Entity<TextInput>,
    /// Installed font families, sorted.
    fonts: Vec<String>,
    _subscriptions: Vec<Subscription>,
}

impl Prmator {
    /// Opens the settings window, or brings it to the front.
    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = &self.settings_window {
            if handle.update(cx, |_, window, _| window.activate_window()).is_ok() {
                return;
            }
        }
        let app = cx.entity();
        // The new window renders right away and reads `Prmator`, so it must be opened
        // after the current update of `Prmator` (e.g. the gear button's click) finishes.
        cx.spawn(async move |this, cx| {
            let handle = cx.update(|cx| {
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(860.), px(820.)), cx))),
                    titlebar: Some(TitlebarOptions { title: Some("PRmator Settings".into()), ..Default::default() }),
                    ..Default::default()
                };
                cx.open_window(options, |window, cx| cx.new(|cx| SettingsView::new(app, window, cx)))
            });
            if let Ok(Ok(handle)) = handle {
                let _ = this.update(cx, |this, _| this.settings_window = Some(handle));
            }
        })
        .detach();
    }
}

impl SettingsView {
    fn new(app: Entity<Prmator>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ui_query = cx.new(|cx| TextInput::new("Search fonts…", cx));
        let code_query = cx.new(|cx| TextInput::new("Search fonts…", cx));
        let subscriptions = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.subscribe(&ui_query, |_, _, _: &Changed, cx| cx.notify()),
            cx.subscribe(&code_query, |_, _, _: &Changed, cx| cx.notify()),
        ];
        let mut fonts: Vec<String> =
            cx.text_system().all_font_names().into_iter().filter(|f| !f.starts_with('.')).collect();
        fonts.sort_by_key(|f| f.to_lowercase());
        fonts.dedup();
        SettingsView { app, ui_query, code_query, fonts, _subscriptions: subscriptions }
    }

    fn update(&self, cx: &mut Context<Self>, change: impl FnOnce(&mut Settings)) {
        self.app.update(cx, |app, cx| app.update_settings(change, cx));
    }

    fn section(t: &Theme, title: &'static str, hint: &'static str) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_xl()
            .bg(t.panel)
            .border_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(title))
                    .when(!hint.is_empty(), |d| d.child(div().text_xs().text_color(t.muted).child(hint))),
            )
    }

    fn stepper(
        &self,
        t: &Theme,
        id: &'static str,
        label: &'static str,
        value: String,
        down: impl Fn(&mut Settings) + 'static,
        up: impl Fn(&mut Settings) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().w(px(110.)).text_sm().text_color(t.muted).child(label))
            .child(
                button(t, SharedString::from(format!("{id}-down")), "−")
                    .on_click(cx.listener(move |this, _, _, cx| this.update(cx, |s| down(s)))),
            )
            .child(
                div()
                    .min_w(px(56.))
                    .flex()
                    .justify_center()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child(SharedString::from(value)),
            )
            .child(
                button(t, SharedString::from(format!("{id}-up")), "+")
                    .on_click(cx.listener(move |this, _, _, cx| this.update(cx, |s| up(s)))),
            )
    }

    /// A searchable list of font families, each shown in its own face.
    fn font_picker(
        &self,
        t: &Theme,
        id: &'static str,
        query: &Entity<TextInput>,
        current: &str,
        extra: &[(&'static str, &'static str)],
        pick: fn(&mut Settings, String),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let q = query.read(cx).text().to_lowercase();
        let mut options: Vec<(String, String)> =
            extra.iter().map(|(family, label)| (family.to_string(), label.to_string())).collect();
        options.extend(self.fonts.iter().map(|f| (f.clone(), f.clone())));
        let matches: Vec<(String, String)> = options
            .into_iter()
            .filter(|(_, label)| q.is_empty() || label.to_lowercase().contains(&q))
            .take(MAX_FONT_ROWS)
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(110.)).text_sm().text_color(t.muted).child("Family"))
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).font_family(SharedString::from(current.to_string())).child(SharedString::from(current.to_string()))),
            )
            .child(div().rounded_md().bg(t.input_bg).border_1().border_color(t.border_strong).child(query.clone()))
            .child(
                div()
                    .id(SharedString::from(format!("{id}-list")))
                    .h(px(180.))
                    .overflow_y_scroll()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border)
                    .bg(t.bg)
                    .py_1()
                    .children(matches.into_iter().enumerate().map(|(i, (family, label))| {
                        let selected = family == current;
                        let value = family.clone();
                        div()
                            .id(SharedString::from(format!("{id}-{i}")))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py(px(3.))
                            .when(selected, |d| d.bg(t.accent_soft))
                            .hover(|s| s.bg(t.hover).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let value = value.clone();
                                this.update(cx, move |s| pick(s, value));
                            }))
                            .child(div().w(px(12.)).text_color(t.accent).child(if selected { "✓" } else { "" }))
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_sm()
                                    .font_family(SharedString::from(family))
                                    .child(SharedString::from(label)),
                            )
                    })),
            )
    }

    fn theme_list(&self, t: &Theme, dark: bool, current: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let entries: Vec<(String, String, [gpui::Hsla; 4])> = cx
            .global::<Themes>()
            .0
            .iter()
            .filter(|e| e.dark == dark)
            .map(|e| (e.name.clone(), e.source.clone(), [e.theme.bg, e.theme.accent, e.theme.add_fg, e.theme.syn_keyword]))
            .collect();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_sm().text_color(t.muted).child(if dark { "Dark theme" } else { "Light theme" }))
            .child(
                div()
                    .id(if dark { "dark-themes" } else { "light-themes" })
                    .h(px(220.))
                    .overflow_y_scroll()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border)
                    .bg(t.bg)
                    .py_1()
                    .children(entries.into_iter().enumerate().map(|(i, (name, source, colors))| {
                        let selected = name == current;
                        let value = name.clone();
                        div()
                            .id(SharedString::from(format!("theme-{dark}-{i}")))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py(px(3.))
                            .when(selected, |d| d.bg(t.accent_soft))
                            .hover(|s| s.bg(t.hover).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let value = value.clone();
                                this.update(cx, move |s| {
                                    if dark {
                                        s.dark_theme = value;
                                    } else {
                                        s.light_theme = value;
                                    }
                                });
                            }))
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .rounded(px(3.))
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(t.border)
                                    .children(colors.into_iter().map(|c| div().w(px(8.)).h(px(12.)).bg(c))),
                            )
                            .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_sm().child(SharedString::from(name)))
                            .child(div().text_xs().text_color(t.dim).child(SharedString::from(source)))
                    })),
            )
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        window.set_rem_size(px(t.ui_size));
        let (settings, follows_zed) = {
            let app = self.app.read(cx);
            (app.settings.clone(), app.store.settings.is_none())
        };
        let zed_path = zed_settings_path().map(|p| p.display().to_string()).unwrap_or_default();
        let mode = settings.theme_mode;

        let source = div()
            .flex()
            .items_center()
            .gap_3()
            .p_3()
            .rounded_xl()
            .bg(if follows_zed { t.accent_soft } else { t.raised })
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(if follows_zed {
                        "Following your Zed settings"
                    } else {
                        "Customized in PRmator"
                    }))
                    .child(div().text_xs().text_color(t.muted).child(SharedString::from(if follows_zed {
                        format!("Fonts and theme come from {zed_path}. Changing anything here makes them PRmator's own.")
                    } else {
                        format!("Your Zed settings ({zed_path}) are no longer applied.")
                    }))),
            )
            .when(!follows_zed, |d| {
                d.child(primary_button(&t, "reset-zed", "Reset to Zed settings").on_click(cx.listener(|this, _, _, cx| {
                    this.app.update(cx, |app, cx| app.reset_settings_to_zed(cx));
                })))
            });

        let appearance = Self::section(&t, "Theme", "Pick the theme for light and dark mode; the mode decides which one shows.")
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(110.)).text_sm().text_color(t.muted).child("Mode"))
                    .child(
                        segmented(&t)
                            .child(segment(&t, "mode-system", "Follow system", mode == ThemeMode::System).on_click(
                                cx.listener(|this, _, _, cx| this.update(cx, |s| s.theme_mode = ThemeMode::System)),
                            ))
                            .child(segment(&t, "mode-light", "Light", mode == ThemeMode::Light).on_click(
                                cx.listener(|this, _, _, cx| this.update(cx, |s| s.theme_mode = ThemeMode::Light)),
                            ))
                            .child(segment(&t, "mode-dark", "Dark", mode == ThemeMode::Dark).on_click(
                                cx.listener(|this, _, _, cx| this.update(cx, |s| s.theme_mode = ThemeMode::Dark)),
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(self.theme_list(&t, false, &settings.light_theme, cx))
                    .child(self.theme_list(&t, true, &settings.dark_theme, cx)),
            );

        let ui_font = Self::section(&t, "Interface font", "Used for the whole interface; the size scales everything, like Zed's ui_font_size.")
            .child(self.font_picker(
                &t,
                "ui-font",
                &self.ui_query.clone(),
                &settings.ui_font_family,
                &[(SYSTEM_UI_FONT, "System UI (San Francisco)")],
                |s, f| s.ui_font_family = f,
                cx,
            ))
            .child(self.stepper(
                &t,
                "ui-size",
                "Size",
                format!("{}", settings.ui_font_size),
                |s| s.ui_font_size = (s.ui_font_size - 1.).max(MIN_FONT_SIZE),
                |s| s.ui_font_size = (s.ui_font_size + 1.).min(MAX_FONT_SIZE),
                cx,
            ));

        let code_font = Self::section(&t, "Code font", "Used for diffs, code in answers, and hover cards.")
            .child(self.font_picker(
                &t,
                "code-font",
                &self.code_query.clone(),
                &settings.code_font_family,
                &[(SF_MONO, "SF Mono (GitHub's code font)")],
                |s, f| s.code_font_family = f,
                cx,
            ))
            .child(self.stepper(
                &t,
                "code-size",
                "Size",
                format!("{}", settings.code_font_size),
                |s| s.code_font_size = (s.code_font_size - 1.).max(MIN_FONT_SIZE),
                |s| s.code_font_size = (s.code_font_size + 1.).min(MAX_FONT_SIZE),
                cx,
            ))
            .child(self.stepper(
                &t,
                "line-height",
                "Line height",
                format!("{:.2}", settings.code_line_height),
                |s| s.code_line_height = ((s.code_line_height - 0.05) * 100.).round() / 100.,
                |s| s.code_line_height = ((s.code_line_height + 0.05) * 100.).round() / 100.,
                cx,
            ))
            .child(code_preview(&t));

        div()
            .id("settings")
            .size_full()
            .overflow_y_scroll()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(t.ui_font.clone())
            .child(
                div()
                    .max_w(px(820.))
                    .mx_auto()
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child("Settings"))
                    .child(source)
                    .child(appearance)
                    .child(ui_font)
                    .child(code_font),
            )
    }
}

/// A few diff lines in the current code font, size and colors.
fn code_preview(t: &Theme) -> impl IntoElement {
    let lines: [(&str, Option<(gpui::Hsla, gpui::Hsla)>); 4] = [
        ("  fn apply(&mut self, op: BatchOperation) {", None),
        ("-     self.items.push(op.item);", Some((t.del_bg, t.del_fg))),
        ("+     self.items.insert(op.id, op.item);", Some((t.add_bg, t.add_fg))),
        ("  }", None),
    ];
    div()
        .flex()
        .flex_col()
        .rounded_md()
        .overflow_hidden()
        .border_1()
        .border_color(t.border)
        .bg(t.bg)
        .font_family(t.mono.clone())
        .text_size(px(t.code_size))
        .children(lines.into_iter().map(|(text, colors)| {
            div()
                .h(px(t.row_h))
                .flex()
                .items_center()
                .px_3()
                .whitespace_nowrap()
                .when_some(colors, |d, (bg, fg)| d.bg(bg).text_color(fg))
                .child(text)
        }))
}
