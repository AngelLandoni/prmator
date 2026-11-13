//! PRmator: review GitHub pull requests fast, with Claude one right-click away.
//!
//! Layout:  toolbar / [PR list | files + plan]  [diff]  [Claude]
//! GitHub data comes from the `gh` CLI; tree-sitter (`prmator-syntax`) powers highlighting,
//! symbols and hover definitions; questions and review plans go over a WebSocket to the
//! background `prmator-claude` bridge, which runs the `claude` CLI.

mod analysis;
mod app;
mod chat;
mod comments;
mod defs;
mod diff;
mod find;
mod github;
mod link;
mod mac;
mod model;
mod selection;
mod browser;
mod settings;
mod stacks;
mod store;
mod story;
mod text_input;
mod theme;
mod themes;
mod tree;
mod ui;

use gpui::{actions, px, size, App, AppContext, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions};

actions!(
    prmator,
    [
        Quit,
        Refresh,
        NextFile,
        PrevFile,
        ToggleViewed,
        AskSelection,
        CopySelection,
        Dismiss,
        BackToList,
        ToggleDiffMode,
        CycleTheme,
        OpenFiles,
        OpenSymbols,
        OpenCommands,
        SearchDiff,
        FindNext,
        FindPrev,
        CloseFind,
        PlanReview,
        NextStep,
        PrevStep,
        ContinueReview,
        PaletteUp,
        PaletteDown,
        PaletteClose,
        FinishChat,
        OpenSettings,
        ToggleReviewMode,
        NextCommit,
        PrevCommit,
        AddComment,
        CloseCommentPopover,
        CloseReviewPanel,
        CommentPostNow,
        CommentBoxLeft,
        CommentBoxRight,
        CommentBoxUp,
        CommentBoxDown,
        CommentBoxWider,
        CommentBoxNarrower,
        CommentBoxTaller,
        CommentBoxShorter,
        CommentBoxGrow,
        CommentBoxShrink,
        CommentBoxReset,
        CommentScrollUp,
        CommentScrollDown,
        TokenLeft,
        TokenRight,
        TokenUp,
        TokenDown,
        ExtendLeft,
        ExtendRight,
        ExtendUp,
        ExtendDown,
        ToggleFileView,
    ]
);

/// `  [✓ approved]` after a PR in `--stacks` output, when the user reviewed it.
fn review_mark(pr: &github::PrSummary) -> String {
    pr.my_review.as_ref().map(|r| format!("  [{}]", r.label())).unwrap_or_default()
}

/// `prmator --analyze owner/repo#123 [Name…]`: runs the code analysis (and definition
/// lookups for the given names) without the UI and prints the results.
fn analyze_cli(pr: &str, names: &[String]) -> anyhow::Result<()> {
    let (repo, number) = github::parse_pr_ref(pr).ok_or_else(|| anyhow::anyhow!("bad PR ref {pr:?}"))?;
    let started = std::time::Instant::now();
    let detail = github::pr_detail(&repo, number)?;
    let base = github::merge_base(&repo, &detail.base_ref_oid, &detail.head_ref_oid).ok();
    let files = diff::parse(&github::pr_diff(&repo, number)?);
    println!("{} files, merge base {:?}", files.len(), base);
    let (tx, rx) = async_channel::unbounded();
    let revs = analysis::Revisions { repo: repo.clone(), base, head: Some(detail.head_ref_oid.clone()) };
    analysis::spawn(revs, files.iter().cloned().enumerate().collect(), tx);
    let mut results: Vec<(usize, analysis::FileAnalysis)> = std::iter::from_fn(|| rx.recv_blocking().ok()).collect();
    results.sort_by_key(|(fi, _)| *fi);
    println!("analyzed {} files in {:?}", results.len(), started.elapsed());
    for (fi, a) in &results {
        let highlighted = a.new.highlights.len() + a.old.highlights.len();
        println!("\n{} [{} | {} lines highlighted | source: {}]", files[*fi].path, a.lang.name(), highlighted, a.new.source.is_some());
        for c in &a.changed {
            println!("  {:<8} {:<6} {} (L{}-{})", c.change.label(), analysis::kind_label(c.symbol.kind), c.display_name(), c.symbol.start_line, c.symbol.end_line);
        }
    }
    let plan_outline: Vec<String> =
        files.iter().enumerate().map(|(fi, f)| analysis::outline_line(f, results.iter().find(|(i, _)| *i == fi).map(|(_, a)| a))).collect();
    println!("\n--- plan outline ---\n{}", plan_outline.join("\n"));
    for name in names {
        let known = results
            .iter()
            .filter_map(|(fi, a)| {
                Some(defs::KnownFile {
                    path: files[*fi].path.clone(),
                    lang: a.lang,
                    source: a.new.source.clone()?,
                    symbols: a.new.symbols.clone(),
                })
            })
            .collect();
        let q = defs::Query { name: name.clone(), lang: prmator_syntax::Lang::Rust, repo: repo.clone(), head_sha: Some(detail.head_ref_oid.clone()), known };
        let t = std::time::Instant::now();
        match defs::lookup(&q) {
            Some(d) => println!(
                "\n--- {name}: {} {}:{} via {} in {:?} ---\n{}",
                analysis::kind_label(d.kind), d.path, d.start_line, d.source.label(), t.elapsed(), d.lines.join("\n")
            ),
            None => println!("\n--- {name}: not found ({:?}) ---", t.elapsed()),
        }
    }
    Ok(())
}

/// Apps opened from Finder or the Dock get a minimal PATH (`/usr/bin:/bin:…`), so `gh` and
/// `claude` wouldn't be found. Outside a terminal, take the login shell's PATH and add the
/// usual tool locations. The Claude bridge inherits it.
fn inherit_login_path() {
    if std::env::var_os("TERM").is_some() {
        return; // started from a terminal: PATH is already right
    }
    let mut dirs: Vec<String> = std::env::var("PATH").unwrap_or_default().split(':').map(String::from).collect();
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let login_path = std::process::Command::new(shell)
        .args(["-l", "-c", "printf %s \"$PATH\""])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    let home = std::env::var("HOME").unwrap_or_default();
    let fallbacks = [
        format!("{home}/.local/bin"),
        format!("{home}/.cargo/bin"),
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
    ];
    for dir in login_path.iter().flat_map(|p| p.split(':')).map(String::from).chain(fallbacks) {
        if !dir.is_empty() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    std::env::set_var("PATH", dirs.join(":"));
}

fn main() {
    inherit_login_path();
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--check-fonts") {
        Application::new().run(|cx: &mut App| {
            theme::load_sf_mono(cx);
            for family in [theme::SF_MONO, "Menlo", "UbuntuMono Nerd Font Mono"] {
                let ts = cx.text_system();
                let listed = ts.all_font_names().iter().any(|n| n == family);
                let resolved = ts.get_font_for_id(ts.resolve_font(&gpui::font(family))).map(|f| f.family);
                println!("{family}: listed={listed} resolves_to={resolved:?}");
            }
            cx.quit();
        });
        return;
    }
    if args.get(1).map(String::as_str) == Some("--overview") {
        let pr = args.get(2).and_then(|a| github::parse_pr_ref(a));
        let Some((repo, number)) = pr else { return eprintln!("usage: prmator --overview owner/repo#123") };
        match (github::pr_detail(&repo, number), github::review_comments(&repo, number)) {
            (Ok(d), Ok(c)) => {
                println!("{} [{}{}] by {} — {} labels, {} comments, {} reviews, {} review requests, {} checks, {} inline comments",
                    d.title, d.state, if d.is_draft { ", draft" } else { "" }, d.author.login, d.labels.len(), d.comments.len(),
                    d.reviews.len(), d.review_requests.len(), d.status_check_rollup.len(), c.len());
                let body = ui::overview::clean_github_markdown(&d.body);
                println!("--- description (cleaned, first 12 lines) ---\n{}", body.lines().take(12).collect::<Vec<_>>().join("\n"));
                if let Some(first) = c.first() {
                    println!("--- first inline comment ---\n{}:{:?} by {:?}: {}", first.path, first.line, first.user.as_ref().map(|u| &u.login), first.body.lines().next().unwrap_or_default());
                }
            }
            (Err(e), _) | (_, Err(e)) => eprintln!("{e:#}"),
        }
        return;
    }
    // Story debugging: `--story-listing PR` prints what Claude gets; `--story-assemble PR`
    // reads Claude's story JSON from stdin and prints the validated virtual commits.
    if let Some(mode @ ("--story-listing" | "--story-assemble")) = args.get(1).map(String::as_str) {
        let Some((repo, number)) = args.get(2).and_then(|a| github::parse_pr_ref(a)) else {
            return eprintln!("usage: prmator {mode} owner/repo#123");
        };
        let mut files = match github::pr_diff(&repo, number) {
            Ok(text) => diff::parse(&text),
            Err(e) => return eprintln!("{e:#}"),
        };
        files.sort_by(|a, b| tree::tree_order(&a.path, &b.path));
        if mode == "--story-listing" {
            print!("{}", story::listing(&files));
            return;
        }
        let mut json = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut json);
        let Ok(raw) = serde_json::from_str::<protocol::Story>(&json) else { return eprintln!("stdin is not a story") };
        let raw_ids: usize = raw.commits.iter().map(|c| c.hunks.len()).sum();
        let saved = story::assemble(raw, &files, String::new());
        let total = story::all_hunks(&files).len();
        let covered: usize = saved.commits.iter().map(|c| c.hunks.len()).sum();
        println!("{}\n", saved.summary);
        for (i, c) in saved.commits.iter().enumerate() {
            let paths: std::collections::BTreeSet<&str> = c.hunks.iter().map(|h| h.path.rsplit('/').next().unwrap_or(&h.path)).collect();
            println!("{:>2}. {}  [{} hunks: {}]\n    why: {}", i + 1, c.title, c.hunks.len(), paths.into_iter().collect::<Vec<_>>().join(", "), c.why);
        }
        let exact = story::resolve(&saved, &files).is_some();
        println!("\nClaude listed {raw_ids} hunk ids; {total} hunks in the PR; {covered} placed; {} auto-placed; exact cover: {exact}", saved.auto_placed);
        return;
    }
    if args.get(1).map(String::as_str) == Some("--selftest-comments") {
        // 1. POST mechanics, against an endpoint with no side effects.
        println!("valid POST   -> {:?}", github::selftest_post(Some("**hi**")).map(|_| "ok"));
        println!("invalid POST -> {:?}", github::selftest_post(None).err().map(|e| format!("{e:#}")));
        // 2. Line mapping on a real diff, printed as a dry-run review request.
        let Some((repo, number)) = args.get(2).and_then(|a| github::parse_pr_ref(a)) else { return };
        let mut files = diff::parse(&github::pr_diff(&repo, number).unwrap_or_default());
        files.sort_by(|a, b| tree::tree_order(&a.path, &b.path));
        let key = model::PrKey { repo: repo.clone(), number };
        let mut pv = model::PrView::new(key, github::PrSummary::stub(repo.clone(), number), store::DiffMode::Unified);
        pv.set_files(files);
        pv.review_comments = model::Load::Ready(vec![]);
        pv.rebuild_rows();
        // The first hunk with both removed and added lines: select its first 4 changed lines.
        let changed: Vec<usize> = (0..pv.rows.len())
            .filter(|&i| matches!(pv.rows[i], model::Row::Line(f, h, l) if pv.files()[f].hunks[h].lines[l].kind != diff::LineKind::Context))
            .collect();
        let Some(&first_removed) = changed.iter().find(|&&i| matches!(pv.rows[i], model::Row::Line(f, h, l) if pv.files()[f].hunks[h].lines[l].kind == diff::LineKind::Removed)) else { return };
        let end = changed.iter().copied().filter(|&i| i > first_removed).nth(2).unwrap_or(first_removed);
        pv.selection = Some((first_removed, end));
        if let Some(comments::CommentTarget::New { path, start, end, label, snippet }) = pv.comment_target(end) {
            println!("\nselection: {label}\n{snippet}");
            let draft = comments::DraftComment { id: 1, path, line: end.0, new_side: end.1, start, body: "Is this right?".into() };
            std::env::set_var("PRMATOR_DRY_RUN", "1");
            let _ = github::submit_review(&repo, number, "HEAD_SHA", "Summary", "COMMENT", &[draft.to_github()]);
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("--stacks") {
        // `prmator --stacks [mine|involved]`: the PR list grouped into stacks, as the UI shows it.
        let filter = match args.get(2).map(String::as_str) {
            Some("mine") => github::Filter::Mine,
            Some("involved") => github::Filter::Involved,
            _ => github::Filter::ReviewRequested,
        };
        let listed = match github::list_prs(filter) {
            Ok(prs) => prs,
            Err(e) => return eprintln!("{e:#}"),
        };
        let repos: std::collections::BTreeSet<&str> = listed.iter().map(|p| p.repo()).collect();
        let open: Vec<_> = repos.into_iter().flat_map(|r| github::open_prs(r).unwrap_or_default()).collect();
        for entry in stacks::group(&listed, &open) {
            match entry {
                stacks::Entry::Single(p) => println!("#{} {}{}", p.number, p.title, review_mark(&p)),
                stacks::Entry::Stack(s) => {
                    println!("stack of {} onto {} ({} steps)", s.rows.len(), s.base, s.steps);
                    for r in &s.rows {
                        let mark = if r.listed { "*" } else { " " };
                        println!("  {mark}{:>2}{:indent$} #{} {}{}", r.step, "", r.pr.number, r.pr.title, review_mark(&r.pr), indent = r.step);
                    }
                }
            }
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("--settings") {
        let mut store = store::Store::load();
        let source = if store.settings.is_some() { "PRmator (customized)" } else { "Zed" };
        let settings = store.effective_settings();
        println!("source: {source}\n{}", serde_json::to_string_pretty(&settings).unwrap_or_default());
        return;
    }
    if args.get(1).map(String::as_str) == Some("--themes") {
        for e in themes::Themes::load().0 {
            println!("{:<34} {:<6} {}", e.name, if e.dark { "dark" } else { "light" }, e.source);
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("--analyze") {
        if let Err(e) = analyze_cli(args.get(2).map(String::as_str).unwrap_or_default(), &args[3.min(args.len())..]) {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
        return;
    }
    let initial = std::env::args().nth(1).and_then(|a| {
        let parsed = github::parse_pr_ref(&a);
        if parsed.is_none() {
            eprintln!("ignoring {a:?}: expected owner/repo#123 or a GitHub PR URL");
        }
        parsed
    });

    Application::new().run(move |cx: &mut App| {
        text_input::bind_keys(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        let diff = Some("DiffPane");
        let source = Some("SourceView");
        let code = Some("DiffPane || SourceView");
        let palette = Some("Palette");
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-c", CopySelection, None),
            KeyBinding::new("cmd-r", Refresh, None),
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-e", ToggleReviewMode, None),
            KeyBinding::new("cmd-[", BackToList, None),
            KeyBinding::new("cmd-p", OpenFiles, None),
            KeyBinding::new("cmd-shift-o", OpenSymbols, None),
            KeyBinding::new("cmd-shift-p", OpenCommands, None),
            KeyBinding::new("cmd-k", OpenCommands, None),
            KeyBinding::new("cmd-f", SearchDiff, None),
            // Find bar: ↵ / ⇧↵ step through matches (beats the input's own Enter: same
            // depth, bound later), ⌘G / ⇧⌘G too, esc closes.
            KeyBinding::new("enter", FindNext, Some("FindBar > TextInput")),
            KeyBinding::new("shift-enter", FindPrev, Some("FindBar > TextInput")),
            KeyBinding::new("escape", CloseFind, Some("FindBar")),
            KeyBinding::new("cmd-g", FindNext, None),
            KeyBinding::new("cmd-shift-g", FindPrev, None),
            KeyBinding::new("j", NextFile, diff),
            KeyBinding::new("n", NextFile, diff),
            KeyBinding::new("k", PrevFile, diff),
            KeyBinding::new("p", PrevFile, diff),
            KeyBinding::new("v", ToggleViewed, diff),
            KeyBinding::new("a", AskSelection, diff),
            KeyBinding::new("cmd-c", CopySelection, diff),
            KeyBinding::new("s", ToggleDiffMode, diff),
            KeyBinding::new("c", ContinueReview, diff),
            KeyBinding::new("]", NextStep, diff),
            KeyBinding::new("[", PrevStep, diff),
            KeyBinding::new(".", NextCommit, diff),
            KeyBinding::new(",", PrevCommit, diff),
            KeyBinding::new("m", AddComment, diff),
            KeyBinding::new("f", ToggleFileView, diff),
            // Token-wise selection, in the diff and the file view.
            KeyBinding::new("left", TokenLeft, code),
            KeyBinding::new("right", TokenRight, code),
            KeyBinding::new("up", TokenUp, code),
            KeyBinding::new("down", TokenDown, code),
            KeyBinding::new("shift-left", ExtendLeft, code),
            KeyBinding::new("shift-right", ExtendRight, code),
            KeyBinding::new("shift-up", ExtendUp, code),
            KeyBinding::new("shift-down", ExtendDown, code),
            KeyBinding::new("f", ToggleFileView, source),
            KeyBinding::new("a", AskSelection, source),
            KeyBinding::new("cmd-c", CopySelection, source),
            KeyBinding::new("escape", Dismiss, source),
            KeyBinding::new("escape", CloseCommentPopover, Some("CommentPopover")),
            KeyBinding::new("cmd-shift-enter", CommentPostNow, Some("CommentPopover")),
            KeyBinding::new("cmd-alt-left", CommentBoxLeft, Some("CommentPopover")),
            KeyBinding::new("cmd-alt-right", CommentBoxRight, Some("CommentPopover")),
            KeyBinding::new("cmd-alt-up", CommentBoxUp, Some("CommentPopover")),
            KeyBinding::new("cmd-alt-down", CommentBoxDown, Some("CommentPopover")),
            KeyBinding::new("cmd-alt-shift-right", CommentBoxWider, Some("CommentPopover")),
            KeyBinding::new("cmd-alt-shift-left", CommentBoxNarrower, Some("CommentPopover")),
            KeyBinding::new("cmd-alt-shift-down", CommentBoxTaller, Some("CommentPopover")),
            KeyBinding::new("cmd-alt-shift-up", CommentBoxShorter, Some("CommentPopover")),
            KeyBinding::new("cmd-=", CommentBoxGrow, Some("CommentPopover")),
            KeyBinding::new("cmd-+", CommentBoxGrow, Some("CommentPopover")),
            KeyBinding::new("cmd--", CommentBoxShrink, Some("CommentPopover")),
            KeyBinding::new("cmd-0", CommentBoxReset, Some("CommentPopover")),
            KeyBinding::new("alt-up", CommentScrollUp, Some("CommentPopover")),
            KeyBinding::new("alt-down", CommentScrollDown, Some("CommentPopover")),
            KeyBinding::new("pageup", CommentScrollUp, Some("CommentPopover")),
            KeyBinding::new("pagedown", CommentScrollDown, Some("CommentPopover")),
            KeyBinding::new("escape", CloseReviewPanel, Some("ReviewPanel")),
            KeyBinding::new("escape", Dismiss, diff),
            KeyBinding::new("up", PaletteUp, palette),
            KeyBinding::new("down", PaletteDown, palette),
            KeyBinding::new("ctrl-p", PaletteUp, palette),
            KeyBinding::new("ctrl-n", PaletteDown, palette),
            KeyBinding::new("escape", PaletteClose, palette),
            KeyBinding::new("escape", FinishChat, Some("Chat")),
            KeyBinding::new("cmd-enter", FinishChat, Some("Chat")),
            // The chat's input is multi-line, which would take ⌘↵ as "send".
            KeyBinding::new("cmd-enter", FinishChat, Some("Chat > TextInput")),
        ]);
        theme::load_sf_mono(cx);
        cx.set_global(themes::Themes::load());
        cx.set_global(theme::Theme::dark());

        let bounds = Bounds::centered(None, size(px(1600.), px(980.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // Content fills the window; the toolbar is the titlebar (see `ui::render_toolbar`).
                titlebar: Some(TitlebarOptions {
                    title: Some("PRmator".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(gpui::point(px(14.), px(15.))),
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| app::Prmator::new(initial.clone(), window, cx)),
        )
        .unwrap();
        // Closing the settings window keeps the app running; closing the last window quits.
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
