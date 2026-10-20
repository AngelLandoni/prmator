//! Stacked PRs: a PR whose base branch is another open PR's head builds on it. The PR
//! lists group such PRs into stacks, in merge order (the PR closest to the trunk first).

use std::collections::{HashMap, HashSet};

use crate::github::PrSummary;

/// A branch that more than this many open PRs target is a long-lived integration
/// branch (a `develop` with a release PR), not a stack.
const MAX_CHILDREN: usize = 3;

pub enum Entry {
    Single(PrSummary),
    Stack(Stack),
}

pub struct Stack {
    /// The branch the bottom PR merges into.
    pub base: String,
    /// Members in merge order (depth first, so a branch of the stack follows its parent).
    pub rows: Vec<StackRow>,
    /// Longest chain from the bottom PR to a top one.
    pub steps: usize,
}

pub struct StackRow {
    pub pr: PrSummary,
    /// 1 for the bottom PR, 2 for the PRs built on it, …
    pub step: usize,
    /// In the list being shown (else only pulled in to complete the stack).
    pub listed: bool,
}

impl Stack {
    /// Identifies the stack across renders (its bottom PR).
    pub fn key(&self) -> (String, u64) {
        let root = &self.rows[0].pr;
        (root.repo().to_string(), root.number)
    }
}

/// Branch links between PRs, keyed by (repo, branch).
#[derive(Default)]
struct Links<'a> {
    by_head: HashMap<(String, String), &'a PrSummary>,
    children: HashMap<(String, String), Vec<&'a PrSummary>>,
}

impl<'a> Links<'a> {
    /// The PRs built on `p` (none when its branch is an integration branch).
    fn kids(&self, p: &PrSummary) -> &[&'a PrSummary] {
        match self.children.get(&(p.repo().to_string(), p.head_ref_name.clone())) {
            Some(k) if k.len() <= MAX_CHILDREN && !p.head_ref_name.is_empty() => k,
            _ => &[],
        }
    }

    /// The PR `p` is built on.
    fn parent(&self, p: &PrSummary) -> Option<&'a PrSummary> {
        let parent = *self.by_head.get(&(p.repo().to_string(), p.base_ref_name.clone()))?;
        (parent.number != p.number && self.kids(parent).iter().any(|k| k.number == p.number)).then_some(parent)
    }
}

/// Groups `listed` (kept in its order, each stack where its first member was) using
/// the PRs in `listed` and `open` (other open PRs of the same repos) to link them.
pub fn group(listed: &[PrSummary], open: &[PrSummary]) -> Vec<Entry> {
    let in_list: HashSet<(&str, u64)> = listed.iter().map(|p| (p.repo(), p.number)).collect();
    let mut seen = HashSet::new();
    let pool: Vec<&PrSummary> =
        listed.iter().chain(open).filter(|p| seen.insert((p.repo(), p.number))).collect();

    let mut links = Links::default();
    for p in &pool {
        if !p.head_ref_name.is_empty() {
            links.by_head.entry((p.repo().to_string(), p.head_ref_name.clone())).or_insert(p);
        }
        links.children.entry((p.repo().to_string(), p.base_ref_name.clone())).or_default().push(p);
    }
    for kids in links.children.values_mut() {
        kids.sort_by_key(|p| p.number);
    }

    let mut done: HashSet<(String, u64)> = HashSet::new();
    let mut entries = vec![];
    for pr in listed {
        if done.contains(&(pr.repo().to_string(), pr.number)) {
            continue;
        }
        // Down to the bottom of the stack (guarding against branch cycles).
        let mut root = pr;
        let mut visited = HashSet::from([root.number]);
        while let Some(parent) = links.parent(root).filter(|p| visited.insert(p.number)) {
            root = parent;
        }
        let mut rows = vec![];
        let mut todo = vec![(root, 1)];
        let mut placed = HashSet::new();
        while let Some((p, step)) = todo.pop() {
            if !placed.insert(p.number) {
                continue;
            }
            rows.push(StackRow { pr: p.clone(), step, listed: in_list.contains(&(p.repo(), p.number)) });
            todo.extend(links.kids(p).iter().rev().map(|k| (*k, step + 1)));
        }
        for row in rows.iter().filter(|r| r.listed) {
            done.insert((row.pr.repo().to_string(), row.pr.number));
        }
        if rows.len() == 1 {
            entries.push(Entry::Single(pr.clone()));
        } else {
            let steps = rows.iter().map(|r| r.step).max().unwrap_or(1);
            entries.push(Entry::Stack(Stack { base: root.base_ref_name.clone(), rows, steps }));
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(number: u64, base: &str, head: &str) -> PrSummary {
        let mut p = PrSummary::stub("o/r".into(), number);
        p.base_ref_name = base.into();
        p.head_ref_name = head.into();
        p
    }

    fn numbers(e: &Entry) -> Vec<(u64, usize, bool)> {
        match e {
            Entry::Single(p) => vec![(p.number, 0, true)],
            Entry::Stack(s) => s.rows.iter().map(|r| (r.pr.number, r.step, r.listed)).collect(),
        }
    }

    #[test]
    fn groups_stacks_in_merge_order() {
        // 1 ← 2 ← 3 on main, listed top first; 9 alone; 4 pulled in from the repo.
        let listed = [pr(3, "b", "c"), pr(9, "main", "x"), pr(2, "a", "b")];
        let open = [pr(1, "main", "a"), pr(4, "c", "d"), pr(3, "b", "c")];
        let entries = group(&listed, &open);
        assert_eq!(entries.len(), 2);
        assert_eq!(numbers(&entries[0]), [(1, 1, false), (2, 2, true), (3, 3, true), (4, 4, false)]);
        assert_eq!(numbers(&entries[1]), [(9, 0, true)]);
        let Entry::Stack(s) = &entries[0] else { panic!() };
        assert_eq!((s.base.as_str(), s.steps, s.key().1), ("main", 4, 1));
    }

    #[test]
    fn branches_follow_their_parent() {
        let listed = [pr(1, "main", "a"), pr(2, "a", "b"), pr(3, "a", "c"), pr(4, "b", "d")];
        let entries = group(&listed, &[]);
        assert_eq!(entries.len(), 1);
        assert_eq!(numbers(&entries[0]), [(1, 1, true), (2, 2, true), (4, 3, true), (3, 2, true)]);
    }

    #[test]
    fn integration_branches_are_not_stacks() {
        // Five PRs target `develop`, which is itself a PR's head (a release PR).
        let mut listed = vec![pr(1, "main", "develop")];
        listed.extend((2..=6).map(|n| pr(n, "develop", &format!("f{n}"))));
        let entries = group(&listed, &[]);
        assert_eq!(entries.len(), 6);
        assert!(entries.iter().all(|e| matches!(e, Entry::Single(_))));
    }

    #[test]
    fn cycles_end() {
        let listed = [pr(1, "b", "a"), pr(2, "a", "b")];
        let entries = group(&listed, &[]);
        assert_eq!(entries.len(), 1);
        assert_eq!(numbers(&entries[0]).len(), 2);
    }
}
