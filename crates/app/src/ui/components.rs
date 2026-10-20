//! Small reusable UI pieces.

use std::ops::Range;

use gpui::{div, prelude::*, px, Div, ElementId, FontWeight, HighlightStyle, Hsla, SharedString, Stateful, StyledText};
use prmator_syntax::Highlight;
use protocol::ChangeContext;

use crate::analysis::Spans;
use crate::diff::FileStatus;
use crate::theme::Theme;

pub fn button(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .h(px(24.))
        .rounded_md()
        .text_xs()
        .text_color(t.text)
        .bg(t.raised)
        .border_1()
        .border_color(t.border)
        .hover(|s| s.bg(t.hover).border_color(t.border_strong).cursor_pointer())
        .child(label.into())
}

pub fn primary_button(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let hover = t.accent.opacity(0.85);
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .h(px(24.))
        .rounded_md()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.accent_fg)
        .bg(t.accent)
        .hover(move |s| s.bg(hover).cursor_pointer())
        .child(label.into())
}

pub fn ghost_button(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .px_2()
        .h(px(22.))
        .rounded_md()
        .text_xs()
        .text_color(t.muted)
        .hover(|s| s.bg(t.hover).text_color(t.text).cursor_pointer())
        .child(label.into())
}

/// One option of a segmented control.
pub fn segment(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .h(px(20.))
        .flex()
        .items_center()
        .rounded(px(5.))
        .text_xs()
        .when(active, |d| d.bg(t.bg).text_color(t.text).shadow_sm().font_weight(FontWeight::MEDIUM))
        .when(!active, |d| d.text_color(t.muted).hover(|s| s.text_color(t.text).cursor_pointer()))
        .child(label.into())
}

pub fn segmented(t: &Theme) -> Div {
    div().flex().items_center().p(px(2.)).gap(px(2.)).rounded_md().bg(t.raised).border_1().border_color(t.border)
}

pub fn checkbox(t: &Theme, id: impl Into<ElementId>, checked: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(14.))
        .rounded(px(4.))
        .border_1()
        .border_color(if checked { t.accent } else { t.border_strong })
        .when(checked, |d| d.bg(t.accent))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.))
        .text_color(t.accent_fg)
        .cursor_pointer()
        .when(checked, |d| d.child("✓"))
}

pub fn badge(text: impl Into<SharedString>, fg: Hsla) -> Div {
    div()
        .flex_none()
        .px(px(5.))
        .rounded(px(4.))
        .bg(fg.opacity(0.12))
        .text_color(fg)
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

pub fn kbd(t: &Theme, key: &'static str) -> Div {
    div()
        .flex_none()
        .px(px(4.))
        .rounded(px(3.))
        .border_1()
        .border_color(t.border)
        .bg(t.raised)
        .text_size(px(10.))
        .text_color(t.muted)
        .child(key)
}

pub fn avatar(t: &Theme, login: &str, size: f32) -> Div {
    let initial = login.chars().next().unwrap_or('?').to_uppercase().to_string();
    div()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .bg(t.avatar(login))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(size * 0.5))
        .font_weight(FontWeight::BOLD)
        .text_color(gpui::white())
        .child(initial)
}

pub fn status_letter(t: &Theme, status: FileStatus) -> Div {
    let (letter, c) = match status {
        FileStatus::Added => ("A", t.add_fg),
        FileStatus::Deleted => ("D", t.del_fg),
        FileStatus::Modified => ("M", t.warn),
        FileStatus::Renamed => ("R", t.accent),
    };
    div()
        .flex_none()
        .w(px(16.))
        .h(px(16.))
        .rounded(px(4.))
        .bg(c.opacity(0.14))
        .flex()
        .items_center()
        .justify_center()
        .font_family(t.mono.clone())
        .text_size(px(10.))
        .font_weight(FontWeight::BOLD)
        .text_color(c)
        .child(letter)
}

pub fn hint(t: &Theme, text: impl Into<SharedString>) -> Div {
    div().p_4().text_sm().text_color(t.muted).child(text.into())
}

pub fn error_text(t: &Theme, text: &str) -> Div {
    div().p_2().text_xs().text_color(t.del_fg).child(SharedString::from(text.to_string()))
}

pub fn short_path(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub fn context_chip(t: &Theme, ctx: &ChangeContext) -> Div {
    div()
        .flex_none()
        .px_2()
        .py(px(2.))
        .rounded_md()
        .bg(t.accent_soft)
        .text_xs()
        .font_family(t.mono.clone())
        .text_color(t.accent)
        .child(SharedString::from(format!("{} · {}", short_path(&ctx.file), ctx.location)))
}

pub fn spinner_text(t: &Theme, text: impl Into<SharedString>) -> Div {
    div().flex().items_center().gap_2().text_xs().text_color(t.muted).child(div().text_color(t.accent).child("◌")).child(text.into())
}

/// Expands tabs to 4 spaces, remapping byte ranges into the expanded text.
pub fn expand_tabs(text: &str) -> (String, impl Fn(usize) -> usize + '_) {
    let expanded = text.replace('\t', "    ");
    let map = move |ix: usize| ix + text[..ix.min(text.len())].matches('\t').count() * 3;
    (expanded, map)
}

/// A line of code with syntax colors and optional emphasized (changed-word) ranges.
/// Returns the display text (tabs expanded) and its styled element.
pub fn code_line(
    t: &Theme,
    text: &str,
    spans: Option<&Spans>,
    emphasis: Option<&Vec<Range<usize>>>,
    emphasis_bg: Hsla,
) -> (SharedString, StyledText) {
    let (display, styled, _) = code_line_with_spans(t, text, spans, emphasis, emphasis_bg);
    (display, styled)
}

/// Like [`code_line`], also returning the syntax spans in display (tab-expanded) offsets.
pub fn code_line_with_spans(
    t: &Theme,
    text: &str,
    spans: Option<&Spans>,
    emphasis: Option<&Vec<Range<usize>>>,
    emphasis_bg: Hsla,
) -> (SharedString, StyledText, Spans) {
    let (display, map) = expand_tabs(text);
    let len = display.len();
    let fix = |r: &Range<usize>| -> Option<Range<usize>> {
        let (s, e) = (map(r.start.min(text.len())), map(r.end.min(text.len())));
        (s < e && display.is_char_boundary(s) && display.is_char_boundary(e)).then_some(s..e.min(len))
    };
    let syntax: Vec<(Range<usize>, Highlight)> =
        spans.into_iter().flatten().filter_map(|(r, h)| Some((fix(r)?, *h))).collect();
    let emph: Vec<Range<usize>> = emphasis.into_iter().flatten().filter_map(|r| fix(r)).collect();

    // Split into segments at every boundary so the two layers combine without overlap.
    let mut cuts: Vec<usize> = vec![0, len];
    for (r, _) in &syntax {
        cuts.extend([r.start, r.end]);
    }
    for r in &emph {
        cuts.extend([r.start, r.end]);
    }
    cuts.sort();
    cuts.dedup();
    let mut highlights = vec![];
    for w in cuts.windows(2) {
        let seg = w[0]..w[1];
        if seg.is_empty() {
            continue;
        }
        let color = syntax.iter().find(|(r, _)| r.start <= seg.start && seg.end <= r.end).map(|(_, h)| t.syntax(*h));
        let bg = emph.iter().any(|r| r.start <= seg.start && seg.end <= r.end).then_some(emphasis_bg);
        if color.is_some() || bg.is_some() {
            highlights.push((seg, HighlightStyle { color, background_color: bg, ..Default::default() }));
        }
    }
    let display: SharedString = display.into();
    (display.clone(), StyledText::new(display).with_highlights(highlights), syntax)
}
