//! Story mode UI: the virtual commit being read (above the diff) and the commit list
//! (left column).

use std::collections::BTreeSet;

use gpui::{div, prelude::*, px, AnyElement, Context, FontWeight, SharedString};

use crate::app::Prmator;
use crate::model::PrView;
use crate::story::SavedStory;
use crate::theme::Theme;

use super::components::*;
use super::markdown::{self, MdSize};

/// (files, additions, deletions) of a commit.
fn commit_stats(pv: &PrView, commit: usize) -> (usize, usize, usize) {
    let Some(focus) = &pv.story_focus else { return (0, 0, 0) };
    let mut files = BTreeSet::new();
    let (mut adds, mut dels) = (0, 0);
    for (&(fi, hi), &c) in &focus.owner {
        if c != commit {
            continue;
        }
        files.insert(fi);
        for l in &pv.files()[fi].hunks[hi].lines {
            match l.kind {
                crate::diff::LineKind::Added => adds += 1,
                crate::diff::LineKind::Removed => dels += 1,
                _ => {}
            }
        }
    }
    (files.len(), adds, dels)
}

impl Prmator {
    /// Center of the Story page: intro / progress, or the current commit above its diff.
    pub fn render_story_page(&self, t: &Theme, pv: &PrView, cx: &mut Context<Self>) -> AnyElement {
        let Some(story) = pv.story.as_ref().filter(|_| pv.story_run.is_none()) else {
            return self.render_story_intro(t, pv, cx).into_any_element();
        };
        if pv.story_outdated || pv.story_focus.is_none() {
            return div()
                .p_6()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("This story is out of date"))
                .child(div().text_sm().text_color(t.muted).child(
                    "The PR changed since the story was written, so its commits no longer add up to the diff.",
                ))
                .child(
                    primary_button(t, "story-regen", "✦ Retell the story")
                        .on_click(cx.listener(|this, _, _, cx| this.generate_story(cx))),
                )
                .into_any_element();
        }
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(self.render_commit_card(t, pv, story, cx))
            .child(self.render_diff(t, pv, cx))
            .into_any_element()
    }

    fn render_story_intro(&self, t: &Theme, pv: &PrView, cx: &mut Context<Self>) -> impl IntoElement {
        let hunks = pv.files().iter().map(|f| f.hunks.len()).sum::<usize>();
        div().id("story-intro").flex_1().overflow_y_scroll().child(
            div()
                .max_w(px(680.))
                .mx_auto()
                .px_6()
                .py_8()
                .flex()
                .flex_col()
                .gap_4()
                .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child("📖 Review as a story"))
                .child(div().text_sm().text_color(t.muted).line_height(gpui::rems(1.5)).child(
                    "Claude retells this PR as a sequence of small, atomic virtual commits, each with the WHY \
                     behind it, ordered so that every step builds on the previous ones: foundations first, \
                     then behavior, then wiring and tests.",
                ))
                .child(div().text_sm().text_color(t.muted).line_height(gpui::rems(1.5)).child(SharedString::from(format!(
                    "No code is added or removed: the commits only regroup the PR's {hunks} existing hunks, and \
                     PRmator checks that together they are exactly the diff."
                ))))
                .map(|d| match &pv.story_run {
                    Some(run) => d
                        .child(spinner_text(t, "Claude is reading the PR and writing its story…"))
                        .children(run.activity.iter().map(|a| {
                            div().text_xs().font_family(t.mono.clone()).text_color(t.tool).child(SharedString::from(format!("▸ {a}")))
                        })),
                    None => d.child(
                        div().flex().child(
                            primary_button(t, "story-generate", "✦ Write the story")
                                .on_click(cx.listener(|this, _, _, cx| this.generate_story(cx))),
                        ),
                    ),
                })
                .when_some(pv.story_error.clone(), |d, e| d.child(error_text(t, &e))),
        )
    }

    fn render_commit_card(&self, t: &Theme, pv: &PrView, story: &SavedStory, cx: &mut Context<Self>) -> impl IntoElement {
        let current = pv.story_focus.as_ref().map_or(0, |f| f.current);
        let total = story.commits.len();
        let commit = &story.commits[current];
        let done = story.done.contains(&current);
        let (files, adds, dels) = commit_stats(pv, current);
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .px_5()
            .py_3()
            .bg(t.raised)
            .border_b_1()
            .border_color(t.border)
            // Progress: one segment per commit.
            .child(
                div().flex().gap(px(3.)).children((0..total).map(|i| {
                    let color = if i == current {
                        t.accent
                    } else if story.done.contains(&i) {
                        t.add_fg
                    } else {
                        t.border_strong
                    };
                    div()
                        .id(("story-seg", i))
                        .flex_1()
                        .h(px(5.))
                        .rounded_full()
                        .bg(color)
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.go_to_commit(i, cx)))
                })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(t.muted)
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(t.accent).child(SharedString::from(format!(
                        "Commit {} of {total}",
                        current + 1
                    ))))
                    .child(SharedString::from(format!("· {files} files · ")))
                    .child(div().text_color(t.add_fg).child(SharedString::from(format!("+{adds}"))))
                    .child(div().text_color(t.del_fg).child(SharedString::from(format!("−{dels}"))))
                    .when(done, |d| d.child(badge("✓ reviewed", t.add_fg)))
                    .child(div().flex_1())
                    .child(ghost_button(t, "story-prev", "‹ Prev  ,").on_click(cx.listener(|this, _, _, cx| this.prev_commit(cx))))
                    .child(
                        primary_button(t, "story-next", if current + 1 == total { "✓ Done" } else { "Done · next  ." })
                            .on_click(cx.listener(|this, _, _, cx| this.next_commit(cx))),
                    ),
            )
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(commit.title.clone())),
            )
            .child(markdown::render(t, &commit.why, MdSize::Small, &format!("story-why-{current}")))
            .when(!commit.next.trim().is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .gap_2()
                        .text_sm()
                        .text_color(t.muted)
                        .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child("Next →"))
                        .child(div().flex_1().child(SharedString::from(commit.next.clone()))),
                )
            })
    }

    /// Left column in story mode: the commits, like a git log.
    pub fn render_story_list(&self, t: &Theme, pv: &PrView, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match &pv.story {
            None => div()
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_sm().text_color(t.muted).child("Read the PR as a sequence of small commits, each explaining why."))
                .child(
                    primary_button(t, "story-generate-side", if pv.story_run.is_some() { "◌ Writing…" } else { "✦ Write the story" })
                        .on_click(cx.listener(|this, _, _, cx| this.generate_story(cx))),
                )
                .into_any_element(),
            Some(story) => {
                let current = pv.story_focus.as_ref().map(|f| f.current);
                let total_hunks: usize = story.commits.iter().map(|c| c.hunks.len()).sum();
                div()
                    .flex()
                    .flex_col()
                    .p_2()
                    .gap_1()
                    .child(
                        div()
                            .p_2()
                            .mb_1()
                            .rounded_lg()
                            .bg(t.raised)
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_xs().text_color(t.muted).child(SharedString::from(story.summary.clone())))
                            .child(div().text_xs().text_color(t.add_fg).child(SharedString::from(format!(
                                "✓ {} commits cover all {total_hunks} hunks of the PR{}",
                                story.commits.len(),
                                if story.auto_placed > 0 { format!(" ({} placed by PRmator)", story.auto_placed) } else { String::new() }
                            )))),
                    )
                    .children(story.commits.iter().enumerate().map(|(i, c)| {
                        let done = story.done.contains(&i);
                        let selected = current == Some(i);
                        let last = i + 1 == story.commits.len();
                        div()
                            .id(("story-commit", i))
                            .flex()
                            .gap_2()
                            .px_2()
                            .rounded_lg()
                            .when(selected, |d| d.bg(t.accent_soft))
                            .hover(|s| s.bg(t.hover).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| this.go_to_commit(i, cx)))
                            // git-log rail: node + line
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(18.))
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .pt(px(9.))
                                    .child(
                                        div()
                                            .id(("story-node", i))
                                            .size(px(14.))
                                            .rounded_full()
                                            .border_2()
                                            .border_color(if done { t.add_fg } else if selected { t.accent } else { t.border_strong })
                                            .when(done, |d| d.bg(t.add_fg))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.toggle_commit_done(i, cx)
                                            })),
                                    )
                                    .when(!last, |d| d.child(div().flex_1().w(px(2.)).min_h(px(10.)).bg(t.border))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .py_2()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_sm()
                                            .when(selected, |d| d.font_weight(FontWeight::SEMIBOLD))
                                            .text_color(if done && !selected { t.muted } else { t.text })
                                            .child(SharedString::from(c.title.clone())),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(t.dim)
                                            .font_family(t.mono.clone())
                                            .child(SharedString::from(format!("#{} · {} hunks", i + 1, c.hunks.len()))),
                                    ),
                            )
                    }))
                    .child(
                        div().flex().justify_end().p_2().child(
                            ghost_button(t, "story-regen-side", "↻ Retell")
                                .on_click(cx.listener(|this, _, _, cx| this.generate_story(cx))),
                        ),
                    )
                    .into_any_element()
            }
        };
        div().id("story-commits").flex_1().overflow_y_scroll().child(body)
    }
}
