//! The Conversation page of a PR, laid out like GitHub's: header, a timeline of the
//! description, comments, reviews and inline review threads, and a sidebar with
//! reviewers, labels, checks and review progress. "Start review" enters review mode.

use std::collections::{BTreeMap, HashMap};

use gpui::{div, prelude::*, px, rems, AnyElement, Context, Div, FontWeight, Hsla, SharedString};

use crate::app::{Command, Prmator};
use crate::github::{self, PrDetail, ReviewComment};
use crate::model::{Load, PrPage, PrView};
use crate::theme::Theme;

use super::components::*;
use super::markdown::{self, MdSize};

/// Comments longer than this (lines) are collapsed until expanded.
const COLLAPSE_LINES: usize = 30;
/// Bots get collapsed sooner.
const COLLAPSE_BOT_LINES: usize = 8;
const AVATAR: f32 = 40.;

/// One entry of the timeline, sorted by time.
enum Event<'a> {
    Comment { author: &'a str, body: &'a str, at: &'a str },
    Review { author: &'a str, state: &'a str, body: &'a str, at: &'a str },
    Thread(Vec<&'a ReviewComment>),
}

impl Event<'_> {
    fn at(&self) -> &str {
        match self {
            Event::Comment { at, .. } | Event::Review { at, .. } => at,
            Event::Thread(c) => &c[0].created_at,
        }
    }
}

impl Prmator {
    pub fn render_overview(&self, t: &Theme, pv: &PrView, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match &pv.detail {
            None => spinner_text(t, "Loading pull request…").p_5().into_any_element(),
            Some(d) => div()
                // Fill the pane (up to a readable width) instead of growing to fit
                // long unbreakable lines; those clip or scroll inside their own boxes.
                .w_full()
                .max_w(px(1180.))
                .mx_auto()
                .px_8()
                .py_6()
                .flex()
                .flex_col()
                .gap_5()
                .child(self.render_pr_heading(t, pv, d, cx))
                .child(
                    // Conversation + info column; on narrow panes the info column wraps
                    // below instead of being cut off.
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_8()
                        .items_start()
                        .child(
                            div()
                                .flex_grow()
                                .flex_shrink()
                                .flex_basis(px(520.))
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_5()
                                .child(self.render_timeline(t, pv, d, cx))
                                .child(self.render_review_box(t, pv, cx)),
                        )
                        .child(self.render_sidebar_info(t, pv, d)),
                )
                .into_any_element(),
        };
        div()
            .id("overview")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .bg(t.bg)
            .child(body)
    }

    fn cta(&self, pv: &PrView) -> &'static str {
        if !pv.viewed.is_empty() || pv.plan.as_ref().is_some_and(|p| !p.done.is_empty()) {
            "▶ Continue review"
        } else {
            "▶ Start review"
        }
    }

    fn render_pr_heading(&self, t: &Theme, pv: &PrView, d: &PrDetail, cx: &mut Context<Self>) -> impl IntoElement {
        let (icon, state, color): (&str, &str, Hsla) = match (d.state.as_str(), d.is_draft) {
            ("MERGED", _) => ("⇋", "Merged", gpui::rgb(0x8250df).into()),
            ("CLOSED", _) => ("✕", "Closed", gpui::rgb(0xcf222e).into()),
            (_, true) => ("◌", "Draft", gpui::rgb(0x6e7781).into()),
            _ => ("⎇", "Open", gpui::rgb(0x1f883d).into()),
        };
        let url = pv.summary.url.clone();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .pb_4()
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(rems(1.75))
                            .line_height(rems(2.25))
                            .child(SharedString::from(format!("{}  ", d.title)))
                            .child(div().text_color(t.dim).child(SharedString::from(format!("#{}", pv.key.number)))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .pt_1()
                            .child(button(t, "ov-github", "GitHub ↗").on_click(move |_, _, _| github::open_in_browser(&url)))
                            .child(
                                primary_button(t, "ov-start", self.cta(pv))
                                    .on_click(cx.listener(|this, _, window, cx| this.set_page(PrPage::Files, window, cx))),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(t.muted)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_3()
                            .py(px(5.))
                            .rounded_full()
                            .bg(color)
                            .text_color(gpui::white())
                            .font_weight(FontWeight::MEDIUM)
                            .child(icon)
                            .child(state),
                    )
                    .child(div().text_color(t.text).font_weight(FontWeight::SEMIBOLD).child(SharedString::from(d.author.login.clone())))
                    .child("wants to merge into")
                    .child(branch_chip(t, &d.base_ref_name))
                    .child("from")
                    .child(branch_chip(t, &d.head_ref_name))
                    .child(SharedString::from(format!("· opened {}", github::relative_time(&d.created_at)))),
            )
    }

    fn render_timeline(&self, t: &Theme, pv: &PrView, d: &PrDetail, cx: &mut Context<Self>) -> impl IntoElement {
        let mut events: Vec<Event> = vec![];
        for c in &d.comments {
            events.push(Event::Comment { author: login(&c.author), body: &c.body, at: &c.created_at });
        }
        for r in &d.reviews {
            // Pending reviews are drafts; empty "commented" reviews only wrap inline comments.
            if r.state == "PENDING" || (r.state == "COMMENTED" && r.body.trim().is_empty()) {
                continue;
            }
            events.push(Event::Review {
                author: login(&r.author),
                state: &r.state,
                body: &r.body,
                at: r.submitted_at.as_deref().unwrap_or_default(),
            });
        }
        if let Load::Ready(comments) = &pv.review_comments {
            events.extend(threads(comments).into_iter().map(Event::Thread));
        }
        events.sort_by(|a, b| a.at().cmp(b.at()));

        let pr_author = d.author.login.as_str();
        let description = self.comment_item(
            t,
            pv,
            "description",
            pr_author,
            pr_author,
            "opened this pull request",
            &d.created_at,
            if d.body.trim().is_empty() { None } else { Some(d.body.as_str()) },
            true,
            cx,
        );
        let mut items: Vec<AnyElement> = vec![description.into_any_element()];
        for (i, e) in events.into_iter().enumerate() {
            items.push(match e {
                Event::Comment { author, body, at } => self
                    .comment_item(t, pv, &format!("c{i}"), author, pr_author, "commented", at, Some(body), false, cx)
                    .into_any_element(),
                Event::Review { author, state, body, at } => {
                    self.review_item(t, pv, &format!("r{i}"), author, pr_author, state, body, at, cx).into_any_element()
                }
                Event::Thread(comments) => self.thread_item(t, i, comments, cx).into_any_element(),
            });
        }
        match &pv.review_comments {
            Load::Loading => items.push(rail_note(t, spinner_text(t, "Loading review comments…")).into_any_element()),
            Load::Failed(e) => items.push(rail_note(t, error_text(t, &format!("Review comments: {e}"))).into_any_element()),
            Load::Ready(_) => {}
        }

        // The rail runs behind the avatars; items are drawn on top of it.
        div()
            .relative()
            .child(div().absolute().top_0().bottom_0().left(px(AVATAR / 2. - 1.)).w(px(2.)).bg(t.border))
            .child(div().flex().flex_col().gap_5().children(items))
    }

    #[allow(clippy::too_many_arguments)]
    fn comment_item(
        &self,
        t: &Theme,
        pv: &PrView,
        key: &str,
        author: &str,
        pr_author: &str,
        action: &str,
        at: &str,
        body: Option<&str>,
        description: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_bot = author.ends_with("[bot]") || author.ends_with("-bot") || author.ends_with("ai");
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .bg(if description { t.accent_soft } else { t.raised })
            .border_b_1()
            .border_color(if description { t.accent.opacity(0.35) } else { t.border })
            .text_sm()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(SharedString::from(author.to_string())))
            .child(div().text_color(t.muted).child(SharedString::from(action.to_string())))
            .child(div().text_color(t.dim).child(SharedString::from(github::relative_time(at))))
            .child(div().flex_1())
            .when(author == pr_author && !description, |d| d.child(outline_badge(t, "Author")))
            .when(is_bot, |d| d.child(outline_badge(t, "bot")));
        let content = match body {
            Some(b) => self.collapsible_body(t, pv, key, b, if is_bot { COLLAPSE_BOT_LINES } else { COLLAPSE_LINES }, cx),
            None => div().text_sm().italic().text_color(t.dim).child("No description provided.").into_any_element(),
        };
        div()
            .flex()
            .gap_3()
            .items_start()
            .child(avatar(t, author, AVATAR))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .rounded_lg()
                    .border_1()
                    .border_color(if description { t.accent.opacity(0.5) } else { t.border })
                    .bg(t.bg)
                    .overflow_hidden()
                    .child(header)
                    .child(div().px_4().py_3().child(content)),
            )
    }

    /// Markdown body, collapsed to its first lines when long.
    fn collapsible_body(&self, t: &Theme, pv: &PrView, key: &str, body: &str, limit: usize, cx: &mut Context<Self>) -> AnyElement {
        let cleaned = clean_github_markdown(body);
        let line_count = cleaned.lines().count();
        let expanded = pv.expanded_comments.contains(key);
        let collapsed = line_count > limit && !expanded;
        let shown = if collapsed { cleaned.lines().take(limit.min(12)).collect::<Vec<_>>().join("\n") } else { cleaned };
        let key_owned = key.to_string();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(markdown::render(t, &shown, MdSize::Normal, &format!("ov-{key}")))
            .when(line_count > limit, |d| {
                d.child(
                    ghost_button(
                        t,
                        SharedString::from(format!("ov-more-{key}")),
                        if collapsed { format!("Show more ({} lines) ▾", line_count) } else { "Show less ▴".to_string() },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(pv) = &mut this.pr {
                            if !pv.expanded_comments.remove(&key_owned) {
                                pv.expanded_comments.insert(key_owned.clone());
                            }
                        }
                        cx.notify();
                    })),
                )
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn review_item(
        &self,
        t: &Theme,
        pv: &PrView,
        key: &str,
        author: &str,
        pr_author: &str,
        state: &str,
        body: &str,
        at: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (icon, action, color) = review_style(t, state);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        // Centered on the rail.
                        div().w(px(AVATAR)).flex().justify_center().child(
                            div()
                                .size(px(30.))
                                .rounded_full()
                                .border_2()
                                .border_color(t.bg)
                                .bg(color)
                                .text_color(gpui::white())
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(icon),
                        ),
                    )
                    .child(avatar(t, author, 20.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_1()
                            .text_sm()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(SharedString::from(author.to_string())))
                            .child(div().text_color(t.muted).child(action))
                            .child(div().text_color(t.dim).child(SharedString::from(github::relative_time(at))))
                            .when(author == pr_author, |d| d.child(outline_badge(t, "Author"))),
                    ),
            )
            .when(!body.trim().is_empty(), |d| {
                d.child(
                    div()
                        .ml(px(AVATAR + 12.))
                        .px_4()
                        .py_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(t.border)
                        .bg(t.bg)
                        .child(self.collapsible_body(t, pv, key, body, COLLAPSE_LINES, cx)),
                )
            })
    }

    fn thread_item(&self, t: &Theme, ix: usize, comments: Vec<&ReviewComment>, cx: &mut Context<Self>) -> impl IntoElement {
        let root = comments[0];
        let line = root.line.or(root.original_line);
        let outdated = root.line.is_none();
        let left = root.side.as_deref() == Some("LEFT");
        let path = root.path.clone();
        let first_author = login(&root.user).to_string();
        div()
            .flex()
            .gap_3()
            .items_start()
            .child(avatar(t, &first_author, AVATAR))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .rounded_lg()
                    .border_1()
                    .border_color(t.border)
                    .bg(t.bg)
                    .overflow_hidden()
                    .child(
                        div()
                            .id(("ov-thread", ix))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_4()
                            .py_2()
                            .bg(t.raised)
                            .border_b_1()
                            .border_color(t.border)
                            .text_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(move |this, _, _, cx| this.jump_to_comment(&path, line, left, cx)))
                            .child(div().text_color(t.muted).child("▤"))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .font_family(t.mono.clone())
                                    .text_color(t.accent)
                                    .child(SharedString::from(match line {
                                        Some(n) => format!("{}:{n}", root.path),
                                        None => root.path.clone(),
                                    })),
                            )
                            .when(outdated, |d| d.child(outline_badge(t, "Outdated")))
                            .child(div().text_xs().text_color(t.muted).child("View in diff →")),
                    )
                    .child(hunk_snippet(t, &root.diff_hunk))
                    .children(comments.into_iter().enumerate().map(|(i, c)| {
                        let author = login(&c.user);
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .px_4()
                            .py_3()
                            .when(i > 0, |d| d.border_t_1().border_color(t.border))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_sm()
                                    .child(avatar(t, author, 22.))
                                    .child(div().font_weight(FontWeight::SEMIBOLD).child(SharedString::from(author.to_string())))
                                    .child(div().text_color(t.dim).child(SharedString::from(github::relative_time(&c.created_at)))),
                            )
                            .child(div().pl(px(30.)).child(markdown::render(
                                t,
                                &clean_github_markdown(&c.body),
                                MdSize::Normal,
                                &format!("ov-thread-{ix}-{i}"),
                            )))
                    })),
            )
    }

    /// GitHub's merge box, but for reviewing.
    fn render_review_box(&self, t: &Theme, pv: &PrView, cx: &mut Context<Self>) -> impl IntoElement {
        let files = pv.files().len();
        let (adds, dels) = pv.files().iter().fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions));
        let summary = match &pv.plan {
            Some(p) => format!(
                "{files} files · +{adds} −{dels} · {}/{files} viewed · plan step {}/{}",
                pv.viewed.len(),
                p.done.len().min(p.plan.steps.len()),
                p.plan.steps.len()
            ),
            None => format!("{files} files · +{adds} −{dels} · {}/{files} viewed", pv.viewed.len()),
        };
        div()
            .flex()
            .gap_3()
            .items_start()
            .child(
                div().w(px(AVATAR)).flex().justify_center().child(
                    div()
                        .size(px(AVATAR))
                        .rounded_full()
                        .bg(t.accent)
                        .text_color(t.accent_fg)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_lg()
                        .child("✦"),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(t.accent.opacity(0.5))
                    .bg(t.bg)
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child("Ready to review?"))
                            .child(div().text_sm().text_color(t.muted).child(SharedString::from(summary))),
                    )
                    .when(pv.plan.is_none(), |d| {
                        d.child(button(t, "ov-plan", "✦ Plan review").on_click(
                            cx.listener(|this, _, window, cx| this.run_command(Command::PlanReview, window, cx)),
                        ))
                    })
                    .child(button(t, "ov-ask", "Ask Claude").on_click(
                        cx.listener(|this, _, window, cx| this.run_command(Command::AskAboutPr, window, cx)),
                    ))
                    .child(
                        primary_button(t, "ov-start-bottom", self.cta(pv))
                            .on_click(cx.listener(|this, _, window, cx| this.set_page(PrPage::Files, window, cx))),
                    ),
            )
    }

    fn render_sidebar_info(&self, t: &Theme, pv: &PrView, d: &PrDetail) -> impl IntoElement {
        // Latest submitted review per reviewer, plus pending requests.
        let mut reviewers: BTreeMap<String, String> = BTreeMap::new();
        for r in &d.reviews {
            if r.state != "PENDING" {
                reviewers.insert(login(&r.author).to_string(), r.state.clone());
            }
        }
        for rr in &d.review_requests {
            if let Some(name) = rr.login.clone().or_else(|| rr.name.clone()) {
                reviewers.entry(name).or_insert_with(|| "REQUESTED".into());
            }
        }

        // One entry per check name.
        let mut checks: BTreeMap<String, String> = BTreeMap::new();
        for c in &d.status_check_rollup {
            let name = c.name.clone().or_else(|| c.context.clone()).unwrap_or_default();
            checks.insert(name, c.outcome());
        }
        let is_failure = |o: &str| matches!(o, "failure" | "error" | "timed_out" | "action_required" | "cancelled" | "startup_failure");
        let passed = checks.values().filter(|o| *o == "success").count();
        let failed: Vec<&String> = checks.iter().filter(|(_, o)| is_failure(o)).map(|(n, _)| n).collect();
        let skipped = checks.values().filter(|o| matches!(o.as_str(), "skipped" | "neutral")).count();
        let running: Vec<&String> = checks
            .iter()
            .filter(|(_, o)| !is_failure(o) && !matches!(o.as_str(), "success" | "skipped" | "neutral"))
            .map(|(n, _)| n)
            .collect();

        let files = pv.files().len();
        let viewed_frac = if files == 0 { 0. } else { pv.viewed.len() as f32 / files as f32 };

        div()
            .w(px(280.))
            .flex_shrink_0()
            .flex_none()
            .flex()
            .flex_col()
            .text_sm()
            .child(side_section(t, "Reviewers").children(if reviewers.is_empty() {
                vec![div().text_color(t.dim).child("No reviews yet").into_any_element()]
            } else {
                reviewers
                    .into_iter()
                    .map(|(name, state)| {
                        let (icon, label, color) = match state.as_str() {
                            "APPROVED" => ("✓", "approved", t.add_fg),
                            "CHANGES_REQUESTED" => ("±", "changes", t.del_fg),
                            "COMMENTED" => ("💬", "commented", t.muted),
                            "DISMISSED" => ("–", "dismissed", t.dim),
                            _ => ("●", "requested", t.warn),
                        };
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(avatar(t, &name, 20.))
                            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(SharedString::from(name)))
                            .child(div().text_xs().text_color(color).child(SharedString::from(format!("{icon} {label}"))))
                            .into_any_element()
                    })
                    .collect()
            }))
            .child(side_section(t, "Labels").child(if d.labels.is_empty() {
                div().text_color(t.dim).child("None")
            } else {
                div().flex().flex_wrap().gap_1().children(d.labels.iter().map(|l| label_chip(t, &l.name, &l.color)))
            }))
            .child(
                side_section(t, "Checks")
                    .when(checks.is_empty(), |d| d.child(div().text_color(t.dim).child("No checks")))
                    .when(!checks.is_empty(), |el| {
                        let headline = if !failed.is_empty() {
                            (format!("✗ {} failing", failed.len()), t.del_fg)
                        } else if !running.is_empty() {
                            (format!("◌ {} running", running.len()), t.warn)
                        } else {
                            ("✓ All checks passed".to_string(), t.add_fg)
                        };
                        el.child(div().font_weight(FontWeight::SEMIBOLD).text_color(headline.1).child(SharedString::from(headline.0)))
                            .child(div().text_xs().text_color(t.muted).child(SharedString::from(format!(
                                "{passed} passed · {} failed · {} running · {skipped} skipped",
                                failed.len(),
                                running.len()
                            ))))
                            .children(failed.iter().take(8).map(|n| check_line(t, "✗", n, t.del_fg)))
                            .children(running.iter().take(5).map(|n| check_line(t, "◌", n, t.warn)))
                    }),
            )
            .child(
                side_section(t, "Your review")
                    .child(div().text_color(t.muted).child(SharedString::from(format!("{}/{files} files viewed", pv.viewed.len()))))
                    .child(div().h(px(6.)).rounded_full().bg(t.border).child(
                        div().h_full().rounded_full().w(gpui::relative(viewed_frac)).bg(t.add_fg),
                    ))
                    .when_some(pv.plan.as_ref(), |el, p| {
                        el.child(div().text_color(t.muted).child(SharedString::from(format!(
                            "Plan: {}/{} steps done",
                            p.done.len().min(p.plan.steps.len()),
                            p.plan.steps.len()
                        ))))
                    }),
            )
            .child(
                side_section(t, "Branches")
                    .child(branch_chip(t, &d.head_ref_name))
                    .child(div().text_xs().text_color(t.dim).child("into"))
                    .child(branch_chip(t, &d.base_ref_name)),
            )
    }
}

fn login(author: &Option<github::Author>) -> &str {
    author.as_ref().map_or("ghost", |a| a.login.as_str())
}

fn review_style(t: &Theme, state: &str) -> (&'static str, &'static str, Hsla) {
    match state {
        "APPROVED" => ("✓", "approved these changes", gpui::rgb(0x1f883d).into()),
        "CHANGES_REQUESTED" => ("±", "requested changes", gpui::rgb(0xcf222e).into()),
        "DISMISSED" => ("–", "had their review dismissed", t.dim),
        _ => ("👁", "reviewed", t.muted),
    }
}

/// A small line on the rail (loading/error notes).
fn rail_note(_t: &Theme, content: impl IntoElement) -> Div {
    div().pl(px(AVATAR + 12.)).child(content)
}

/// The last lines of a review comment's diff hunk, with line numbers and diff colors.
fn hunk_snippet(t: &Theme, hunk: &str) -> impl IntoElement {
    let mut lines = hunk.lines();
    let header = lines.next().unwrap_or_default();
    let mut nums = header
        .split_whitespace()
        .skip(1)
        .take(2)
        .map(|r| r[1..].split(',').next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(1));
    let (mut old, mut new) = (nums.next().unwrap_or(1), nums.next().unwrap_or(1));
    let mut rows: Vec<(Option<u32>, Option<u32>, char, String)> = vec![];
    for l in lines {
        let sign = l.chars().next().unwrap_or(' ');
        let text = l.get(1..).unwrap_or_default().replace('\t', "    ");
        match sign {
            '+' => {
                rows.push((None, Some(new), '+', text));
                new += 1;
            }
            '-' => {
                rows.push((Some(old), None, '-', text));
                old += 1;
            }
            _ => {
                rows.push((Some(old), Some(new), ' ', text));
                old += 1;
                new += 1;
            }
        }
    }
    let start = rows.len().saturating_sub(6);
    let num = |n: Option<u32>| {
        div()
            .w(px(t.code_size * 2.8))
            .flex_none()
            .pr_2()
            .flex()
            .justify_end()
            .text_color(t.line_no)
            .child(SharedString::from(n.map(|n| n.to_string()).unwrap_or_default()))
    };
    div()
        .flex()
        .flex_col()
        .py_1()
        .border_b_1()
        .border_color(t.border)
        .font_family(t.mono.clone())
        .text_size(px(t.code_size * 0.88))
        .children(rows.into_iter().skip(start).map(|(o, n, sign, text)| {
            let bg = match sign {
                '+' => Some(t.add_bg),
                '-' => Some(t.del_bg),
                _ => None,
            };
            div()
                .flex()
                .whitespace_nowrap()
                .overflow_hidden()
                .when_some(bg, |d, bg| d.bg(bg))
                .child(num(o))
                .child(num(n))
                .child(div().w(px(14.)).flex_none().text_color(t.dim).child(SharedString::from(sign.to_string())))
                .child(div().text_color(t.text).child(SharedString::from(text)))
        }))
}

fn side_section(t: &Theme, title: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .pb_4()
        .mb_4()
        .border_b_1()
        .border_color(t.border)
        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(t.muted).child(title))
}

fn check_line(t: &Theme, icon: &'static str, name: &str, color: Hsla) -> impl IntoElement {
    div()
        .flex()
        .gap_2()
        .text_xs()
        .child(div().flex_none().text_color(color).child(icon))
        .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_color(t.text).child(SharedString::from(name.to_string())))
}

fn outline_badge(t: &Theme, text: &'static str) -> Div {
    div()
        .px(px(6.))
        .rounded_full()
        .border_1()
        .border_color(t.border_strong)
        .text_xs()
        .text_color(t.muted)
        .child(text)
}

fn branch_chip(t: &Theme, name: &str) -> Div {
    div()
        .px_2()
        .py(px(1.))
        .rounded(px(6.))
        .bg(t.accent_soft)
        .text_color(t.accent)
        .font_family(t.mono.clone())
        .text_xs()
        .child(SharedString::from(name.to_string()))
}

fn label_chip(t: &Theme, name: &str, hex: &str) -> Div {
    let color: Hsla = u32::from_str_radix(hex, 16).map(|v| gpui::rgb(v).into()).unwrap_or(t.muted);
    div()
        .px_2()
        .py(px(1.))
        .rounded_full()
        .border_1()
        .border_color(color.opacity(0.6))
        .bg(color.opacity(0.18))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .child(SharedString::from(name.to_string()))
}

/// Groups inline comments into threads (root comment first, replies in order).
fn threads(comments: &[ReviewComment]) -> Vec<Vec<&ReviewComment>> {
    let mut roots: BTreeMap<u64, Vec<&ReviewComment>> = BTreeMap::new();
    let by_id: HashMap<u64, &ReviewComment> = comments.iter().map(|c| (c.id, c)).collect();
    for c in comments {
        // Follow reply chains to the root.
        let mut root = c;
        while let Some(parent) = root.in_reply_to_id.and_then(|id| by_id.get(&id)) {
            root = parent;
        }
        roots.entry(root.id).or_default().push(c);
    }
    roots
        .into_values()
        .map(|mut t| {
            t.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            t
        })
        .collect()
}

/// Strips what the markdown renderer can't show: HTML comments and HTML tags (their text
/// is kept). Markdown links, task lists and tables are left for the renderer.
pub fn clean_github_markdown(src: &str) -> String {
    // HTML comments (may span lines).
    let mut text = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(start) = rest.find("<!--") {
        text.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    text.push_str(rest);

    const TAGS: &[&str] = &[
        "sub", "sup", "br", "details", "summary", "p", "div", "img", "b", "i", "strong", "em", "kbd", "picture", "source",
        "span", "table", "tr", "td", "th", "thead", "tbody", "ul", "li", "ol", "h1", "h2", "h3", "h4", "a", "hr", "code",
    ];
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '<' {
            let after = &text[i + 1..];
            let name: String = after.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if TAGS.contains(&name.to_lowercase().as_str()) {
                if let Some(end) = after.find('>') {
                    if matches!(name.to_lowercase().as_str(), "br" | "p" | "summary" | "details") {
                        out.push('\n');
                    }
                    let skip_to = i + 1 + end + 1;
                    while chars.peek().is_some_and(|(j, _)| *j < skip_to) {
                        chars.next();
                    }
                    continue;
                }
            }
        }
        out.push(c);
    }
    // Collapse the blank lines left behind by removed blocks.
    let mut cleaned = String::new();
    let mut blank = 0;
    for line in out.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        cleaned.push_str(line);
        cleaned.push('\n');
    }
    cleaned.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_github_markdown() {
        let src = "<!-- summary -->\n**Risk: none**\n<!-- a\nb -->\n\n\nSee [docs](https://x.y)<br>\n- [ ] todo\n<sub>Reviewed `551b270`</sub>\nKeep Vec<T> as is.";
        assert_eq!(
            clean_github_markdown(src),
            "**Risk: none**\n\nSee [docs](https://x.y)\n\n- [ ] todo\nReviewed `551b270`\nKeep Vec<T> as is."
        );
    }

    #[test]
    fn groups_reply_threads() {
        let c = |id, reply: Option<u64>, at: &str| ReviewComment {
            id,
            path: "a.rs".into(),
            line: Some(1),
            original_line: Some(1),
            side: None,
            in_reply_to_id: reply,
            user: None,
            body: String::new(),
            created_at: at.into(),
            diff_hunk: String::new(),
        };
        let comments = vec![c(1, None, "1"), c(2, Some(1), "2"), c(3, None, "3"), c(4, Some(2), "4")];
        let threads = threads(&comments);
        let ids: Vec<Vec<u64>> = threads.iter().map(|t| t.iter().map(|c| c.id).collect()).collect();
        assert_eq!(ids, vec![vec![1, 2, 4], vec![3]]);
    }
}
