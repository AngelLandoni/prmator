//! Folder tree of a PR's changed files, for the sidebar. Directories come first, and chains
//! of single-child directories are merged into one row (`crates/app/src`), like GitHub.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeRow {
    Dir {
        /// Full path, used as the key for collapsing.
        path: String,
        /// Display name (possibly several merged segments).
        name: String,
        depth: usize,
        /// Every file index below this directory.
        files: Vec<usize>,
    },
    File {
        fi: usize,
        name: String,
        depth: usize,
    },
}

#[derive(Default)]
struct Node {
    dirs: BTreeMap<String, Node>,
    files: Vec<(String, usize)>,
}

impl Node {
    fn all_files(&self, out: &mut Vec<usize>) {
        for child in self.dirs.values() {
            child.all_files(out);
        }
        out.extend(self.files.iter().map(|(_, fi)| *fi));
    }
}

/// Rows to display for `paths` (indexed like the PR's files), skipping the contents of
/// directories in `collapsed`.
pub fn rows(paths: &[&str], collapsed: &HashSet<String>) -> Vec<TreeRow> {
    let mut root = Node::default();
    for (fi, path) in paths.iter().enumerate() {
        let mut parts: Vec<&str> = path.split('/').collect();
        let name = parts.pop().unwrap_or_default();
        let mut node = &mut root;
        for dir in parts {
            node = node.dirs.entry(dir.to_string()).or_default();
        }
        node.files.push((name.to_string(), fi));
    }
    let mut out = vec![];
    flatten(&root, "", 0, collapsed, &mut out);
    out
}

fn flatten(node: &Node, prefix: &str, depth: usize, collapsed: &HashSet<String>, out: &mut Vec<TreeRow>) {
    let mut dirs: Vec<(&String, &Node)> = node.dirs.iter().collect();
    dirs.sort_by(|a, b| name_order(a.0, b.0));
    for (name, mut child) in dirs {
        let mut name = name.clone();
        // Merge `a/` → `b/` while `a` holds nothing but `b`.
        while child.files.is_empty() && child.dirs.len() == 1 {
            let (sub, grandchild) = child.dirs.iter().next().unwrap();
            name = format!("{name}/{sub}");
            child = grandchild;
        }
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let mut files = vec![];
        child.all_files(&mut files);
        files.sort();
        out.push(TreeRow::Dir { path: path.clone(), name, depth, files });
        if !collapsed.contains(&path) {
            flatten(child, &path, depth + 1, collapsed, out);
        }
    }
    let mut files: Vec<&(String, usize)> = node.files.iter().collect();
    files.sort_by(|a, b| name_order(&a.0, &b.0));
    out.extend(files.into_iter().map(|(name, fi)| TreeRow::File { fi: *fi, name: name.clone(), depth }));
}

fn name_order(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b))
}

/// Orders paths the way the tree shows them (directories before files at each level), so
/// the diff reads top to bottom in the same order as the sidebar.
pub fn tree_order(a: &str, b: &str) -> Ordering {
    let (pa, pb): (Vec<&str>, Vec<&str>) = (a.split('/').collect(), b.split('/').collect());
    for i in 0.. {
        let (Some(x), Some(y)) = (pa.get(i), pb.get(i)) else { return pa.len().cmp(&pb.len()) };
        let (x_dir, y_dir) = (i + 1 < pa.len(), i + 1 < pb.len());
        if x_dir != y_dir {
            return if x_dir { Ordering::Less } else { Ordering::Greater };
        }
        match name_order(x, y) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATHS: &[&str] = &[
        "Cargo.lock",
        "crates/app/Cargo.toml",
        "crates/app/src/batch.rs",
        "crates/app/src/batch/history.rs",
        "crates/model/src/models/pim.rs",
        "README.md",
    ];

    fn render(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                TreeRow::Dir { name, depth, files, .. } => format!("{}{name}/ {files:?}", "  ".repeat(*depth)),
                TreeRow::File { name, depth, .. } => format!("{}{name}", "  ".repeat(*depth)),
            })
            .collect()
    }

    #[test]
    fn builds_compressed_tree() {
        assert_eq!(
            render(&rows(PATHS, &HashSet::new())),
            vec![
                "crates/ [1, 2, 3, 4]",
                "  app/ [1, 2, 3]",
                "    src/ [2, 3]",
                "      batch/ [3]",
                "        history.rs",
                "      batch.rs",
                "    Cargo.toml",
                "  model/src/models/ [4]",
                "    pim.rs",
                "Cargo.lock",
                "README.md",
            ]
        );
    }

    #[test]
    fn collapsed_dirs_hide_children() {
        let collapsed = HashSet::from(["crates/app".to_string()]);
        assert_eq!(
            render(&rows(PATHS, &collapsed)),
            vec!["crates/ [1, 2, 3, 4]", "  app/ [1, 2, 3]", "  model/src/models/ [4]", "    pim.rs", "Cargo.lock", "README.md"]
        );
    }

    #[test]
    fn diff_order_matches_tree() {
        let mut paths = PATHS.to_vec();
        paths.sort_by(|a, b| tree_order(a, b));
        assert_eq!(
            paths,
            vec![
                "crates/app/src/batch/history.rs",
                "crates/app/src/batch.rs",
                "crates/app/Cargo.toml",
                "crates/model/src/models/pim.rs",
                "Cargo.lock",
                "README.md",
            ]
        );
    }
}
