//! The root view's state and behavior. Rendering lives in `ui/`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    SharedString, TextLayout,
    ClipboardItem, Context, Entity, FocusHandle, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    Point, ScrollHandle, ScrollStrategy, Task, UniformListScrollHandle, Window,
};
use protocol::{ChangeContext, Msg};

use crate::analysis::{self, FileAnalysis, Revisions, Spans};
use crate::browser::{FileBrowser, SourceFile};
use crate::selection::{Caret, Dir, TokenSel};
use prmator_syntax::Highlight;
use crate::defs::{self, Definition, KnownFile};
use crate::diff;
use crate::github::{self, Filter, PrSummary};
use crate::link::{self, Link, LinkEvent};
use crate::model::{ExpandDir, LeftTab, Load, PlanRun, PrKey, PrPage, PrView, Row};
use crate::store::{DiffMode, SavedPlan, Store};
use crate::text_input::{Submitted, TextInput};
use crate::settings::Settings;
use crate::theme::{Theme, ThemeMode};
use crate::themes::Themes;
use crate::chat::{ChatPopover, Conversation};
use crate::ui::palette::Palette;
use crate::*;

/// Files with more diff lines than this start collapsed.
const AUTO_COLLAPSE_LINES: usize = 1500;
const HOVER_DELAY: Duration = Duration::from_millis(450);
const HOVER_HIDE_DELAY: Duration = Duration::from_millis(250);

pub struct Menu {
    pub position: Point<Pixels>,
    pub context: ChangeContext,
    /// Which code view was right-clicked.
    pub view: CodeView,
    /// The diff row right-clicked, for "Add comment".
    pub row: Option<usize>,
    /// Identifier under the mouse, when the right-click landed on one.
    pub symbol: Option<String>,
    /// The token selection's text, for "Copy selection".
    pub selected_text: Option<String>,
    /// The code's place in the head version (path, line), for "Open in file view".
    pub open_at: Option<(String, u32)>,
}

/// The two code views that support token selection and the right-click menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeView {
    Diff,
    /// The full-file view.
    Source,
}

/// Code text rendered on a diff row, kept so mouse positions can be mapped to characters.
pub struct CodeHit {
    pub layout: TextLayout,
    /// Display text (tabs expanded).
    pub text: SharedString,
    /// Syntax spans in display offsets.
    pub syntax: Spans,
    /// 0 = left (or unified), 1 = right side of a split row.
    pub side: usize,
}

/// Where `position` falls in a row's code: (side, byte index in the display text). `side`
/// restricts it to one side; otherwise `None` over the gutter (left of the code).
pub fn text_index_at(hits: &[CodeHit], position: Point<Pixels>, side: Option<usize>) -> Option<(usize, usize)> {
    hits.iter()
        .filter(|h| side.is_none_or(|s| h.side == s))
        .filter(|h| side.is_some() || h.layout.bounds().left() <= position.x)
        .max_by(|a, b| a.layout.bounds().left().partial_cmp(&b.layout.bounds().left()).unwrap_or(std::cmp::Ordering::Equal))
        .map(|h| (h.side, h.layout.index_for_position(position).unwrap_or_else(|i| i)))
}

/// The identifier under `position` on a row, if any, with its syntax highlight.
pub fn symbol_at(hits: &[CodeHit], position: Point<Pixels>) -> Option<(String, Option<Highlight>)> {
    hits.iter().find_map(|hit| {
        let ix = hit.layout.index_for_position(position).ok()?;
        let (range, name) = defs::identifier_at(&hit.text, ix)?;
        let highlight = hit.syntax.iter().find(|(r, _)| r.start <= range.start && range.end <= r.end).map(|(_, h)| *h);
        Some((name, highlight))
    })
}

pub const QUICK_ASKS: &[(&str, &str)] = &[
    ("Explain this change", "Explain what this change does and why it was likely made."),
    ("Find bugs & risks", "Review this change for bugs, edge cases and risks. Be specific and cite lines."),
    ("Suggest a simpler version", "Could this be simpler or more idiomatic? Show the improved code."),
    ("What could this break?", "What other code or behavior could this change break? What should I verify?"),
];

/// Quick questions about code in the full-file view (not a change).
pub const QUICK_ASKS_SOURCE: &[(&str, &str)] = &[
    ("Explain this code", "Explain what this code does and how it fits into the rest of the codebase."),
    ("How does the PR affect it?", "Does this PR change or affect this code (directly or through what it calls)? How?"),
    ("Find bugs & risks", "Review this code for bugs, edge cases and risks. Be specific and cite lines."),
    ("Where is it used?", "Where and how is this code used in the repo? Use `gh` or search the code to find out."),
];

pub enum DefState {
    Loading,
    Found(Arc<Definition>),
    NotFound,
}

pub struct HoverCard {
    pub name: String,
    pub position: Point<Pixels>,
    pub over_card: bool,
}

/// Everything the command palette can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    PlanReview,
    ContinueReview,
    NextStep,
    PrevStep,
    ReviewWithClaude,
    UnifiedDiff,
    SplitDiff,
    ThemeAuto,
    ThemeLight,
    ThemeDark,
    SelectTheme,
    OpenSettings,
    MarkViewed,
    NextFile,
    PrevFile,
    CollapseAll,
    ExpandAll,
    Refresh,
    OpenOnGitHub,
    AskAboutPr,
    ReviewAsStory,
    OpenFileView,
    BackToList,
}

impl Command {
    pub const ALL: &[Command] = &[
        Command::PlanReview,
        Command::ContinueReview,
        Command::NextStep,
        Command::PrevStep,
        Command::ReviewWithClaude,
        Command::UnifiedDiff,
        Command::SplitDiff,
        Command::ThemeAuto,
        Command::ThemeLight,
        Command::ThemeDark,
        Command::SelectTheme,
        Command::OpenSettings,
        Command::MarkViewed,
        Command::NextFile,
        Command::PrevFile,
        Command::CollapseAll,
        Command::ExpandAll,
        Command::Refresh,
        Command::OpenOnGitHub,
        Command::AskAboutPr,
        Command::ReviewAsStory,
        Command::OpenFileView,
        Command::BackToList,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Command::PlanReview => "Plan review with Claude",
            Command::ContinueReview => "Continue review (next plan step)",
            Command::NextStep => "Complete step and go to next",
            Command::PrevStep => "Previous plan step",
            Command::ReviewWithClaude => "Ask Claude for a PR overview",
            Command::UnifiedDiff => "Diff: unified",
            Command::SplitDiff => "Diff: side by side",
            Command::ThemeAuto => "Theme mode: follow system",
            Command::ThemeLight => "Theme mode: light",
            Command::ThemeDark => "Theme mode: dark",
            Command::SelectTheme => "Theme: select…",
            Command::OpenSettings => "Settings…",
            Command::MarkViewed => "Mark current file viewed",
            Command::NextFile => "Next file",
            Command::PrevFile => "Previous file",
            Command::CollapseAll => "Collapse all files",
            Command::ExpandAll => "Expand all files",
            Command::Refresh => "Refresh",
            Command::OpenOnGitHub => "Open PR on GitHub",
            Command::AskAboutPr => "Ask Claude about the PR…",
            Command::ReviewAsStory => "Review as a story (virtual commits)",
            Command::OpenFileView => "Open file view (full files + project tree)",
            Command::BackToList => "Back to pull requests",
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            Command::ContinueReview => "c",
            Command::NextStep => "]",
            Command::PrevStep => "[",
            Command::UnifiedDiff | Command::SplitDiff => "s",
            Command::MarkViewed => "v",
            Command::NextFile => "j",
            Command::PrevFile => "k",
            Command::OpenFileView => "f",
            Command::Refresh => "⌘R",
            Command::BackToList => "⌘[",
            _ => "",
        }
    }

    pub fn needs_pr(self) -> bool {
        !matches!(
            self,
            Command::ThemeAuto
                | Command::ThemeLight
                | Command::ThemeDark
                | Command::SelectTheme
                | Command::OpenSettings
                | Command::Refresh | Command::UnifiedDiff | Command::SplitDiff
        )
    }
}

pub struct Prmator {
    pub store: Store,
    /// Settings in effect (PRmator's own, or Zed's).
    pub settings: Settings,
    /// macOS light/dark appearance, for "follow system" theme mode.
    pub appearance: gpui::WindowAppearance,
    pub settings_window: Option<gpui::WindowHandle<crate::ui::settings_window::SettingsView>>,
    pub filter: Filter,
    pub prs: HashMap<Filter, Load<Vec<PrSummary>>>,
    /// All open PRs of the repos in the lists, to complete stacks: repo -> PRs.
    pub open_prs: HashMap<String, Load<Vec<PrSummary>>>,
    pub pr: Option<PrView>,
    /// The full-file view, when open (it replaces the file list and the diff).
    pub browser: Option<FileBrowser>,
    /// The last closed file view, reopened as it was.
    browser_cache: Option<FileBrowser>,
    /// Conversations per PR, kept while switching between PRs.
    /// Claude conversations per PR (open chat + review log).
    pub conversations: HashMap<PrKey, Vec<Conversation>>,
    /// Bridge request id -> (PR, conversation) it answers.
    pub requests: HashMap<u64, (PrKey, u64)>,
    /// The floating chat, when open.
    pub chat: Option<ChatPopover>,
    pub next_id: u64,
    pub menu: Option<Menu>,
    pub palette: Option<Palette>,
    /// ⌘F find bar over the diff.
    pub find: Option<crate::find::FindBar>,
    pub hover: Option<HoverCard>,
    hover_task: Option<Task<()>>,
    /// Identifier waiting for the hover delay.
    hover_pending: Option<String>,
    pub defs: HashMap<(String, String), DefState>,
    pub dragging: bool,
    /// Mouse is down on an empty toolbar area (a move starts a window drag).
    pub titlebar_drag: bool,
    pub input: Entity<TextInput>,
    /// The comment box (floating, on diff lines) and the review summary.
    pub comment_input: Entity<TextInput>,
    pub review_input: Entity<TextInput>,
    pub comment_popover: Option<crate::comments::CommentPopover>,
    /// The signed-in GitHub user.
    pub viewer: Option<String>,
    pub review_panel: Option<crate::comments::ReviewPanel>,
    pub root_focus: FocusHandle,
    pub diff_focus: FocusHandle,
    pub log_scroll: ScrollHandle,
    pub link: Link,
    pub connected: bool,
    pub claude_version: Option<String>,
    pub bridge_error: Option<String>,
}

impl Prmator {
    pub fn new(initial: Option<(String, u64)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut store = Store::load();
        let settings = store.effective_settings();
        let appearance = window.appearance();
        let theme = resolve_theme(&settings, appearance, cx);
        cx.set_global(theme);
        cx.observe_window_appearance(window, |this, window, cx| {
            this.appearance = window.appearance();
            this.apply_theme(cx);
        })
        .detach();

        let (link, events) = link::start();
        cx.spawn(async move |this, cx| {
            while let Ok(ev) = events.recv().await {
                if this.update(cx, |this, cx| this.on_link(ev, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        // Wraps and takes ⇧↵ for new lines; ↵ still sends.
        let input = cx.new(|cx| TextInput::new("Ask Claude…", cx).multiline(false));
        cx.subscribe(&input, |this, _, Submitted(text), cx| this.submit_chat(text.clone(), cx)).detach();
        // Review comments: Enter adds a line, ⌘↵ saves the comment to the review.
        let comment_input = cx.new(|cx| TextInput::new("Leave a comment…", cx).multiline(true));
        cx.subscribe_in(&comment_input, window, |this, _, Submitted(text), window, cx| {
            this.add_to_review_body(text.clone(), window, cx)
        })
        .detach();
        let review_input = cx.new(|cx| TextInput::new("Leave a summary (optional)…", cx).multiline(true));
        cx.subscribe(&review_input, |this, _, Submitted(text), cx| this.submit_review_with(text.clone(), cx)).detach();

        debug_log(format_args!("window bounds {:?}", window.bounds()));
        let root_focus = cx.focus_handle();
        let diff_focus = cx.focus_handle();
        window.focus(&root_focus);

        let mut this = Prmator {
            store,
            settings,
            appearance,
            settings_window: None,
            filter: Filter::ReviewRequested,
            prs: HashMap::new(),
            open_prs: HashMap::new(),
            pr: None,
            browser: None,
            browser_cache: None,
            conversations: HashMap::new(),
            requests: HashMap::new(),
            chat: None,
            next_id: 1,
            menu: None,
            palette: None,
            find: None,
            hover: None,
            hover_task: None,
            hover_pending: None,
            defs: HashMap::new(),
            dragging: false,
            titlebar_drag: false,
            input,
            comment_input,
            review_input,
            comment_popover: None,
            viewer: None,
            review_panel: None,
            root_focus,
            diff_focus,
            log_scroll: ScrollHandle::new(),
            link,
            connected: false,
            claude_version: None,
            bridge_error: None,
        };
        let viewer = cx.background_executor().spawn(async { github::viewer_login().ok() });
        cx.spawn(async move |this, cx| {
            let login = viewer.await;
            let _ = this.update(cx, |this, _| this.viewer = login);
        })
        .detach();
        this.load_prs(Filter::ReviewRequested, cx);
        if let Some((repo, number)) = initial {
            this.open_pr(PrSummary::stub(repo, number), window, cx);
        }
        this
    }

    pub fn next_request_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // ---- settings

    pub fn set_theme(&mut self, mode: ThemeMode, cx: &mut Context<Self>) {
        self.update_settings(|s| s.theme_mode = mode, cx);
    }

    /// Changes settings from PRmator (palette, settings window): they become PRmator's own
    /// and stop following Zed.
    pub fn update_settings(&mut self, change: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        change(&mut self.settings);
        self.settings = self.settings.clone().clamp();
        self.store.settings = Some(self.settings.clone());
        self.store.save();
        self.apply_theme(cx);
    }

    /// Goes back to following Zed's settings.
    pub fn reset_settings_to_zed(&mut self, cx: &mut Context<Self>) {
        self.store.settings = None;
        self.store.save();
        self.settings = Settings::from_zed();
        self.apply_theme(cx);
    }

    /// Installs the theme and typography for the settings and system appearance.
    pub fn apply_theme(&mut self, cx: &mut Context<Self>) {
        let theme = resolve_theme(&self.settings, self.appearance, cx);
        cx.set_global(theme);
        cx.refresh_windows();
        cx.notify();
    }

    /// Name of the theme currently in effect.
    pub fn active_theme_name(&self) -> &str {
        if self.settings.theme_mode.is_dark(self.appearance) {
            &self.settings.dark_theme
        } else {
            &self.settings.light_theme
        }
    }

    /// Picks a theme by name: it becomes the light or dark theme (by its appearance), and
    /// the mode switches to show it unless "Auto" would already pick it.
    pub fn select_theme(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(dark) = cx.global::<Themes>().get(name).map(|e| e.dark) else { return };
        let appearance = self.appearance;
        self.update_settings(
            |s| {
                if dark {
                    s.dark_theme = name.to_string();
                } else {
                    s.light_theme = name.to_string();
                }
                if s.theme_mode.is_dark(appearance) != dark {
                    s.theme_mode = if dark { ThemeMode::Dark } else { ThemeMode::Light };
                }
            },
            cx,
        );
    }

    /// Shows a theme without saving it (palette preview).
    pub fn preview_theme(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(entry) = cx.global::<Themes>().get(name) {
            let theme = entry.theme.clone().with_settings(&self.settings);
            cx.set_global(theme);
            cx.notify();
        }
    }

    pub fn set_diff_mode(&mut self, mode: DiffMode, cx: &mut Context<Self>) {
        self.store.diff_mode = mode;
        self.store.save();
        if let Some(pv) = &mut self.pr {
            let fi = pv.current_file;
            pv.mode = mode;
            pv.rebuild_rows();
            pv.jump_to_file(fi);
        }
        cx.notify();
    }

    pub fn toggle_diff_mode(&mut self, _: &ToggleDiffMode, _: &mut Window, cx: &mut Context<Self>) {
        let mode = match self.store.diff_mode {
            DiffMode::Unified => DiffMode::Split,
            DiffMode::Split => DiffMode::Unified,
        };
        self.set_diff_mode(mode, cx);
    }

    pub fn cycle_theme(&mut self, _: &CycleTheme, _: &mut Window, cx: &mut Context<Self>) {
        self.set_theme(self.settings.theme_mode.next(), cx);
    }

    // ---- data loading

    pub fn load_prs(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.prs.insert(filter, Load::Loading);
        let task = cx.background_executor().spawn(async move { github::list_prs(filter) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(prs) = &result {
                    let repos: HashSet<String> = prs.iter().map(|p| p.repo().to_string()).collect();
                    for repo in repos {
                        this.load_open_prs(repo, cx);
                    }
                }
                this.prs.insert(
                    filter,
                    match result {
                        Ok(prs) => Load::Ready(prs),
                        Err(e) => Load::Failed(format!("{e:#}")),
                    },
                );
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Reloads the PR lists (the current one now, the others when next shown), e.g. after
    /// a review was submitted so its ✓ shows.
    pub fn reload_pr_lists(&mut self, cx: &mut Context<Self>) {
        self.prs.clear();
        self.open_prs.clear();
        self.load_prs(self.filter, cx);
    }

    /// Fetches a repo's open PRs (once), so stacks show the members not in the list.
    fn load_open_prs(&mut self, repo: String, cx: &mut Context<Self>) {
        if self.open_prs.contains_key(&repo) {
            return;
        }
        self.open_prs.insert(repo.clone(), Load::Loading);
        let task = cx.background_executor().spawn({
            let repo = repo.clone();
            async move { github::open_prs(&repo) }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = &result {
                    debug_log(format_args!("open PRs of {repo}: {e:#}"));
                }
                this.open_prs.insert(repo, result.map_or_else(|e| Load::Failed(format!("{e:#}")), Load::Ready));
                cx.notify();
            });
        })
        .detach();
    }

    pub fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.filter = filter;
        if !self.prs.contains_key(&filter) {
            self.load_prs(filter, cx);
        }
        cx.notify();
    }

    pub fn open_pr(&mut self, summary: PrSummary, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_chat(window, cx);
        let key = PrKey { repo: summary.repo().to_string(), number: summary.number };
        self.load_log(&key);
        let mut pv = PrView::new(key.clone(), summary, self.store.diff_mode);
        pv.plan = self.store.progress(&key.store_key()).plan.clone();
        pv.story = self.store.progress(&key.store_key()).story.clone();
        pv.drafts = self.store.progress(&key.store_key()).drafts.clone();
        if pv.plan.is_some() {
            pv.left_tab = LeftTab::Plan;
        }
        self.pr = Some(pv);
        self.browser = None;
        self.browser_cache = None;
        self.menu = None;
        self.hover = None;
        window.focus(&self.diff_focus);
        self.fetch_pr(key, cx);
        cx.notify();
    }

    fn fetch_pr(&mut self, key: PrKey, cx: &mut Context<Self>) {
        let (repo, number) = (key.repo.clone(), key.number);
        let detail = cx.background_executor().spawn({
            let repo = repo.clone();
            async move {
                // Detail, then the diff, then detail again: if the PR was pushed to in
                // between, the diff may not match the head, so go again. Comments are
                // anchored to lines of this diff at this head.
                let mut detail = github::pr_detail(&repo, number);
                let mut diff = github::pr_diff(&repo, number);
                for _ in 0..2 {
                    let Ok(before) = &detail else { break };
                    let Ok(after) = github::pr_detail(&repo, number) else { break };
                    let moved = after.head_ref_oid != before.head_ref_oid;
                    detail = Ok(after);
                    if !moved {
                        break;
                    }
                    diff = github::pr_diff(&repo, number);
                }
                let detail = detail.map(|d| {
                    let base = github::merge_base(&repo, &d.base_ref_oid, &d.head_ref_oid).ok();
                    (d, base)
                });
                (detail, diff.map(|text| diff::parse(&text)))
            }
        });
        let comments = cx.background_executor().spawn({
            let repo = repo.clone();
            async move { github::review_comments(&repo, number) }
        });
        cx.spawn({
            let key = key.clone();
            async move |this, cx| {
                let comments = comments.await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(pv) = this.pr.as_mut().filter(|pv| pv.key == key) {
                        pv.review_comments = match comments {
                            Ok(c) => Load::Ready(c),
                            Err(e) => Load::Failed(format!("{e:#}")),
                        };
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let (detail, files) = detail.await;
            let _ = this.update(cx, |this, cx| {
                let viewed_paths = this.store.progress(&key.store_key()).viewed.clone();
                let Some(pv) = this.pr.as_mut().filter(|pv| pv.key == key) else { return };
                let mut merge_base = None;
                if let Ok((d, base)) = detail {
                    pv.summary.title = d.title.clone();
                    pv.summary.author = d.author.clone();
                    pv.summary.url = d.url.clone();
                    merge_base = base.or_else(|| Some(d.base_ref_oid.clone()));
                    pv.diff_sha = Some(d.head_ref_oid.clone());
                    pv.detail = Some(d);
                }
                match files {
                    Ok(mut files) => {
                        // Same order as the sidebar tree.
                        files.sort_by(|a, b| crate::tree::tree_order(&a.path, &b.path));
                        pv.viewed = files
                            .iter()
                            .enumerate()
                            .filter(|(_, f)| viewed_paths.contains(&f.path))
                            .map(|(i, _)| i)
                            .collect();
                        pv.collapsed = files
                            .iter()
                            .enumerate()
                            .filter(|(i, f)| {
                                pv.viewed.contains(i) || f.is_probably_generated() || f.line_count() > AUTO_COLLAPSE_LINES
                            })
                            .map(|(i, _)| i)
                            .collect();
                        let jobs: Vec<_> = files.iter().cloned().enumerate().collect();
                        pv.set_files(files);
                        let revs = Revisions {
                            repo: key.repo.clone(),
                            base: merge_base,
                            head: pv.detail.as_ref().map(|d| d.head_ref_oid.clone()),
                        };
                        this.start_analysis(key.clone(), revs, jobs, cx);
                    }
                    Err(e) => pv.files = Load::Failed(format!("{e:#}")),
                }
                if let Some(pv) = &mut this.pr {
                    pv.rebuild_rows();
                    if let Some(step) = pv.plan.as_ref().and_then(|p| p.next_step()) {
                        pv.current_step = Some(step);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_analysis(&mut self, key: PrKey, revs: Revisions, jobs: Vec<(usize, diff::FileDiff)>, cx: &mut Context<Self>) {
        let (tx, rx) = async_channel::unbounded::<(usize, FileAnalysis)>();
        if let Some(pv) = &mut self.pr {
            pv.analysis_running = true;
        }
        analysis::spawn(revs, jobs, tx);
        cx.spawn(async move |this, cx| {
            while let Ok((fi, result)) = rx.recv().await {
                let alive = this.update(cx, |this, cx| {
                    if let Some(pv) = this.pr.as_mut().filter(|pv| pv.key == key) {
                        // The fetched head version doubles as the source for revealing context.
                        if let Some(source) = &result.new.source {
                            if !matches!(pv.sources.get(&fi), Some(Load::Ready(_))) {
                                pv.sources.insert(fi, Load::Ready(Arc::new(source.lines().map(String::from).collect())));
                            }
                        }
                        pv.analysis.insert(fi, result);
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                if let Some(pv) = this.pr.as_mut().filter(|pv| pv.key == key) {
                    pv.analysis_running = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        match &mut self.pr {
            Some(pv) => {
                pv.files = Load::Loading;
                pv.rows.clear();
                pv.selection = None;
                pv.tokens = None;
                pv.analysis.clear();
                let key = pv.key.clone();
                self.fetch_pr(key, cx);
            }
            None => {
                self.open_prs.clear();
                self.load_prs(self.filter, cx)
            }
        }
        cx.notify();
    }

    /// Switches between the Conversation page and review mode (the diff).
    pub fn set_page(&mut self, page: PrPage, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        if page != PrPage::Conversation {
            window.focus(&self.diff_focus);
        }
        if page == PrPage::Story {
            // Resume the story where the reviewer was (or show its intro).
            let commit = pv.story_focus.as_ref().map(|f| f.current).or_else(|| pv.story.as_ref()?.next_commit()).or(Some(0));
            if pv.story.is_some() {
                return self.go_to_commit(commit.unwrap_or(0), cx);
            }
            pv.page = page;
            pv.left_tab = LeftTab::Story;
            return cx.notify();
        }
        let leaving_story = pv.page == PrPage::Story;
        pv.page = page;
        if leaving_story {
            pv.rebuild_rows();
        }
        if pv.left_tab == LeftTab::Story {
            pv.left_tab = LeftTab::Files;
        }
        cx.notify();
    }

    pub fn toggle_review_mode(&mut self, _: &ToggleReviewMode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.pr.as_ref().map(|pv| pv.page) else { return };
        let next = if page == PrPage::Files { PrPage::Conversation } else { PrPage::Files };
        self.set_page(next, window, cx);
    }

    /// Opens review mode at an inline comment's line.
    pub fn jump_to_comment(&mut self, path: &str, line: Option<u32>, left_side: bool, cx: &mut Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        let Some(fi) = pv.file_index(path) else {
            pv.page = PrPage::Files;
            return cx.notify();
        };
        match line {
            Some(n) => {
                pv.jump_to_lines(fi, !left_side, n..n + 1);
            }
            None => pv.jump_to_file(fi),
        }
        cx.notify();
    }

    pub fn back_to_list(&mut self, _: &BackToList, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_chat(window, cx);
        self.pr = None;
        self.browser = None;
        self.browser_cache = None;
        self.menu = None;
        self.hover = None;
        window.focus(&self.root_focus);
        cx.notify();
    }

    // ---- navigation & selection

    pub fn next_file(&mut self, _: &NextFile, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(pv) = &mut self.pr {
            let next = (pv.current_file + 1).min(pv.files().len().saturating_sub(1));
            pv.jump_to_file(next);
            cx.notify();
        }
    }

    pub fn prev_file(&mut self, _: &PrevFile, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(pv) = &mut self.pr {
            pv.jump_to_file(pv.current_file.saturating_sub(1));
            cx.notify();
        }
    }

    pub fn toggle_viewed(&mut self, _: &ToggleViewed, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(pv) = &self.pr {
            self.set_viewed(pv.current_file, true, cx);
        }
    }

    /// Toggles "viewed" on a file. Marking a file viewed collapses it and, when
    /// `advance`, moves on to the next unviewed file.
    pub fn set_viewed(&mut self, fi: usize, advance: bool, cx: &mut Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        if pv.viewed.remove(&fi) {
            pv.collapsed.remove(&fi);
        } else {
            pv.viewed.insert(fi);
            pv.collapsed.insert(fi);
        }
        pv.rebuild_rows();
        if advance && pv.viewed.contains(&fi) {
            let count = pv.files().len();
            let next = (1..count).map(|d| (fi + d) % count).find(|f| !pv.viewed.contains(f));
            pv.jump_to_file(next.unwrap_or(fi));
        } else {
            pv.jump_to_file(fi);
        }
        let paths = pv.viewed.iter().map(|&i| pv.files()[i].path.clone()).collect();
        let key = pv.key.store_key();
        self.store.progress(&key).viewed = paths;
        self.store.save();
        cx.notify();
    }

    /// Reveals hidden unchanged lines around hunks. Fetches the file's head version first
    /// when it isn't loaded yet.
    pub fn expand_gap(&mut self, fi: usize, k: usize, dir: ExpandDir, cx: &mut Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        pv.expand(fi, k, dir);
        pv.rebuild_rows();
        self.ensure_source(fi, cx);
        cx.notify();
    }

    /// Shows a file in full: every gap expanded.
    pub fn expand_file(&mut self, fi: usize, cx: &mut Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        if !pv.can_expand(fi) {
            return;
        }
        pv.collapsed.remove(&fi);
        for k in 0..=pv.files()[fi].hunks.len() {
            pv.expanded.insert((fi, k), (u32::MAX / 4, 0));
            pv.clamp_expansion(fi, k);
        }
        pv.rebuild_rows();
        pv.scroll_to_file(fi);
        self.ensure_source(fi, cx);
        cx.notify();
    }

    /// Whether every gap of the file is fully revealed.
    pub fn file_fully_expanded(&self, fi: usize) -> bool {
        let Some(pv) = &self.pr else { return false };
        (0..=pv.files()[fi].hunks.len()).all(|k| pv.gap_hidden(fi, k) == Some(0))
    }

    pub fn collapse_context(&mut self, fi: usize, cx: &mut Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        pv.expanded.retain(|(f, _), _| *f != fi);
        pv.rebuild_rows();
        pv.scroll_to_file(fi);
        cx.notify();
    }

    fn ensure_source(&mut self, fi: usize, cx: &mut Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        if pv.sources.contains_key(&fi) {
            return;
        }
        let Some(sha) = pv.diff_sha() else { return };
        pv.sources.insert(fi, Load::Loading);
        let (key, repo, path) = (pv.key.clone(), pv.key.repo.clone(), pv.files()[fi].path.clone());
        let task = cx.background_executor().spawn(async move { github::file_content(&repo, &sha, &path) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(pv) = this.pr.as_mut().filter(|pv| pv.key == key) else { return };
                pv.sources.insert(
                    fi,
                    match result {
                        Ok(source) => Load::Ready(Arc::new(source.lines().map(String::from).collect())),
                        Err(e) => Load::Failed(format!("{e:#}")),
                    },
                );
                for k in 0..=pv.files()[fi].hunks.len() {
                    pv.clamp_expansion(fi, k);
                }
                pv.rebuild_rows();
                cx.notify();
            });
        })
        .detach();
    }

    /// Marks several files (a folder) viewed or not viewed.
    pub fn set_files_viewed(&mut self, files: &[usize], viewed: bool, cx: &mut Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        for &fi in files {
            if viewed {
                pv.viewed.insert(fi);
                pv.collapsed.insert(fi);
            } else if pv.viewed.remove(&fi) {
                pv.collapsed.remove(&fi);
            }
        }
        pv.rebuild_rows();
        let paths = pv.viewed.iter().map(|&i| pv.files()[i].path.clone()).collect();
        let key = pv.key.store_key();
        self.store.progress(&key).viewed = paths;
        self.store.save();
        cx.notify();
    }

    pub fn toggle_dir(&mut self, path: String, cx: &mut Context<Self>) {
        if let Some(pv) = &mut self.pr {
            if !pv.collapsed_dirs.remove(&path) {
                pv.collapsed_dirs.insert(path);
            }
            cx.notify();
        }
    }

    pub fn toggle_collapsed(&mut self, fi: usize, cx: &mut Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        if !pv.collapsed.remove(&fi) {
            pv.collapsed.insert(fi);
        }
        pv.rebuild_rows();
        pv.jump_to_file(fi);
        cx.notify();
    }

    pub fn set_all_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        pv.collapsed = if collapsed { (0..pv.files().len()).collect() } else { HashSet::new() };
        pv.rebuild_rows();
        pv.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    // ---- selection (whole lines from the gutter, tokens in the code)

    /// The code view the keyboard acts on: the file view when open, else the diff.
    pub fn active_view(&self) -> CodeView {
        if self.browser.is_some() { CodeView::Source } else { CodeView::Diff }
    }

    pub fn view_text(&self, view: CodeView, row: usize, side: usize) -> Option<String> {
        match view {
            CodeView::Diff => self.pr.as_ref()?.row_text(row, side),
            CodeView::Source => self.browser.as_ref()?.row_text(row, side),
        }
    }

    fn view_rows(&self, view: CodeView) -> usize {
        match view {
            CodeView::Diff => self.pr.as_ref().map_or(0, |pv| pv.rows.len()),
            CodeView::Source => self.browser.as_ref().map_or(0, |b| b.line_count()),
        }
    }

    pub fn view_token_sel(&self, view: CodeView) -> Option<TokenSel> {
        match view {
            CodeView::Diff => self.pr.as_ref()?.token_sel(),
            CodeView::Source => self.browser.as_ref()?.token_sel(),
        }
    }

    fn view_selection(&self, view: CodeView) -> Option<(usize, usize)> {
        match view {
            CodeView::Diff => self.pr.as_ref()?.selection,
            CodeView::Source => self.browser.as_ref()?.selection,
        }
    }

    fn set_view_tokens(&mut self, view: CodeView, sel: TokenSel) {
        match view {
            CodeView::Diff => {
                if let Some(pv) = &mut self.pr {
                    pv.set_token_sel(sel)
                }
            }
            CodeView::Source => {
                if let Some(b) = &mut self.browser {
                    b.set_token_sel(sel)
                }
            }
        }
    }

    fn view_scroll(&self, view: CodeView) -> Option<UniformListScrollHandle> {
        match view {
            CodeView::Diff => Some(self.pr.as_ref()?.scroll.clone()),
            CodeView::Source => Some(self.browser.as_ref()?.scroll.clone()),
        }
    }

    /// Whether a selection may span rows `a` and `b` (in the diff: only within one file).
    fn same_group(&self, view: CodeView, a: usize, b: usize) -> bool {
        match (view, &self.pr) {
            (CodeView::Diff, Some(pv)) => pv.rows.get(a).map(|r| r.file()) == pv.rows.get(b).map(|r| r.file()),
            _ => true,
        }
    }

    /// Selects whole rows (clicks on the gutter, or rows without code).
    fn select_row(&mut self, view: CodeView, ix: usize, extend: bool) {
        let anchor = self.view_selection(view).map(|(a, _)| a).filter(|&a| extend && self.same_group(view, a, ix));
        let selection = Some((anchor.unwrap_or(ix), ix));
        match view {
            CodeView::Diff => {
                let Some(pv) = &mut self.pr else { return };
                let Some(row) = pv.rows.get(ix).copied() else { return };
                pv.current_file = row.file();
                pv.selection = selection;
                pv.tokens = None;
            }
            CodeView::Source => {
                let Some(b) = &mut self.browser else { return };
                b.selection = selection;
                b.tokens = None;
            }
        }
    }

    /// The token under `position` on row `ix` (optionally only on one side).
    fn caret_at(&self, view: CodeView, ix: usize, hits: &[CodeHit], position: Point<Pixels>, side: Option<usize>) -> Option<Caret> {
        let (side, i) = text_index_at(hits, position, side)?;
        let text = self.view_text(view, ix, side)?;
        Some(Caret::new(ix, side, crate::selection::token_at(&text, i)?))
    }

    /// Mouse down on a code row: selects the token under the mouse (⇧ extends, a double
    /// click takes the whole line), or the whole row from the gutter.
    pub fn on_code_mouse_down(
        &mut self,
        view: CodeView,
        ix: usize,
        ev: &MouseDownEvent,
        hits: &[CodeHit],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.diff_focus);
        self.menu = None;
        self.dragging = true;
        match self.caret_at(view, ix, hits, ev.position, None) {
            Some(caret) if ev.click_count >= 2 => {
                let tokens = self.view_text(view, ix, caret.side).map(|t| crate::selection::tokens(&t)).unwrap_or_default();
                if let (Some(first), Some(last)) = (tokens.first(), tokens.last()) {
                    let sel = TokenSel { anchor: Caret::new(ix, caret.side, first.clone()), head: Caret::new(ix, caret.side, last.clone()) };
                    self.set_view_tokens(view, sel);
                }
            }
            Some(caret) => {
                let sel = match self.view_token_sel(view) {
                    Some(s) if ev.modifiers.shift && s.side() == caret.side && self.same_group(view, s.anchor.row, ix) => {
                        TokenSel { anchor: s.anchor, head: caret }
                    }
                    _ => TokenSel::single(caret),
                };
                self.set_view_tokens(view, sel);
            }
            None => self.select_row(view, ix, ev.modifiers.shift),
        }
        cx.notify();
    }

    /// Dragging extends the selection token by token (or row by row from the gutter).
    pub fn on_code_mouse_move(&mut self, view: CodeView, ix: usize, ev: &MouseMoveEvent, hits: &[CodeHit], cx: &mut Context<Self>) {
        if !self.dragging {
            return;
        }
        if ev.pressed_button != Some(MouseButton::Left) {
            self.dragging = false;
            return;
        }
        match self.view_token_sel(view) {
            Some(sel) => {
                let Some(head) = self.caret_at(view, ix, hits, ev.position, Some(sel.side())) else { return };
                if head != sel.head && self.same_group(view, sel.anchor.row, ix) {
                    self.set_view_tokens(view, TokenSel { anchor: sel.anchor, head });
                    cx.notify();
                }
            }
            None => {
                if self.view_selection(view).is_some_and(|(_, head)| head != ix) {
                    self.select_row(view, ix, true);
                    cx.notify();
                }
            }
        }
    }

    /// Arrow keys: move to the next token (⇧ extends the selection).
    pub fn move_tokens(&mut self, dir: Dir, extend: bool, cx: &mut Context<Self>) {
        let view = self.active_view();
        if view == CodeView::Diff && self.pr.as_ref().is_none_or(|pv| pv.page == PrPage::Conversation) {
            return;
        }
        let rows = self.view_rows(view);
        let Some(scroll) = self.view_scroll(view) else { return };
        let Some(sel) = self.view_token_sel(view) else {
            // No token selected yet: start at the selected line, or the top of the view.
            let from = self.view_selection(view).map_or_else(|| scroll.0.borrow().base_handle.logical_scroll_top().0, |(_, head)| head);
            let first = (from..rows).find_map(|r| {
                let text = self.view_text(view, r, 0)?;
                Some(Caret::new(r, 0, crate::selection::tokens(&text).first()?.clone()))
            });
            if let Some(caret) = first {
                self.set_view_tokens(view, TokenSel::single(caret));
                scroll.scroll_to_item(caret.row, ScrollStrategy::Center);
                cx.notify();
            }
            return;
        };
        let Some(next) = crate::selection::step(sel.head, dir, rows, |r, s| self.view_text(view, r, s)) else { return };
        if extend && !self.same_group(view, sel.anchor.row, next.row) {
            return;
        }
        self.set_view_tokens(view, if extend { TokenSel { anchor: sel.anchor, head: next } } else { TokenSel::single(next) });
        scroll.scroll_to_item(next.row, if next.row < sel.head.row { ScrollStrategy::Top } else { ScrollStrategy::Bottom });
        cx.notify();
    }

    /// The selected text, when tokens are selected.
    pub fn selected_text(&self, view: CodeView) -> Option<String> {
        let sel = self.view_token_sel(view)?;
        Some(sel.text(|r, s| self.view_text(view, r, s))).filter(|t| !t.is_empty())
    }

    /// Context for Claude about the selection in a view: the selected lines, and the
    /// selected text in the location when tokens are selected.
    pub fn selection_context(&self, view: CodeView) -> Option<ChangeContext> {
        let mut context = match view {
            CodeView::Diff => {
                let pv = self.pr.as_ref()?;
                pv.rows_context(pv.selected_range()?)?
            }
            CodeView::Source => self.source_context()?,
        };
        if let Some(text) = self.selected_text(view).filter(|t| !t.contains('\n') && t.chars().count() <= 60) {
            context.location = format!("{} · `{text}`", context.location);
        }
        Some(context)
    }

    pub fn on_row_right_click(
        &mut self,
        view: CodeView,
        ix: usize,
        ev: &MouseDownEvent,
        hits: &[CodeHit],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.diff_focus);
        self.hover = None;
        self.hover_pending = None;
        self.hover_task = None;
        // Like an editor: right-clicking outside the selection selects what's under the mouse.
        let inside = self.view_token_sel(view).is_some_and(|s| {
            text_index_at(hits, ev.position, Some(s.side())).is_some_and(|(side, i)| s.contains(ix, side, i))
        }) || (self.view_token_sel(view).is_none()
            && self.view_selection(view).is_some_and(|(a, b)| (a.min(b)..=a.max(b)).contains(&ix)));
        if !inside {
            match self.caret_at(view, ix, hits, ev.position, None) {
                Some(caret) => self.set_view_tokens(view, TokenSel::single(caret)),
                None => self.select_row(view, ix, false),
            }
        }
        let symbol = symbol_at(hits, ev.position)
            .map(|(name, _)| name)
            .filter(|s| s.len() > 1 && !s.chars().all(|c| c.is_ascii_digit()));
        let selected_text = self.selected_text(view);
        let menu = match view {
            CodeView::Source => self.source_context().map(|context| Menu {
                position: ev.position,
                context,
                view,
                row: None,
                symbol,
                selected_text,
                open_at: None,
            }),
            CodeView::Diff => {
                let Some(pv) = &mut self.pr else { return };
                let Some(row) = pv.rows.get(ix).copied() else { return };
                let context = match row {
                    Row::File(fi) | Row::Note(fi) => Some(pv.file_context(fi)),
                    Row::Hunk(..) => {
                        let rows = pv.hunk_rows(ix);
                        pv.selection = Some((rows.start, rows.end - 1));
                        pv.tokens = None;
                        pv.rows_context(rows)
                    }
                    Row::Expand(..) | Row::Elsewhere(..) | Row::Comments(..) => None,
                    Row::Line(..) | Row::Split(..) | Row::Context(..) => self.selection_context(view),
                };
                let open_at = self.diff_head_line(ix);
                let commentable = matches!(row, Row::Line(..) | Row::Split(..)).then_some(ix);
                context.map(|context| Menu {
                    position: ev.position,
                    context,
                    view,
                    row: commentable,
                    symbol,
                    selected_text: selected_text.filter(|_| matches!(row, Row::Line(..) | Row::Split(..) | Row::Context(..))),
                    open_at,
                })
            }
        };
        self.menu = menu;
        cx.notify();
    }

    /// Where a diff row is in the head version: (path, line). The file's first line for its
    /// header, the next head line for removed lines; `None` for deleted files.
    pub fn diff_head_line(&self, ix: usize) -> Option<(String, u32)> {
        let pv = self.pr.as_ref()?;
        let row = *pv.rows.get(ix)?;
        let file = &pv.files()[row.file()];
        if file.status == crate::diff::FileStatus::Deleted || file.binary {
            return None;
        }
        let line = match row {
            Row::Line(f, h, l) | Row::Split(f, h, Some(l), None) => {
                let lines = &pv.files()[f].hunks[h].lines;
                lines[l..].iter().find_map(|l| l.new_no).or_else(|| lines[..l].iter().rev().find_map(|l| l.new_no))
            }
            Row::Split(f, h, _, Some(r)) => pv.files()[f].hunks[h].lines[r].new_no,
            Row::Hunk(f, h) => Some(pv.files()[f].hunks[h].before().1 + 1),
            Row::Context(_, _, n) => Some(n),
            _ => None,
        };
        Some((file.path.clone(), line.unwrap_or(1)))
    }

    pub fn ask_selection(&mut self, _: &AskSelection, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ctx) = self.selection_context(self.active_view()) {
            self.open_chat(Some(ctx), window.mouse_position(), None, window, cx);
        }
    }

    pub fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat.is_some() {
            return self.finish_chat(window, cx);
        }
        if self.hover.take().is_none() && self.menu.take().is_none() {
            if let Some(b) = &mut self.browser {
                // Esc clears the selection, then closes the file view.
                if b.selection.take().is_none() {
                    self.close_browser(window, cx);
                }
            } else if let Some(pv) = &mut self.pr {
                pv.selection = None;
                pv.tokens = None;
            }
        }
        cx.notify();
    }

    /// ⌘C: the selected text (tokens), else the selected lines.
    pub fn copy_selection(&mut self, cx: &mut Context<Self>) {
        // Text selected with the mouse (chat, review log, comments) wins.
        if crate::ui::select::copy(cx) {
            return;
        }
        let view = self.active_view();
        let text = self.selected_text(view).or_else(|| match view {
            CodeView::Diff => {
                let pv = self.pr.as_ref()?;
                Some(pv.rows_context(pv.selected_range()?)?.snippet)
            }
            CodeView::Source => {
                let b = self.browser.as_ref()?;
                let file = b.current()?;
                let range = b.selected_range()?;
                Some(file.lines.get(range.start..range.end.min(file.lines.len()))?.join("\n"))
            }
        });
        if let Some(text) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    // ---- file view

    /// Opens the full-file view (at `path` and line, when given). It shows the PR's head.
    pub fn open_browser(&mut self, at: Option<(String, u32)>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pv) = &self.pr else { return };
        let Some(sha) = pv.detail.as_ref().map(|d| d.head_ref_oid.clone()) else { return };
        if self.browser.is_none() {
            let cached = self.browser_cache.take().filter(|b| b.key == pv.key && b.sha == sha);
            let fresh = cached.is_none();
            let changed = pv.files().iter().map(|f| (f.path.clone(), f.status)).collect();
            self.browser = Some(cached.unwrap_or_else(|| FileBrowser::new(pv.key.clone(), sha, changed)));
            if fresh {
                self.load_tree(cx);
            }
        }
        self.menu = None;
        self.hover = None;
        window.focus(&self.diff_focus);
        let at = at.or_else(|| {
            // Default to the current file of the diff.
            let b = self.browser.as_ref()?;
            if b.path.is_some() {
                return None;
            }
            let pv = self.pr.as_ref()?;
            let file = pv.files().get(pv.current_file)?;
            (file.status != crate::diff::FileStatus::Deleted).then(|| (file.path.clone(), 1))
        });
        if let Some((path, line)) = at {
            self.browser_open_file(path, Some(line), cx);
        }
        cx.notify();
    }

    pub fn close_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.browser_cache = self.browser.take();
        self.menu = None;
        self.hover = None;
        window.focus(&self.diff_focus);
        cx.notify();
    }

    fn load_tree(&mut self, cx: &mut Context<Self>) {
        let Some(b) = &self.browser else { return };
        let (key, repo, sha) = (b.key.clone(), b.key.repo.clone(), b.sha.clone());
        let task = cx.background_executor().spawn(async move { github::repo_tree(&repo, &sha) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(b) = this.browser.as_mut().filter(|b| b.key == key) else { return };
                match result {
                    Ok((paths, truncated)) => {
                        b.set_tree(paths, truncated);
                        if let Some(path) = b.path.clone() {
                            b.reveal(&path);
                        }
                    }
                    Err(e) => b.tree = Load::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows a file in the file view, loading it first if needed.
    pub fn browser_open_file(&mut self, path: String, line: Option<u32>, cx: &mut Context<Self>) {
        let Some(pv) = &self.pr else { return };
        let Some(b) = &mut self.browser else { return };
        b.path = Some(path.clone());
        b.selection = None;
        b.tokens = None;
        b.pending_line = Some(line.unwrap_or(1));
        if matches!(b.tree, Load::Ready(_)) {
            b.reveal(&path);
        }
        if b.files.contains_key(&path) {
            b.apply_pending_line();
            return cx.notify();
        }
        b.files.insert(path.clone(), Load::Loading);
        // Files of the PR: the analysis already fetched their head version.
        let fi = pv.files().iter().position(|f| f.path == path);
        let known = fi.and_then(|fi| pv.analysis.get(&fi)?.new.source.clone());
        let diff = fi.map(|fi| pv.files()[fi].clone());
        let (key, repo, sha) = (b.key.clone(), b.key.repo.clone(), b.sha.clone());
        let task = cx.background_executor().spawn({
            let path = path.clone();
            async move {
                let source = match known {
                    Some(source) => source.to_string(),
                    None => github::file_content(&repo, &sha, &path)?,
                };
                anyhow::Ok(SourceFile::new(&source, &path, diff.as_ref()))
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(b) = this.browser.as_mut().filter(|b| b.key == key) else { return };
                b.files.insert(path, result.map_or_else(|e| Load::Failed(format!("{e:#}")), |f| Load::Ready(Arc::new(f))));
                b.apply_pending_line();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn browser_toggle_dir(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Some(b) = &mut self.browser {
            b.toggle_dir(path);
            cx.notify();
        }
    }

    /// Context for Claude about the selected lines of the file view.
    fn source_context(&self) -> Option<ChangeContext> {
        let pv = self.pr.as_ref()?;
        let b = self.browser.as_ref()?;
        let file = b.current()?;
        let range = b.selected_range()?;
        let lines = file.lines.get(range.start..range.end.min(file.lines.len()))?;
        let location = if range.len() == 1 {
            format!("L{} (head)", range.start + 1)
        } else {
            format!("L{}–L{} (head)", range.start + 1, range.end)
        };
        Some(ChangeContext {
            repo: pv.key.repo.clone(),
            pr_number: pv.key.number,
            pr_title: pv.summary.title.clone(),
            file: b.path.clone()?,
            location,
            snippet: lines.iter().map(|l| format!(" {l}")).collect::<Vec<_>>().join("\n"),
        })
    }

    /// Jumps to a symbol by name in a file (plan steps, palette).
    pub fn jump_to_symbol(&mut self, fi: usize, name: &str) -> bool {
        let Some(pv) = &mut self.pr else { return false };
        let found = pv.analysis.get(&fi).and_then(|a| {
            let matches = |s: &prmator_syntax::Symbol| {
                s.name == name || s.container.as_deref().is_some_and(|c| format!("{c}::{}", s.name) == name)
            };
            a.new.symbols.iter().find(|s| matches(s)).map(|s| (true, s.start_line..s.end_line + 1)).or_else(|| {
                a.old.symbols.iter().find(|s| matches(s)).map(|s| (false, s.start_line..s.end_line + 1))
            })
        });
        if let Some((new_side, lines)) = found {
            if pv.jump_to_lines(fi, new_side, lines) {
                return true;
            }
        }
        // Not parsed (or outside the diff): first diff line mentioning it.
        let files = pv.files();
        let hit = files[fi].hunks.iter().flat_map(|h| &h.lines).find(|l| l.text.contains(name));
        if let Some(l) = hit {
            let (new_side, n) = match l.new_no {
                Some(n) => (true, n),
                None => (false, l.old_no.unwrap_or(1)),
            };
            return pv.jump_to_lines(fi, new_side, n..n + 1);
        }
        pv.jump_to_file(fi);
        false
    }

    // ---- review plan

    pub fn plan_review(&mut self, cx: &mut Context<Self>) {
        let id = self.next_request_id();
        let Some(pv) = &mut self.pr else { return };
        if !matches!(pv.files, Load::Ready(_)) || pv.plan_run.is_some() {
            return;
        }
        if !self.connected {
            pv.plan_error = Some("Claude bridge is not connected yet; try again in a moment.".into());
            cx.notify();
            return;
        }
        self.link.send(Msg::Plan {
            id,
            repo: pv.key.repo.clone(),
            pr_number: pv.key.number,
            pr_title: pv.summary.title.clone(),
            outline: pv.outline(),
        });
        pv.plan_run = Some(PlanRun { id, activity: vec![] });
        pv.plan_error = None;
        pv.left_tab = LeftTab::Plan;
        cx.notify();
    }

    pub fn go_to_step(&mut self, step: usize, cx: &mut Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        let Some(s) = pv.plan.as_ref().and_then(|p| p.plan.steps.get(step)).cloned() else { return };
        pv.current_step = Some(step);
        pv.left_tab = LeftTab::Plan;
        if let Some(fi) = pv.file_index(&s.file) {
            match s.symbol.as_deref().filter(|s| !s.is_empty()) {
                Some(symbol) => {
                    self.jump_to_symbol(fi, symbol);
                }
                None => {
                    if let Some(pv) = &mut self.pr {
                        if pv.collapsed.remove(&fi) {
                            pv.rebuild_rows();
                        }
                        pv.jump_to_file(fi);
                    }
                }
            }
        }
        cx.notify();
    }

    fn save_plan(&mut self) {
        let Some(pv) = &self.pr else { return };
        let key = pv.key.store_key();
        self.store.progress(&key).plan = pv.plan.clone();
        self.store.save();
    }

    pub fn set_step_done(&mut self, step: usize, done: bool, cx: &mut Context<Self>) {
        let Some(plan) = self.pr.as_mut().and_then(|pv| pv.plan.as_mut()) else { return };
        if done {
            plan.done.insert(step);
        } else {
            plan.done.remove(&step);
        }
        self.save_plan();
        cx.notify();
    }

    pub fn next_step(&mut self, _: &NextStep, _: &mut Window, cx: &mut Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        let Some(plan) = &mut pv.plan else { return self.plan_review(cx) };
        if let Some(cur) = pv.current_step {
            plan.done.insert(cur);
        }
        let next = plan.next_step();
        self.save_plan();
        match next {
            Some(step) => self.go_to_step(step, cx),
            None => cx.notify(),
        }
    }

    pub fn prev_step(&mut self, _: &PrevStep, _: &mut Window, cx: &mut Context<Self>) {
        let Some(pv) = &self.pr else { return };
        if pv.plan.is_some() {
            let step = pv.current_step.unwrap_or(0).saturating_sub(1);
            self.go_to_step(step, cx);
        }
    }

    pub fn continue_review(&mut self, _: &ContinueReview, _: &mut Window, cx: &mut Context<Self>) {
        let Some(pv) = &self.pr else { return };
        match &pv.plan {
            Some(plan) => {
                if let Some(step) = plan.next_step() {
                    self.go_to_step(step, cx);
                }
            }
            None => self.plan_review(cx),
        }
    }

    // ---- hover cards

    /// Called when the mouse moves over code text. `ix` is the byte index in `text`.
    /// Called on every mouse move over a diff row with the identifier under the mouse.
    pub fn on_code_hover(&mut self, symbol: Option<(String, Option<Highlight>)>, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(name) = symbol.filter(|(n, h)| worth_looking_up(n, *h)).map(|(n, _)| n) else {
            self.hover_pending = None;
            self.schedule_hover_hide(cx);
            return;
        };
        if self.hover.as_ref().is_some_and(|h| h.name == name) {
            self.hover_task = None; // keep it open
            return;
        }
        if self.hover_pending.as_ref() == Some(&name) {
            return; // already waiting for this one
        }
        debug_log(format_args!("hover pending {name} at {position:?}"));
        self.hover_pending = Some(name.clone());
        self.hover_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_DELAY).await;
            let _ = this.update(cx, |this, cx| this.show_hover(name, position, cx));
        }));
    }

    pub fn show_hover(&mut self, name: String, position: Point<Pixels>, cx: &mut Context<Self>) {
        debug_log(format_args!("show_hover {name} menu={} palette={}", self.menu.is_some(), self.palette.is_some()));
        self.hover_pending = None;
        if self.menu.is_some() || self.palette.is_some() {
            return;
        }
        let Some(pv) = &self.pr else { return };
        let key = (pv.key.repo.clone(), name.clone());
        if !self.defs.contains_key(&key) {
            self.defs.insert(key.clone(), DefState::Loading);
            let in_browser = self.browser.as_ref().and_then(|b| prmator_syntax::detect(b.path.as_ref()?));
            let lang = in_browser
                .or_else(|| pv.files().get(pv.current_file).and_then(|f| prmator_syntax::detect(&f.path)))
                .or_else(|| pv.analysis.values().next().map(|a| a.lang));
            let Some(lang) = lang else {
                self.defs.insert(key, DefState::NotFound);
                return;
            };
            let known = pv
                .analysis
                .iter()
                .filter_map(|(&fi, a)| {
                    Some(KnownFile {
                        path: pv.files()[fi].path.clone(),
                        lang: a.lang,
                        source: a.new.source.clone()?,
                        symbols: a.new.symbols.clone(),
                    })
                })
                .collect();
            let query = defs::Query {
                name: name.clone(),
                lang,
                repo: pv.key.repo.clone(),
                head_sha: pv.detail.as_ref().map(|d| d.head_ref_oid.clone()),
                known,
            };
            let task = cx.background_executor().spawn(async move { defs::lookup(&query) });
            cx.spawn(async move |this, cx| {
                let def = task.await;
                let _ = this.update(cx, |this, cx| {
                    debug_log(format_args!("lookup {} -> {:?}", key.1, def.as_ref().map(|d| (&d.path, d.start_line))));
                    this.defs.insert(key, def.map_or(DefState::NotFound, |d| DefState::Found(Arc::new(d))));
                    cx.notify();
                });
            })
            .detach();
        }
        self.hover = Some(HoverCard { name, position, over_card: false });
        cx.notify();
    }

    pub fn schedule_hover_hide(&mut self, cx: &mut Context<Self>) {
        if self.hover.is_none() {
            self.hover_task = None;
            return;
        }
        self.hover_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_HIDE_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.hover.as_ref().is_some_and(|h| !h.over_card) {
                    this.hover = None;
                    cx.notify();
                }
            });
        }));
    }

    pub fn on_card_hover(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if let Some(h) = &mut self.hover {
            h.over_card = hovered;
            if hovered {
                self.hover_task = None;
            } else {
                self.schedule_hover_hide(cx);
            }
        }
    }

    pub fn current_definition(&self) -> Option<(&HoverCard, &DefState)> {
        let hover = self.hover.as_ref()?;
        let pv = self.pr.as_ref()?;
        Some((hover, self.defs.get(&(pv.key.repo.clone(), hover.name.clone()))?))
    }

    /// Where a definition from this PR shows up in the diff, if it does.
    pub fn definition_in_diff(&self, def: &Definition) -> Option<usize> {
        let pv = self.pr.as_ref()?;
        pv.file_index(&def.path).filter(|_| def.source == defs::DefSource::ThisPr)
    }

    pub fn jump_to_definition(&mut self, def: Arc<Definition>, window: &mut Window, cx: &mut Context<Self>) {
        self.hover = None;
        if self.browser.is_some() || self.definition_in_diff(&def).is_none() {
            // In the file view, or outside the diff: show it in the file view.
            if self.definition_in_repo(&def) {
                if self.browser.is_some() {
                    self.browser_open_file(def.path.clone(), Some(def.start_line), cx);
                } else {
                    self.open_browser(Some((def.path.clone(), def.start_line)), window, cx);
                }
            }
            return cx.notify();
        }
        if let Some(fi) = self.definition_in_diff(&def) {
            if let Some(pv) = &mut self.pr {
                if !pv.jump_to_lines(fi, true, def.start_line..def.end_line + 1) {
                    self.jump_to_symbol(fi, &def.name);
                }
            }
        }
        cx.notify();
    }

    /// Whether a definition is in this PR's repo (so the file view can show it).
    pub fn definition_in_repo(&self, def: &Definition) -> bool {
        let Some(pv) = &self.pr else { return false };
        def.source == defs::DefSource::ThisPr
            || def.url.as_deref().is_some_and(|u| u.contains(&format!("github.com/{}/", pv.key.repo)))
    }

    pub fn ask_about_definition(&mut self, def: Arc<Definition>, window: &mut Window, cx: &mut Context<Self>) {
        self.hover = None;
        let Some(pv) = &self.pr else { return };
        let context = ChangeContext {
            repo: pv.key.repo.clone(),
            pr_number: pv.key.number,
            pr_title: pv.summary.title.clone(),
            file: def.path.clone(),
            location: format!("definition of {} (L{})", def.name, def.start_line),
            snippet: def.lines.iter().map(|l| format!(" {l}")).collect::<Vec<_>>().join("\n"),
        };
        let question = format!(
            "What is `{}` and what role does it play in this PR? How do the changes use or affect it?",
            def.name
        );
        self.open_chat(Some(context), window.mouse_position(), Some(question), window, cx);
    }

    /// The PR view a plan request belongs to, if it is still open.
    fn plan_view(&mut self, id: u64) -> Option<&mut PrView> {
        self.pr.as_mut().filter(|pv| pv.plan_run.as_ref().is_some_and(|r| r.id == id))
    }

    fn on_link(&mut self, ev: LinkEvent, cx: &mut Context<Self>) {
        match ev {
            LinkEvent::Connected => {
                self.connected = true;
                self.bridge_error = None;
            }
            LinkEvent::Disconnected => {
                self.connected = false;
                self.fail_running_turns("Claude bridge disconnected.");
                if let Some(pv) = &mut self.pr {
                    if pv.plan_run.take().is_some() {
                        pv.plan_error = Some("Claude bridge disconnected.".into());
                    }
                    if pv.story_run.take().is_some() {
                        pv.story_error = Some("Claude bridge disconnected.".into());
                    }
                }
            }
            LinkEvent::Msg(msg) if self.on_chat_msg(&msg) => {}
            LinkEvent::Msg(msg) => match msg {
                Msg::Ready { claude_version } => self.claude_version = Some(claude_version),
                Msg::Activity { id, text } => {
                    if let Some(pv) = self.plan_view(id) {
                        pv.plan_run.as_mut().unwrap().activity.push(text);
                    } else if let Some(pv) = self.story_run_view(id) {
                        pv.story_run.as_mut().unwrap().activity.push(text);
                    }
                }
                Msg::StoryReady { id, story } => self.on_story_ready(id, story, cx),
                Msg::PlanReady { id, plan } => {
                    if let Some(pv) = self.plan_view(id) {
                        let head_sha = pv.detail.as_ref().map(|d| d.head_ref_oid.clone()).unwrap_or_default();
                        pv.plan = Some(SavedPlan { plan, done: Default::default(), head_sha });
                        pv.plan_run = None;
                        self.save_plan();
                        self.go_to_step(0, cx);
                    }
                }
                Msg::Done { id, .. } => {
                    if let Some(pv) = self.plan_view(id) {
                        pv.plan_run = None;
                    }
                    if let Some(pv) = self.story_run_view(id) {
                        pv.story_run = None;
                    }
                }
                Msg::Error { id: Some(id), text } => {
                    if let Some(pv) = self.plan_view(id) {
                        pv.plan_error = Some(text);
                        pv.plan_run = None;
                    } else if let Some(pv) = self.story_run_view(id) {
                        pv.story_error = Some(text);
                        pv.story_run = None;
                    }
                }
                Msg::Error { id: None, text } => self.bridge_error = Some(text),
                _ => {}
            },
        }
        cx.notify();
    }

    pub fn run_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            Command::PlanReview => self.plan_review(cx),
            Command::ContinueReview => self.continue_review(&ContinueReview, window, cx),
            Command::NextStep => self.next_step(&NextStep, window, cx),
            Command::PrevStep => self.prev_step(&PrevStep, window, cx),
            Command::ReviewWithClaude => {
                let at = self.default_chat_position(window);
                self.open_chat(None, at, Some(REVIEW_PROMPT.into()), window, cx)
            }
            Command::UnifiedDiff => self.set_diff_mode(DiffMode::Unified, cx),
            Command::SplitDiff => self.set_diff_mode(DiffMode::Split, cx),
            Command::ThemeAuto => self.set_theme(ThemeMode::System, cx),
            Command::ThemeLight => self.set_theme(ThemeMode::Light, cx),
            Command::ThemeDark => self.set_theme(ThemeMode::Dark, cx),
            Command::OpenSettings => self.open_settings(cx),
            Command::SelectTheme => self.open_palette(crate::ui::palette::PaletteMode::Themes, window, cx),
            Command::MarkViewed => self.toggle_viewed(&ToggleViewed, window, cx),
            Command::NextFile => self.next_file(&NextFile, window, cx),
            Command::PrevFile => self.prev_file(&PrevFile, window, cx),
            Command::CollapseAll => self.set_all_collapsed(true, cx),
            Command::ExpandAll => self.set_all_collapsed(false, cx),
            Command::Refresh => self.refresh(&Refresh, window, cx),
            Command::OpenOnGitHub => {
                if let Some(pv) = &self.pr {
                    github::open_in_browser(&pv.summary.url);
                }
            }
            Command::ReviewAsStory => self.continue_story(cx),
            Command::OpenFileView => self.open_browser(None, window, cx),
            Command::AskAboutPr => {
                let at = self.default_chat_position(window);
                self.open_chat(None, at, None, window, cx)
            }
            Command::BackToList => self.back_to_list(&BackToList, window, cx),
        }
    }

    /// Summary of saved progress for the PR list.
    pub fn progress_label(&self, pr: &PrSummary) -> Option<String> {
        let progress = self.store.prs.get(&crate::store::pr_key(pr.repo(), pr.number))?;
        if let Some(plan) = &progress.plan {
            return Some(format!("step {}/{}", plan.done.len().min(plan.plan.steps.len()), plan.plan.steps.len()));
        }
        (!progress.viewed.is_empty()).then(|| format!("{} viewed", progress.viewed.len()))
    }

}

/// The theme to show for the saved settings and system appearance.
fn resolve_theme(settings: &Settings, appearance: gpui::WindowAppearance, cx: &gpui::App) -> Theme {
    let dark = settings.theme_mode.is_dark(appearance);
    let name = if dark { &settings.dark_theme } else { &settings.light_theme };
    let themes = cx.global::<Themes>();
    themes
        .get(name)
        .or_else(|| themes.get(if dark { crate::themes::DEFAULT_DARK } else { crate::themes::DEFAULT_LIGHT }))
        .map(|e| e.theme.clone())
        .unwrap_or_else(|| if dark { Theme::dark() } else { Theme::light() })
        .with_settings(settings)
}

pub const REVIEW_PROMPT: &str = "Give me a quick review of this whole PR: what it does, the riskiest parts, \
likely bugs, and which files deserve the most careful reading. Use `gh pr view` and `gh pr diff` to read it.";

/// Identifiers that are worth a definition lookup: types, functions, macros and modules
/// rather than locals, keywords and standard library names.
fn worth_looking_up(name: &str, highlight: Option<Highlight>) -> bool {
    const SKIP: &[&str] = &[
        "Self", "Some", "None", "Ok", "Err", "String", "Vec", "Option", "Result", "Box", "Arc", "Rc", "HashMap",
        "HashSet", "BTreeMap", "BTreeSet", "True", "False", "TODO", "self", "super", "crate", "new", "default",
        "clone", "into", "from", "unwrap", "expect", "iter", "map", "collect", "to_string", "as_ref", "len",
        "push", "insert", "get", "is_empty", "is_some", "is_none", "ok", "err", "println", "format", "vec",
        "assert", "assert_eq", "assert_ne", "debug_assert", "write", "writeln", "matches", "Default", "Clone",
        "Debug", "PartialEq", "Eq", "Hash", "Serialize", "Deserialize",
    ];
    if name.len() < 2 || SKIP.contains(&name) || name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }
    match highlight {
        Some(Highlight::Function | Highlight::Type | Highlight::Macro | Highlight::Module) => true,
        Some(
            Highlight::Keyword
            | Highlight::String
            | Highlight::Comment
            | Highlight::Number
            | Highlight::Operator
            | Highlight::Punctuation
            | Highlight::Escape,
        ) => false,
        // Unhighlighted or other: types by convention start with a capital letter.
        _ => {
            name.chars().next().is_some_and(|c| c.is_uppercase())
                && !name.chars().all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit())
        }
    }
}

/// Debug tracing, enabled with `PRMATOR_DEBUG=1`.
pub fn debug_log(args: std::fmt::Arguments) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("PRMATOR_DEBUG").is_some()) {
        eprintln!("[prmator] {args}");
    }
}

#[cfg(test)]
mod tests {
    use super::worth_looking_up;
    use prmator_syntax::Highlight;

    #[test]
    fn hover_targets() {
        // Types and functions get cards…
        assert!(worth_looking_up("BatchOperation", None));
        assert!(worth_looking_up("BatchOperation", Some(Highlight::Type)));
        assert!(worth_looking_up("set_variant_items", Some(Highlight::Function)));
        assert!(worth_looking_up("batch_model", Some(Highlight::Module)));
        // …locals, keywords, std names and constants don't.
        assert!(!worth_looking_up("items", None));
        assert!(!worth_looking_up("items", Some(Highlight::Variable)));
        assert!(!worth_looking_up("Option", Some(Highlight::Type)));
        assert!(!worth_looking_up("unwrap", Some(Highlight::Function)));
        assert!(!worth_looking_up("MAX_LINES", None));
        assert!(!worth_looking_up("Fooo", Some(Highlight::String)));
    }
}
