//! Command palette: files (⌘P), symbols (⇧⌘O, `@`), commands (⇧⌘P, `>`) and diff text
//! search (⌘F, `/`). The mode follows the query's first character, like VS Code.

use std::ops::Range;

use gpui::{
    deferred, div, prelude::*, px, App, Context, Entity, Focusable, FontWeight, HighlightStyle, Hsla, ScrollHandle, SharedString,
    StyledText, Subscription, Window,
};

use crate::analysis::{kind_icon, kind_label, SymbolChange};
use crate::app::{Command, Prmator};
use crate::github::PrSummary;
use crate::model::Load;
use crate::text_input::{Changed, Submitted, TextInput};
use crate::theme::{theme, Theme};
use crate::themes::Themes;
use crate::*;

use super::components::*;

const MAX_ITEMS: usize = 80;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    Files,
    Symbols,
    Commands,
    Text,
    Themes,
}

impl PaletteMode {
    fn prefix(self) -> &'static str {
        match self {
            PaletteMode::Files => "",
            PaletteMode::Symbols => "@",
            PaletteMode::Commands => ">",
            PaletteMode::Text => "/",
            PaletteMode::Themes => "~",
        }
    }

    fn of(query: &str) -> (PaletteMode, &str) {
        match query.chars().next() {
            Some('@') => (PaletteMode::Symbols, &query[1..]),
            Some('>') => (PaletteMode::Commands, &query[1..]),
            Some('/') => (PaletteMode::Text, &query[1..]),
            Some('~') => (PaletteMode::Themes, &query[1..]),
            _ => (PaletteMode::Files, query),
        }
    }
}

#[derive(Clone)]
pub enum PaletteAction {
    OpenPr(PrSummary),
    File(usize),
    Lines { fi: usize, new_side: bool, lines: Range<u32> },
    Command(Command),
    Theme(String),
}

pub struct PaletteItem {
    icon: SharedString,
    icon_color: Option<Hsla>,
    label: String,
    matched: Vec<usize>,
    detail: String,
    trailing: Option<(String, Hsla)>,
    action: PaletteAction,
    score: i32,
    /// Color preview (themes).
    swatch: Option<[Hsla; 5]>,
}

pub struct Palette {
    pub input: Entity<TextInput>,
    pub items: Vec<PaletteItem>,
    pub selected: usize,
    pub scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl Prmator {
    pub fn open_palette(&mut self, mode: PaletteMode, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.hover = None;
        if let Some(p) = &self.palette {
            // Already open: just switch the prefix.
            let input = p.input.clone();
            input.update(cx, |i, cx| i.set_text(mode.prefix(), cx));
            window.focus(&input.focus_handle(cx));
            return;
        }
        let placeholder = if self.pr.is_some() {
            "Go to file · @ symbol · > command · / search diff"
        } else {
            "Go to pull request · > command"
        };
        let input = cx.new(|cx| TextInput::new(placeholder, cx).submit_empty());
        let subscriptions = vec![
            cx.subscribe(&input, |this, _, Changed(text), cx| this.update_palette(text.clone(), cx)),
            cx.subscribe_in(&input, window, |this, _, _: &Submitted, window, cx| this.confirm_palette(window, cx)),
        ];
        window.focus(&input.focus_handle(cx));
        self.palette = Some(Palette { input: input.clone(), items: vec![], selected: 0, scroll: ScrollHandle::new(), _subscriptions: subscriptions });
        input.update(cx, |i, cx| i.set_text(mode.prefix(), cx));
        self.update_palette(mode.prefix().to_string(), cx);
    }

    pub fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.apply_theme(cx); // undo any theme preview
        if self.pr.is_some() {
            window.focus(&self.diff_focus);
        } else {
            window.focus(&self.root_focus);
        }
        cx.notify();
    }

    fn update_palette(&mut self, query: String, cx: &mut Context<Self>) {
        let items = self.palette_items(&query, cx);
        // Theme list: start on the theme in use.
        let active = self.active_theme_name().to_string();
        let active = &active;
        let selected = match PaletteMode::of(&query) {
            (PaletteMode::Themes, q) if q.trim().is_empty() => items.iter().position(|i| &i.label == active).unwrap_or(0),
            _ => 0,
        };
        if let Some(p) = &mut self.palette {
            p.items = items;
            p.selected = selected;
            p.scroll.scroll_to_item(selected);
        }
        self.preview_selected_theme(cx);
        cx.notify();
    }

    /// Live preview while browsing themes; closing without picking restores the saved one.
    fn preview_selected_theme(&mut self, cx: &mut Context<Self>) {
        let name = self.palette.as_ref().and_then(|p| match &p.items.get(p.selected)?.action {
            PaletteAction::Theme(name) => Some(name.clone()),
            _ => None,
        });
        if let Some(name) = name {
            self.preview_theme(&name, cx);
        }
    }

    pub fn palette_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.palette {
            if p.items.is_empty() {
                return;
            }
            let n = p.items.len() as isize;
            p.selected = ((p.selected as isize + delta).rem_euclid(n)) as usize;
            p.scroll.scroll_to_item(p.selected);
            self.preview_selected_theme(cx);
            cx.notify();
        }
    }

    fn confirm_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.palette else { return };
        let Some(action) = p.items.get(p.selected).map(|i| i.action.clone()) else { return };
        self.close_palette(window, cx);
        self.run_palette_action(action, window, cx);
    }

    fn run_palette_action(&mut self, action: PaletteAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            PaletteAction::OpenPr(pr) => self.open_pr(pr, window, cx),
            PaletteAction::File(fi) => {
                if let Some(pv) = &mut self.pr {
                    if pv.collapsed.remove(&fi) {
                        pv.rebuild_rows();
                    }
                    pv.jump_to_file(fi);
                }
            }
            PaletteAction::Lines { fi, new_side, lines } => {
                if let Some(pv) = &mut self.pr {
                    pv.jump_to_lines(fi, new_side, lines);
                }
            }
            PaletteAction::Command(c) => self.run_command(c, window, cx),
            PaletteAction::Theme(name) => self.select_theme(&name, cx),
        }
        cx.notify();
    }

    fn palette_items(&self, query: &str, cx: &App) -> Vec<PaletteItem> {
        let (mode, q) = PaletteMode::of(query);
        let q = q.trim();
        let mut items = match mode {
            PaletteMode::Files => match &self.pr {
                Some(pv) => pv
                    .files()
                    .iter()
                    .enumerate()
                    .filter_map(|(fi, f)| {
                        let (score, matched) = fuzzy(q, &f.path)?;
                        Some(PaletteItem {
                            icon: "▤".into(),
                            icon_color: None,
                            label: f.path.clone(),
                            matched,
                            detail: String::new(),
                            trailing: Some((format!("+{} −{}", f.additions, f.deletions), gpui::transparent_black())),
                            action: PaletteAction::File(fi),
                            score: score + if pv.viewed.contains(&fi) { 0 } else { 1 },
                            swatch: None,
                        })
                    })
                    .collect(),
                None => match self.prs.get(&self.filter) {
                    Some(Load::Ready(prs)) => prs
                        .iter()
                        .filter_map(|pr| {
                            let (score, matched) = fuzzy(q, &pr.title)?;
                            Some(PaletteItem {
                                icon: "⇄".into(),
                                icon_color: None,
                                label: pr.title.clone(),
                                matched,
                                detail: format!("{} #{} · {}", pr.repo(), pr.number, pr.author.login),
                                trailing: None,
                                action: PaletteAction::OpenPr(pr.clone()),
                                score,
                                swatch: None,
                            })
                        })
                        .collect(),
                    _ => vec![],
                },
            },
            PaletteMode::Symbols => self.symbol_items(q),
            PaletteMode::Commands => Command::ALL
                .iter()
                .filter(|c| self.pr.is_some() || !c.needs_pr())
                .filter_map(|&c| {
                    let (score, matched) = fuzzy(q, c.label())?;
                    Some(PaletteItem {
                        icon: "›".into(),
                        icon_color: None,
                        label: c.label().into(),
                        matched,
                        detail: String::new(),
                        trailing: (!c.shortcut().is_empty()).then(|| (c.shortcut().to_string(), gpui::transparent_black())),
                        action: PaletteAction::Command(c),
                        score,
                        swatch: None,
                    })
                })
                .collect(),
            PaletteMode::Text => self.text_items(q),
            PaletteMode::Themes => self.theme_items(q, cx),
        };
        if !q.is_empty() && mode != PaletteMode::Text {
            items.sort_by(|a, b| b.score.cmp(&a.score));
        }
        items.truncate(MAX_ITEMS);
        items
    }

    fn theme_items(&self, q: &str, cx: &App) -> Vec<PaletteItem> {
        let active = [self.settings.light_theme.as_str(), self.settings.dark_theme.as_str()];
        cx.global::<Themes>()
            .0
            .iter()
            .filter_map(|e| {
                let (score, matched) = fuzzy(q, &e.name)?;
                let t = &e.theme;
                Some(PaletteItem {
                    icon: if e.dark { "☾".into() } else { "☀".into() },
                    icon_color: None,
                    label: e.name.clone(),
                    matched,
                    detail: e.source.clone(),
                    trailing: active.contains(&e.name.as_str()).then(|| ("✓".to_string(), gpui::transparent_black())),
                    action: PaletteAction::Theme(e.name.clone()),
                    score,
                    swatch: Some([t.bg, t.accent, t.add_fg, t.del_fg, t.syn_keyword]),
                })
            })
            .collect()
    }

    fn symbol_items(&self, q: &str) -> Vec<PaletteItem> {
        let Some(pv) = &self.pr else { return vec![] };
        let mut items = vec![];
        let mut seen = std::collections::HashSet::new();
        for (&fi, a) in &pv.analysis {
            let path = &pv.files()[fi].path;
            let changed = a.changed.iter().map(|c| (c.symbol.clone(), c.new_side, Some(c.change)));
            let all = a.new.symbols.iter().map(|s| (s.clone(), true, None));
            for (sym, new_side, change) in changed.chain(all) {
                if matches!(sym.kind, prmator_syntax::SymbolKind::Impl) {
                    continue;
                }
                if !seen.insert((fi, sym.name.clone(), sym.container.clone(), sym.start_line, new_side)) {
                    continue;
                }
                let name = match &sym.container {
                    Some(c) => format!("{c}::{}", sym.name),
                    None => sym.name.clone(),
                };
                let Some((score, matched)) = fuzzy(q, &name) else { continue };
                let trailing = change.map(|c| (c.label().to_string(), gpui::transparent_black()));
                items.push(PaletteItem {
                    icon: kind_icon(sym.kind).into(),
                    icon_color: Some(match change {
                        Some(SymbolChange::Added) => gpui::green(),
                        Some(SymbolChange::Removed) => gpui::red(),
                        Some(SymbolChange::Modified) => gpui::yellow(),
                        None => gpui::transparent_black(),
                    }),
                    label: name,
                    matched,
                    detail: format!("{} · {}:{}", kind_label(sym.kind), short_path(path), sym.start_line),
                    trailing,
                    action: PaletteAction::Lines { fi, new_side, lines: sym.start_line..sym.end_line + 1 },
                    // Changed symbols first.
                    score: score + if change.is_some() { 25 } else { 0 },
                    swatch: None,
                });
            }
        }
        if q.is_empty() {
            items.sort_by(|a, b| b.score.cmp(&a.score).then(a.label.cmp(&b.label)));
        }
        items
    }

    fn text_items(&self, q: &str) -> Vec<PaletteItem> {
        let Some(pv) = &self.pr else { return vec![] };
        if q.len() < 2 {
            return vec![];
        }
        let needle = q.to_lowercase();
        let mut items = vec![];
        for (fi, f) in pv.files().iter().enumerate() {
            for line in f.hunks.iter().flat_map(|h| &h.lines) {
                let lower = line.text.to_lowercase();
                let Some(pos) = lower.find(&needle) else { continue };
                let (new_side, n) = match line.new_no {
                    Some(n) => (true, n),
                    None => (false, line.old_no.unwrap_or(0)),
                };
                let trimmed = line.text.trim_start();
                let offset = line.text.len() - trimmed.len();
                let matched = if pos >= offset && lower.len() == line.text.len() {
                    (pos - offset..pos - offset + needle.len()).collect()
                } else {
                    vec![]
                };
                items.push(PaletteItem {
                    icon: match line.kind {
                        crate::diff::LineKind::Added => "+".into(),
                        crate::diff::LineKind::Removed => "−".into(),
                        _ => " ".into(),
                    },
                    icon_color: None,
                    label: trimmed.chars().take(160).collect(),
                    matched,
                    detail: format!("{}:{n}", short_path(&f.path)),
                    trailing: None,
                    action: PaletteAction::Lines { fi, new_side, lines: n..n + 1 },
                    score: 0,
                    swatch: None,
                });
                if items.len() >= MAX_ITEMS {
                    return items;
                }
            }
        }
        items
    }

    pub fn render_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let p = self.palette.as_ref().unwrap();
        let items = p.items.iter().enumerate().map(|(i, item)| {
            let selected = i == p.selected;
            let action = item.action.clone();
            let icon_color = item
                .icon_color
                .filter(|c| c.a > 0.)
                .map(|c| semantic(&t, c))
                .unwrap_or(t.muted);
            let label = {
                let highlights = item
                    .matched
                    .iter()
                    .filter(|&&b| item.label.is_char_boundary(b))
                    .filter_map(|&b| {
                        let len = item.label[b..].chars().next()?.len_utf8();
                        Some((b..b + len, HighlightStyle { color: Some(t.accent), font_weight: Some(FontWeight::BOLD), ..Default::default() }))
                    })
                    .collect::<Vec<_>>();
                StyledText::new(item.label.clone()).with_highlights(highlights)
            };
            div()
                .id(("palette-item", i))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py(px(5.))
                .mx_1()
                .rounded_md()
                .when(selected, |d| d.bg(t.accent_soft))
                .hover(|s| s.bg(t.hover).cursor_pointer())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close_palette(window, cx);
                    this.run_palette_action(action.clone(), window, cx);
                }))
                .child(div().w(px(16.)).flex_none().font_family(t.mono.clone()).text_color(icon_color).child(item.icon.clone()))
                .when_some(item.swatch, |d, colors| {
                    d.child(
                        div()
                            .flex_none()
                            .flex()
                            .rounded(px(3.))
                            .overflow_hidden()
                            .border_1()
                            .border_color(t.border)
                            .children(colors.into_iter().map(|c| div().w(px(9.)).h(px(12.)).bg(c))),
                    )
                })
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_sm().text_color(t.text).child(label))
                .when(!item.detail.is_empty(), |d| {
                    d.child(div().flex_none().text_xs().text_color(t.dim).child(SharedString::from(item.detail.clone())))
                })
                .when_some(item.trailing.clone(), |d, (text, _)| {
                    d.child(div().flex_none().text_xs().text_color(t.muted).font_family(t.mono.clone()).child(SharedString::from(text)))
                })
        });
        let empty = p.items.is_empty();
        let help = match PaletteMode::of(p.input.read(cx).text()).0 {
            PaletteMode::Symbols if self.pr.as_ref().is_some_and(|pv| pv.analysis.is_empty()) => "Analyzing code… symbols appear as files finish.",
            PaletteMode::Text => "Type at least 2 characters to search the diff.",
            _ => "No matches.",
        };
        deferred(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .pt(px(70.))
                .bg(t.shadow.opacity(0.25))
                .occlude()
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, window, cx| this.close_palette(window, cx)))
                .child(
                    div()
                        .id("palette")
                        .key_context("Palette")
                        .on_action(cx.listener(|this, _: &PaletteUp, _, cx| this.palette_move(-1, cx)))
                        .on_action(cx.listener(|this, _: &PaletteDown, _, cx| this.palette_move(1, cx)))
                        .on_action(cx.listener(|this, _: &PaletteClose, window, cx| this.close_palette(window, cx)))
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .w(px(640.))
                        .max_h(px(480.))
                        .flex()
                        .flex_col()
                        .rounded_xl()
                        .bg(t.panel)
                        .border_1()
                        .border_color(t.border_strong)
                        .shadow_lg()
                        .overflow_hidden()
                        .child(div().p_2().border_b_1().border_color(t.border).child(p.input.clone()))
                        .child(
                            div()
                                .id("palette-items")
                                .flex_1()
                                .py_1()
                                .overflow_y_scroll()
                                .track_scroll(&p.scroll)
                                .children(items)
                                .when(empty, |d| d.child(hint(&t, help))),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_3()
                                .px_3()
                                .py_1()
                                .border_t_1()
                                .border_color(t.border)
                                .text_xs()
                                .text_color(t.dim)
                                .child("↑↓ move · ↵ open · esc close")
                                .child(div().flex_1())
                                .child("@ symbols · > commands · / text"),
                        ),
                ),
        )
        .with_priority(2)
    }
}

/// Maps the placeholder colors used while building items to theme colors.
fn semantic(t: &Theme, c: Hsla) -> Hsla {
    if c == gpui::green() {
        t.add_fg
    } else if c == gpui::red() {
        t.del_fg
    } else if c == gpui::yellow() {
        t.warn
    } else {
        c
    }
}

/// Case-insensitive subsequence match. Returns a score (higher is better) and the matched
/// byte positions in `candidate`.
pub fn fuzzy(query: &str, candidate: &str) -> Option<(i32, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, vec![]));
    }
    let q: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();
    let mut matched = vec![];
    let mut score = 0;
    let mut qi = 0;
    let mut prev_match: Option<usize> = None;
    let mut prev_char: Option<char> = None;
    for (ci, (b, c)) in candidate.char_indices().enumerate() {
        if qi < q.len() && c.to_lowercase().next() == Some(q[qi]) {
            score += 10;
            if prev_match == Some(ci.wrapping_sub(1)) {
                score += 15;
            }
            let word_start = match prev_char {
                None => true,
                Some(p) => matches!(p, '/' | '_' | '-' | ':' | '.' | ' ') || (p.is_lowercase() && c.is_uppercase()),
            };
            if word_start {
                score += 20;
            }
            matched.push(b);
            prev_match = Some(ci);
            qi += 1;
        }
        prev_char = Some(c);
    }
    if qi < q.len() {
        return None;
    }
    // Prefer matches in the file name / last segment and shorter candidates.
    let tail = candidate.rfind('/').map_or(0, |i| i + 1);
    if matched.first().is_some_and(|&m| m >= tail) {
        score += 15;
    }
    score -= (candidate.len() as i32) / 8;
    Some((score, matched))
}

#[cfg(test)]
mod tests {
    use super::fuzzy;

    #[test]
    fn fuzzy_prefers_word_starts() {
        let (a, _) = fuzzy("bo", "BatchOperation").unwrap();
        let (b, _) = fuzzy("bo", "about").unwrap();
        assert!(a > b);
        assert!(fuzzy("xyz", "BatchOperation").is_none());
        let (_, m) = fuzzy("hist", "crates/app/src/batch/history.rs").unwrap();
        assert_eq!(m.len(), 4);
    }
}
