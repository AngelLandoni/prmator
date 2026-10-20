//! Story mode: the PR retold as an ordered series of small "virtual commits".
//!
//! Claude only groups and orders the PR's existing hunks; it never sees an editable copy
//! of the code. [`assemble`] then guarantees the result is exactly the PR: every hunk lands
//! in exactly one commit (unknown ids are dropped, duplicates removed, missed hunks placed
//! automatically), so reading all commits is reading the whole diff, no more and no less.

use std::collections::{BTreeSet, HashMap, HashSet};

use protocol::Msg;
use serde::{Deserialize, Serialize};

use crate::app::Prmator;
use crate::diff::FileDiff;
use crate::model::{Load, PlanRun, PrPage};

/// Hunks are listed with at most this many lines…
const HUNK_LINES: usize = 60;
/// …while the whole listing stays under this size (lines per hunk shrink to fit).
const LISTING_BUDGET: usize = 180_000;

/// A hunk identified independently of indices, so a saved story survives reloads.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HunkRef {
    pub path: String,
    pub header: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedCommit {
    pub title: String,
    pub why: String,
    #[serde(default)]
    pub next: String,
    pub hunks: Vec<HunkRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedStory {
    pub summary: String,
    pub commits: Vec<SavedCommit>,
    /// Commits marked as reviewed.
    #[serde(default)]
    pub done: BTreeSet<usize>,
    /// Head commit the story was made for.
    #[serde(default)]
    pub head_sha: String,
    /// Hunks Claude left out that PRmator placed itself.
    #[serde(default)]
    pub auto_placed: usize,
}

impl SavedStory {
    pub fn next_commit(&self) -> Option<usize> {
        (0..self.commits.len()).find(|i| !self.done.contains(i))
    }
}

/// Which commit each hunk belongs to, while reading the story.
#[derive(Clone, Debug)]
pub struct StoryFocus {
    pub current: usize,
    /// (file, hunk) -> commit index.
    pub owner: HashMap<(usize, usize), usize>,
}

pub fn hunk_ref(files: &[FileDiff], fi: usize, hi: usize) -> HunkRef {
    HunkRef { path: files[fi].path.clone(), header: files[fi].hunks[hi].header.clone() }
}

/// Every hunk of the PR, in diff order, as (file, hunk).
pub fn all_hunks(files: &[FileDiff]) -> Vec<(usize, usize)> {
    files.iter().enumerate().flat_map(|(fi, f)| (0..f.hunks.len()).map(move |hi| (fi, hi))).collect()
}

/// The listing sent to Claude: `### h3 — path  @@ … @@` followed by the hunk's lines.
pub fn listing(files: &[FileDiff]) -> String {
    let hunks = all_hunks(files);
    let render = |max_lines: usize| {
        let mut out = String::new();
        for (n, &(fi, hi)) in hunks.iter().enumerate() {
            let file = &files[fi];
            let hunk = &file.hunks[hi];
            out.push_str(&format!("### h{n} — {}  {}\n", file.path, hunk.header));
            for line in hunk.lines.iter().take(max_lines) {
                out.push_str(&line.as_diff());
                out.push('\n');
            }
            if hunk.lines.len() > max_lines {
                out.push_str(&format!("… ({} more lines)\n", hunk.lines.len() - max_lines));
            }
            out.push('\n');
        }
        out
    };
    let mut max_lines = HUNK_LINES;
    loop {
        let text = render(max_lines);
        if text.len() <= LISTING_BUDGET || max_lines <= 4 {
            return text;
        }
        max_lines /= 2;
    }
}

/// Turns Claude's answer into a story that covers every hunk exactly once.
pub fn assemble(story: protocol::Story, files: &[FileDiff], head_sha: String) -> SavedStory {
    let hunks = all_hunks(files);
    let mut taken: HashSet<usize> = HashSet::new();
    let mut commits: Vec<(SavedCommit, Vec<usize>)> = vec![];
    for c in story.commits {
        let mut ids: Vec<usize> = c
            .hunks
            .iter()
            .filter_map(|h| h.trim().trim_start_matches('h').parse::<usize>().ok())
            .filter(|&n| n < hunks.len() && taken.insert(n))
            .collect();
        if ids.is_empty() {
            continue; // nothing left in it
        }
        ids.sort();
        commits.push((SavedCommit { title: c.title, why: c.why, next: c.next, hunks: vec![] }, ids));
    }

    // Hunks Claude didn't place: put each with another change to the same file when
    // possible, otherwise into a final commit.
    let mut leftover = vec![];
    for n in 0..hunks.len() {
        if taken.contains(&n) {
            continue;
        }
        let fi = hunks[n].0;
        match commits.iter_mut().find(|(_, ids)| ids.iter().any(|&m| hunks[m].0 == fi)) {
            Some((_, ids)) => {
                ids.push(n);
                ids.sort();
            }
            None => leftover.push(n),
        }
    }
    let auto_placed = hunks.len() - taken.len();
    if !leftover.is_empty() {
        commits.push((
            SavedCommit {
                title: "Remaining changes".into(),
                why: "Changes the story didn't place; included so that the commits add up to the whole PR.".into(),
                next: String::new(),
                hunks: vec![],
            },
            leftover,
        ));
    }

    SavedStory {
        summary: story.summary,
        commits: commits
            .into_iter()
            .map(|(mut c, ids)| {
                c.hunks = ids.into_iter().map(|n| hunk_ref(files, hunks[n].0, hunks[n].1)).collect();
                c
            })
            .collect(),
        done: BTreeSet::new(),
        head_sha,
        auto_placed,
    }
}

/// Maps a saved story onto the loaded diff. `None` when the diff changed so much that some
/// story hunk no longer exists (the story is outdated).
pub fn resolve(story: &SavedStory, files: &[FileDiff]) -> Option<StoryFocus> {
    let index: HashMap<HunkRef, (usize, usize)> =
        all_hunks(files).into_iter().map(|(fi, hi)| (hunk_ref(files, fi, hi), (fi, hi))).collect();
    let mut owner = HashMap::new();
    for (ci, c) in story.commits.iter().enumerate() {
        for h in &c.hunks {
            owner.insert(*index.get(h)?, ci);
        }
    }
    // A diff hunk that no commit covers also means the story is outdated.
    (owner.len() == index.len()).then_some(StoryFocus { current: 0, owner })
}

impl Prmator {
    /// Asks Claude to retell the PR as virtual commits.
    pub fn generate_story(&mut self, cx: &mut gpui::Context<Self>) {
        let id = self.next_request_id();
        let connected = self.connected;
        let Some(pv) = &mut self.pr else { return };
        let Load::Ready(files) = &pv.files else { return };
        if pv.story_run.is_some() {
            return;
        }
        if !connected {
            pv.story_error = Some("Claude bridge is not connected yet; try again in a moment.".into());
            return cx.notify();
        }
        self.link.send(Msg::Story {
            id,
            repo: pv.key.repo.clone(),
            pr_number: pv.key.number,
            pr_title: pv.summary.title.clone(),
            hunks: listing(files),
        });
        pv.story_run = Some(PlanRun { id, activity: vec![] });
        pv.story_error = None;
        pv.page = PrPage::Story;
        cx.notify();
    }

    /// Claude answered: validate, save, and start reading at the first commit.
    pub fn on_story_ready(&mut self, id: u64, story: protocol::Story, cx: &mut gpui::Context<Self>) {
        let Some(pv) = self.pr.as_mut().filter(|pv| pv.story_run.as_ref().is_some_and(|r| r.id == id)) else { return };
        pv.story_run = None;
        let head = pv.detail.as_ref().map(|d| d.head_ref_oid.clone()).unwrap_or_default();
        let saved = assemble(story, pv.files(), head);
        pv.story = Some(saved);
        let key = pv.key.store_key();
        self.store.progress(&key).story = pv.story.clone();
        self.store.save();
        self.go_to_commit(0, cx);
    }

    pub fn story_run_view(&mut self, id: u64) -> Option<&mut crate::model::PrView> {
        self.pr.as_mut().filter(|pv| pv.story_run.as_ref().is_some_and(|r| r.id == id))
    }

    /// Shows one virtual commit.
    pub fn go_to_commit(&mut self, commit: usize, cx: &mut gpui::Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        let Some(story) = &pv.story else { return };
        if commit >= story.commits.len() {
            return;
        }
        match resolve(story, pv.files()) {
            Some(mut focus) => {
                focus.current = commit;
                pv.story_focus = Some(focus);
                pv.story_outdated = false;
            }
            None => {
                pv.story_focus = None;
                pv.story_outdated = true;
            }
        }
        pv.page = PrPage::Story;
        pv.left_tab = crate::model::LeftTab::Story;
        pv.rebuild_rows();
        pv.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        cx.notify();
    }

    pub fn current_commit(&self) -> Option<usize> {
        self.pr.as_ref()?.story_focus.as_ref().map(|f| f.current)
    }

    /// Marks the current commit reviewed and moves to the next unreviewed one.
    pub fn next_commit(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(current) = self.current_commit() else { return self.continue_story(cx) };
        let Some(pv) = &mut self.pr else { return };
        let Some(story) = &mut pv.story else { return };
        story.done.insert(current);
        let next = (current + 1..story.commits.len()).find(|i| !story.done.contains(i)).or_else(|| story.next_commit());
        let key = pv.key.store_key();
        self.store.progress(&key).story = pv.story.clone();
        self.store.save();
        match next {
            Some(n) => self.go_to_commit(n, cx),
            None => cx.notify(),
        }
    }

    pub fn prev_commit(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some(current) = self.current_commit() {
            self.go_to_commit(current.saturating_sub(1), cx);
        }
    }

    /// Opens the story where the reviewer left off (or generates it).
    pub fn continue_story(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(pv) = &self.pr else { return };
        match &pv.story {
            Some(story) => {
                let n = story.next_commit().unwrap_or(0);
                self.go_to_commit(n, cx);
            }
            None => self.generate_story(cx),
        }
    }

    pub fn toggle_commit_done(&mut self, commit: usize, cx: &mut gpui::Context<Self>) {
        let Some(pv) = &mut self.pr else { return };
        let Some(story) = &mut pv.story else { return };
        if !story.done.remove(&commit) {
            story.done.insert(commit);
        }
        let key = pv.key.store_key();
        self.store.progress(&key).story = pv.story.clone();
        self.store.save();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse;

    fn files() -> Vec<FileDiff> {
        parse(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,1 +1,1 @@\n-a\n+A\n@@ -9,1 +9,1 @@\n-b\n+B\n\
diff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1,1 +1,1 @@\n-c\n+C\n\
diff --git a/c.rs b/c.rs\n--- a/c.rs\n+++ b/c.rs\n@@ -1,1 +1,1 @@\n-d\n+D\n",
        )
    }

    fn commit(title: &str, hunks: &[&str]) -> protocol::StoryCommit {
        protocol::StoryCommit {
            title: title.into(),
            why: "w".into(),
            next: String::new(),
            hunks: hunks.iter().map(|h| h.to_string()).collect(),
        }
    }

    #[test]
    fn listing_has_every_hunk() {
        let text = listing(&files());
        for n in 0..4 {
            assert!(text.contains(&format!("### h{n} — ")), "missing h{n}");
        }
        assert!(text.contains("### h1 — a.rs  @@ -9,1 +9,1 @@\n-b\n+B"));
    }

    #[test]
    fn assembled_story_covers_every_hunk_exactly_once() {
        let files = files();
        // h2 duplicated, h9 unknown, h1 and h3 missing.
        let story = protocol::Story {
            summary: "s".into(),
            commits: vec![commit("first", &["h2", "h0", "h9"]), commit("second", &["h2"]), commit("third", &["h0"])],
        };
        let saved = assemble(story, &files, "sha".into());
        // "second" and "third" only had duplicates: dropped. h1 joins a.rs's commit; h3 has
        // no sibling so it gets a final commit.
        let titles: Vec<&str> = saved.commits.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["first", "Remaining changes"]);
        assert_eq!(saved.auto_placed, 2);
        let mut covered: Vec<HunkRef> = saved.commits.iter().flat_map(|c| c.hunks.clone()).collect();
        let all: Vec<HunkRef> = all_hunks(&files).into_iter().map(|(fi, hi)| hunk_ref(&files, fi, hi)).collect();
        assert_eq!(covered.len(), all.len());
        covered.sort_by(|a, b| (&a.path, &a.header).cmp(&(&b.path, &b.header)));
        let mut expected = all.clone();
        expected.sort_by(|a, b| (&a.path, &a.header).cmp(&(&b.path, &b.header)));
        assert_eq!(covered, expected);

        let focus = resolve(&saved, &files).expect("resolves");
        assert_eq!(focus.owner.len(), 4);
        assert_eq!(focus.owner[&(2, 0)], 1);
    }

    #[test]
    fn outdated_story_does_not_resolve() {
        let files = files();
        let story = protocol::Story { summary: "s".into(), commits: vec![commit("all", &["h0", "h1", "h2", "h3"])] };
        let mut saved = assemble(story, &files, String::new());
        saved.commits[0].hunks[0].header = "@@ -100,1 +100,1 @@".into();
        assert!(resolve(&saved, &files).is_none());
    }
}
