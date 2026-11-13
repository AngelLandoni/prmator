//! Left column: PR list, or (with a PR open) its files with symbol outline and the review plan.

use gpui::{div, prelude::*, px, AnyElement, Context, Div, FontWeight, SharedString, Stateful};

use crate::analysis::{kind_icon, SymbolChange};
use crate::app::Prmator;
use crate::github::{self, Filter, MyReviewState, PrSummary};
use crate::model::{LeftTab, Load, PrView};
use crate::stacks::{self, Entry, Stack, StackRow};
use crate::tree::{self, TreeRow};
use crate::theme::{theme, Theme};
use crate::*;

use super::components::*;

/// The background of the next PR row: every other one is shaded.
fn stripe(t: &Theme, row: &std::cell::Cell<usize>) -> gpui::Hsla {
    let n = row.get();
    row.set(n + 1);
    if n % 2 == 1 { t.text.opacity(0.045) } else { gpui::transparent_black() }
}

/// A stack member's position (1 merges first); filled for the PRs in the list.
fn step_label(t: &Theme, step: usize, listed: bool) -> Div {
    div().w(px(26.)).flex_none().flex().justify_center().pt(px(9.)).child(
        div()
            .min_w(px(18.))
            .h(px(18.))
            .px(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .text_xs()
            .font_family(t.mono.clone())
            .when(listed, |d| d.bg(t.accent).text_color(t.accent_fg))
            .when(!listed, |d| d.border_1().border_color(t.border).text_color(t.dim))
            .child(SharedString::from(step.to_string())),
    )
}

impl Prmator {
    pub fn render_pr_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let tabs = Filter::ALL.map(|f| {
            segment(&t, SharedString::from(f.label()), f.label(), f == self.filter)
                .on_click(cx.listener(move |this, _, _, cx| this.set_filter(f, cx)))
        });

        let body: AnyElement = match self.prs.get(&self.filter) {
            None | Some(Load::Loading) => spinner_text(&t, "Loading pull requests…").p_4().into_any_element(),
            Some(Load::Failed(e)) => error_text(&t, e).into_any_element(),
            Some(Load::Ready(prs)) if prs.is_empty() => hint(&t, "Nothing here. 🎉").into_any_element(),
            Some(Load::Ready(prs)) => {
                let open: Vec<PrSummary> = self
                    .open_prs
                    .values()
                    .filter_map(|l| match l {
                        Load::Ready(p) => Some(p.iter().cloned()),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                // Zebra stripes, counted through stacks too.
                let row = std::cell::Cell::new(0);
                div()
                    .flex()
                    .flex_col()
                    .p_2()
                    .gap_1()
                    .children(stacks::group(prs, &open).into_iter().map(|entry| match entry {
                        Entry::Single(pr) => self.render_pr_item(&t, &pr, cx).bg(stripe(&t, &row)).into_any_element(),
                        Entry::Stack(stack) => self.render_stack(&t, &stack, &row, cx),
                    }))
                    .into_any_element()
            }
        };
        let linking = self.open_prs.values().any(|l| matches!(l, Load::Loading));

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
                    .py_2()
                    .border_b_1()
                    .border_color(t.border)
                    .child(segmented(&t).children(tabs))
                    .child(div().flex_1())
                    .child(ghost_button(&t, "refresh", "↻").on_click(
                        cx.listener(|this, _, window, cx| this.refresh(&Refresh, window, cx)),
                    )),
            )
            .when(linking, |d| d.child(div().px_3().pt_2().text_xs().text_color(t.dim).child("Finding stacked PRs…")))
            .child(div().id("pr-list").flex_1().overflow_y_scroll().child(body))
    }

    /// Stacked PRs as one card, in merge order, with every member (the ones in the list
    /// highlighted). Clicking the header folds the card to one line.
    fn render_stack(&self, t: &Theme, stack: &Stack, row_ix: &std::cell::Cell<usize>, cx: &mut Context<Self>) -> AnyElement {
        let (repo, root) = stack.key();
        let key = crate::store::pr_key(&repo, root);
        let collapsed = self.store.collapsed_stacks.contains(&key);
        let mine: Vec<&StackRow> = stack.rows.iter().filter(|r| r.listed).collect();
        let mut title = format!("Stack of {}", stack.rows.len());
        if stack.steps != stack.rows.len() {
            title += &format!(" · {} steps", stack.steps);
        }
        let header = div()
            .id(SharedString::from(format!("stack-{key}")))
            .flex()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .text_xs()
            .text_color(t.muted)
            .hover(|s| s.bg(t.hover).cursor_pointer())
            .on_click(cx.listener(move |this, _, _, cx| {
                let set = &mut this.store.collapsed_stacks;
                if !set.remove(&key) {
                    set.insert(key.clone());
                }
                this.store.save();
                cx.notify();
            }))
            .child(div().flex_none().w(px(10.)).text_color(t.dim).child(if collapsed { "▸" } else { "▾" }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_x_1()
                            .child(div().text_color(t.accent).child("⧉"))
                            .child(div().font_weight(FontWeight::MEDIUM).text_color(t.text).child(SharedString::from(title)))
                            .child(SharedString::from(format!("· {} for you", mine.len()))),
                    )
                    .child(div().text_color(t.dim).child(SharedString::from(format!("onto {} · 1 merges first", stack.base))))
                    .when(collapsed, |d| {
                        let yours = mine.iter().map(|r| format!("{}·#{}", r.step, r.pr.number)).collect::<Vec<_>>().join("  ");
                        d.child(div().text_color(t.accent).child(SharedString::from(yours)))
                    }),
            );

        div()
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(t.border)
            .p_1()
            .child(header)
            .when(!collapsed, |d| {
                d.children(stack.rows.iter().map(|row| {
                    div()
                        .flex()
                        .rounded_lg()
                        .bg(stripe(t, row_ix))
                        .child(step_label(t, row.step, row.listed))
                        .child(self.render_pr_item(t, &row.pr, cx).flex_1().min_w_0().when(!row.listed, |d| d.opacity(0.6)))
                }))
            })
            .into_any_element()
    }

    fn render_pr_item(&self, t: &Theme, pr: &PrSummary, cx: &mut Context<Self>) -> Stateful<Div> {
        let progress = self.progress_label(pr);
        let pr_clone = pr.clone();
        div()
            .id(SharedString::from(format!("{}#{}", pr.repo(), pr.number)))
            .flex()
            .gap_2()
            .px_2()
            .py_2()
            .rounded_lg()
            .hover(|s| s.bg(t.hover).cursor_pointer())
            .on_click(cx.listener(move |this, _, window, cx| this.open_pr(pr_clone.clone(), window, cx)))
            .child(avatar(t, &pr.author.login, 22.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.text)
                            .child(SharedString::from(pr.title.clone())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(t.muted)
                            // Separate pieces, so a narrow row wraps between them (one long
                            // text in a row would run past the column instead).
                            .child(SharedString::from(format!(
                                "{} #{}",
                                pr.repository.name_with_owner.rsplit('/').next().unwrap_or_default(),
                                pr.number
                            )))
                            .child(SharedString::from(format!("· {}", pr.author.login)))
                            .child(SharedString::from(format!("· {}", github::relative_time(&pr.updated_at))))
                            .when(pr.is_draft, |d| d.child(badge("draft", t.dim)))
                            .when_some(progress, |d, p| d.child(badge(p, t.accent)))
                            .when_some(pr.my_review.as_ref(), |d, r| {
                                let color = match (r.rereview, r.state) {
                                    (Some(_), _) => t.warn,
                                    (None, MyReviewState::ChangesRequested) => t.del_fg,
                                    (None, _) => t.add_fg,
                                };
                                d.child(badge(r.label(), color))
                            }),
                    ),
            )
    }

    pub fn render_pr_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let pv = self.pr.as_ref().unwrap();
        let tab = pv.left_tab;
        let plan_label = match &pv.plan {
            Some(p) => format!("Plan {}/{}", p.done.len().min(p.plan.steps.len()), p.plan.steps.len()),
            None => "Plan".into(),
        };
        let files = pv.files().len();
        let files_label = format!("Files {}/{}", pv.viewed.len(), files);
        let story_label = match &pv.story {
            Some(s) => format!("Story {}/{}", s.done.len().min(s.commits.len()), s.commits.len()),
            None => "Story".into(),
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
                    .px_2()
                    .py_2()
                    .border_b_1()
                    .border_color(t.border)
                    .child(ghost_button(&t, "back", "←").on_click(
                        cx.listener(|this, _, window, cx| this.back_to_list(&BackToList, window, cx)),
                    ))
                    .child(
                        segmented(&t)
                            .child(segment(&t, "tab-files", files_label, tab == LeftTab::Files).on_click(
                                cx.listener(|this, _, _, cx| {
                                    if let Some(pv) = &mut this.pr {
                                        pv.left_tab = LeftTab::Files;
                                    }
                                    cx.notify();
                                }),
                            ))
                            .child(segment(&t, "tab-plan", plan_label, tab == LeftTab::Plan).on_click(
                                cx.listener(|this, _, _, cx| {
                                    if let Some(pv) = &mut this.pr {
                                        pv.left_tab = LeftTab::Plan;
                                    }
                                    cx.notify();
                                }),
                            ))
                            .child(segment(&t, "tab-story", story_label, tab == LeftTab::Story).on_click(
                                cx.listener(|this, _, _, cx| {
                                    if let Some(pv) = &mut this.pr {
                                        pv.left_tab = LeftTab::Story;
                                    }
                                    cx.notify();
                                }),
                            )),
                    ),
            )
            .child(progress_bar(&t, pv.viewed.len(), files))
            .child(match tab {
                LeftTab::Files => self.render_file_list(&t, pv, cx).into_any_element(),
                LeftTab::Plan => self.render_plan(&t, pv, cx).into_any_element(),
                LeftTab::Story => self.render_story_list(&t, pv, cx).into_any_element(),
            })
    }

    fn render_file_list(&self, t: &Theme, pv: &PrView, cx: &mut Context<Self>) -> impl IntoElement {
        let files = pv.files();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        let mut list = div().flex().flex_col().py_1();
        for row in tree::rows(&paths, &pv.collapsed_dirs) {
            let (fi, name, depth) = match row {
                TreeRow::Dir { path, name, depth, files } => {
                    list = list.child(self.render_dir_row(t, pv, path, name, depth, files, cx));
                    continue;
                }
                TreeRow::File { fi, name, depth } => (fi, name, depth),
            };
            let file = &files[fi];
            let indent = tree_indent(depth);
            let viewed = pv.viewed.contains(&fi);
            let current = fi == pv.current_file;
            let symbols = pv.analysis.get(&fi).map(|a| &a.changed);
            let symbol_count = symbols.map_or(0, |s| s.len());
            let open = pv.outline_open.contains(&fi);
            list = list.child(
                div()
                    .id(("file", fi))
                    .flex()
                    .items_center()
                    .gap_2()
                    .mx_1()
                    .pr_2()
                    .pl(px(indent))
                    .py(px(4.))
                    .rounded_md()
                    .text_xs()
                    .when(current, |d| d.bg(t.accent_soft))
                    .hover(|s| s.bg(t.hover).cursor_pointer())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.diff_focus);
                        if let Some(pv) = &mut this.pr {
                            if pv.collapsed.remove(&fi) {
                                pv.rebuild_rows();
                            }
                            pv.jump_to_file(fi);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .id(("outline-toggle", fi))
                            .w(px(10.))
                            .flex_none()
                            .text_color(t.dim)
                            .when(symbol_count > 0, |d| {
                                d.child(if open { "▾" } else { "▸" }).on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if let Some(pv) = &mut this.pr {
                                        if !pv.outline_open.remove(&fi) {
                                            pv.outline_open.insert(fi);
                                        }
                                    }
                                    cx.notify();
                                }))
                            }),
                    )
                    .child(status_letter(t, file.status))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .when(viewed, |d| d.opacity(0.45))
                            .text_color(t.text)
                            .when(current, |d| d.font_weight(FontWeight::MEDIUM))
                            .child(SharedString::from(name)),
                    )
                    .child(div().text_color(t.add_fg).child(SharedString::from(format!("+{}", file.additions))))
                    .child(div().text_color(t.del_fg).child(SharedString::from(format!("−{}", file.deletions))))
                    .child(
                        checkbox(t, ("viewed", fi), viewed)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.set_viewed(fi, false, cx)
                            })),
                    ),
            );
            if open {
                for (si, sym) in symbols.into_iter().flatten().enumerate() {
                    let color = match sym.change {
                        SymbolChange::Added => t.add_fg,
                        SymbolChange::Modified => t.warn,
                        SymbolChange::Removed => t.del_fg,
                    };
                    let (new_side, lines) = (sym.new_side, sym.symbol.start_line..sym.symbol.end_line + 1);
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("sym-{fi}-{si}")))
                            .flex()
                            .items_center()
                            .gap_2()
                            .ml(px(indent + 30.))
                            .mr_1()
                            .px_2()
                            .py(px(2.))
                            .rounded_md()
                            .text_xs()
                            .hover(|s| s.bg(t.hover).cursor_pointer())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.focus(&this.diff_focus);
                                if let Some(pv) = &mut this.pr {
                                    pv.jump_to_lines(fi, new_side, lines.clone());
                                }
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .w(px(12.))
                                    .font_family(t.mono.clone())
                                    .text_color(color)
                                    .child(kind_icon(sym.symbol.kind)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .font_family(t.mono.clone())
                                    .text_color(t.text)
                                    .when(sym.change == SymbolChange::Removed, |d| d.line_through())
                                    .child(SharedString::from(sym.display_name())),
                            )
                            .child(div().text_color(t.dim).child(sym.change.label())),
                    );
                }
            }
        }
        div()
            .id("file-list")
            .flex_1()
            .overflow_y_scroll()
            .child(list)
            .when(pv.analysis_running, |d| d.child(spinner_text(t, "Analyzing code…").px_3().py_2()))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_dir_row(
        &self,
        t: &Theme,
        pv: &PrView,
        path: String,
        name: String,
        depth: usize,
        files: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let collapsed = pv.collapsed_dirs.contains(&path);
        let all_viewed = files.iter().all(|f| pv.viewed.contains(f));
        let (adds, dels) = files.iter().fold((0, 0), |(a, d), &fi| {
            let f = &pv.files()[fi];
            (a + f.additions, d + f.deletions)
        });
        let holds_current = collapsed && files.contains(&pv.current_file);
        let toggle_path = path.clone();
        div()
            .id(SharedString::from(format!("dir-{path}")))
            .flex()
            .items_center()
            .gap_2()
            .mx_1()
            .pr_2()
            .pl(px(tree_indent(depth)))
            .py(px(4.))
            .rounded_md()
            .text_xs()
            .when(holds_current, |d| d.bg(t.accent_soft))
            .hover(|s| s.bg(t.hover).cursor_pointer())
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_dir(toggle_path.clone(), cx)))
            .child(div().w(px(10.)).flex_none().text_color(t.dim).child(if collapsed { "▸" } else { "▾" }))
            .child(folder_icon(t, !collapsed))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(t.text)
                    .font_weight(FontWeight::MEDIUM)
                    .when(all_viewed, |d| d.opacity(0.45))
                    .child(SharedString::from(name)),
            )
            .when(collapsed, |d| {
                d.child(div().text_color(t.add_fg).child(SharedString::from(format!("+{adds}"))))
                    .child(div().text_color(t.del_fg).child(SharedString::from(format!("−{dels}"))))
            })
            .child(
                checkbox(t, SharedString::from(format!("dir-viewed-{path}")), all_viewed).on_click(cx.listener(
                    move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_files_viewed(&files, !all_viewed, cx)
                    },
                )),
            )
    }

    fn render_plan(&self, t: &Theme, pv: &PrView, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = if let Some(run) = &pv.plan_run {
            div()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(spinner_text(t, "Claude is reading the PR and planning the review…"))
                .children(run.activity.iter().map(|a| {
                    div().text_xs().font_family(t.mono.clone()).text_color(t.tool).child(SharedString::from(format!("▸ {a}")))
                }))
                .into_any_element()
        } else if let Some(saved) = &pv.plan {
            let head = pv.detail.as_ref().map(|d| d.head_ref_oid.as_str()).unwrap_or_default();
            let outdated = !saved.head_sha.is_empty() && !head.is_empty() && saved.head_sha != head;
            let steps = saved.plan.steps.iter().enumerate().map(|(i, step)| {
                let done = saved.done.contains(&i);
                let current = pv.current_step == Some(i);
                let last = i + 1 == saved.plan.steps.len();
                let node_color = if done { t.add_fg } else if current { t.accent } else { t.border_strong };
                div()
                    .id(("step", i))
                    .flex()
                    .gap_2()
                    .px_2()
                    .rounded_lg()
                    .when(current, |d| d.bg(t.accent_soft))
                    .hover(|s| s.bg(t.hover).cursor_pointer())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.diff_focus);
                        this.go_to_step(i, cx)
                    }))
                    // pipeline rail: node + connector
                    .child(
                        div()
                            .flex_none()
                            .w(px(20.))
                            .flex()
                            .flex_col()
                            .items_center()
                            .pt(px(8.))
                            .child(
                                div()
                                    .id(("step-node", i))
                                    .size(px(18.))
                                    .rounded_full()
                                    .border_2()
                                    .border_color(node_color)
                                    .when(done, |d| d.bg(t.add_fg))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(9.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if done { t.bg } else { node_color })
                                    .child(SharedString::from(if done { "✓".to_string() } else { (i + 1).to_string() }))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.set_step_done(i, !done, cx)
                                    })),
                            )
                            .when(!last, |d| d.child(div().flex_1().w(px(2.)).min_h(px(12.)).bg(t.border))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .py_2()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if done { t.muted } else { t.text })
                                    .child(SharedString::from(step.title.clone())),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(t.mono.clone())
                                    .text_color(t.accent)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(SharedString::from(match &step.symbol {
                                        Some(s) if !s.is_empty() => format!("{} · {s}", short_path(&step.file)),
                                        _ => short_path(&step.file).to_string(),
                                    })),
                            )
                            .when(current || !done, |d| {
                                d.child(div().text_xs().text_color(t.muted).child(SharedString::from(step.why.clone())))
                            })
                            .when(current && !step.check.is_empty(), |d| {
                                d.child(
                                    div()
                                        .mt_1()
                                        .p_2()
                                        .rounded_md()
                                        .bg(t.raised)
                                        .text_xs()
                                        .text_color(t.text)
                                        .child(SharedString::from(format!("Check: {}", step.check))),
                                )
                            })
                            .when(current && !step.related_files.is_empty(), |d| {
                                d.child(div().flex().flex_wrap().gap_1().children(step.related_files.iter().enumerate().map(
                                    |(ri, f)| {
                                        let path = f.clone();
                                        ghost_button(t, SharedString::from(format!("rel-{i}-{ri}")), short_path(f).to_string())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                if let Some(pv) = &mut this.pr {
                                                    if let Some(fi) = pv.file_index(&path) {
                                                        if pv.collapsed.remove(&fi) {
                                                            pv.rebuild_rows();
                                                        }
                                                        pv.jump_to_file(fi);
                                                    }
                                                }
                                                cx.notify();
                                            }))
                                    },
                                )))
                            })
                            .when(current, |d| {
                                d.child(
                                    div().flex().gap_2().mt_1().child(
                                        primary_button(t, ("step-next", i), "Done · next step ]").on_click(cx.listener(
                                            |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.next_step(&NextStep, window, cx)
                                            },
                                        )),
                                    ),
                                )
                            }),
                    )
            });
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
                        .text_xs()
                        .text_color(t.muted)
                        .child(SharedString::from(saved.plan.overview.clone())),
                )
                .when(outdated, |d| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .p_2()
                            .text_xs()
                            .text_color(t.warn)
                            .child("PR has new commits since this plan.")
                            .child(button(t, "replan", "Re-plan").on_click(cx.listener(|this, _, _, cx| this.plan_review(cx)))),
                    )
                })
                .children(steps)
                .child(
                    div().flex().justify_end().p_2().child(
                        ghost_button(t, "replan-2", "↻ Re-plan").on_click(cx.listener(|this, _, _, cx| this.plan_review(cx))),
                    ),
                )
                .into_any_element()
        } else {
            div()
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("Where should I start?"))
                .child(div().text_xs().text_color(t.muted).child(
                    "Claude reads the PR and lays out a review route: the entry point first, then the code it \
                     drives, then models and wiring, tests last. Your progress is saved, so you can continue later.",
                ))
                .child(
                    primary_button(t, "plan-start", "✦ Plan review with Claude")
                        .on_click(cx.listener(|this, _, _, cx| this.plan_review(cx))),
                )
                .into_any_element()
        };
        div()
            .id("plan-steps")
            .flex_1()
            .overflow_y_scroll()
            .when_some(pv.plan_error.clone(), |d, e| d.child(error_text(t, &e)))
            .child(body)
    }
}

/// Left padding of a tree row at `depth`.
pub(super) fn tree_indent(depth: usize) -> f32 {
    8. + depth as f32 * 14.
}

pub(super) fn folder_icon(t: &Theme, open: bool) -> impl IntoElement {
    let color = t.accent.opacity(if open { 0.6 } else { 0.45 });
    div()
        .flex_none()
        .w(px(14.))
        .h(px(11.))
        .flex()
        .flex_col()
        .child(div().w(px(6.)).h(px(2.)).rounded_t(px(1.)).bg(color))
        .child(div().w_full().flex_1().rounded(px(2.)).bg(color))
}

fn progress_bar(t: &Theme, done: usize, total: usize) -> impl IntoElement {
    let frac = if total == 0 { 0. } else { done as f32 / total as f32 };
    div().h(px(2.)).w_full().bg(t.border).child(div().h_full().w(gpui::relative(frac)).bg(t.add_fg))
}

