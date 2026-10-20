//! Unified diff parsing (the output of `gh pr diff`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    /// `\ No newline at end of file`
    Meta,
}

#[derive(Debug, Clone)]
pub struct Line {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: String,
}

impl Line {
    /// The line as it appears in a unified diff.
    pub fn as_diff(&self) -> String {
        let sign = match self.kind {
            LineKind::Context => ' ',
            LineKind::Added => '+',
            LineKind::Removed => '-',
            LineKind::Meta => '\\',
        };
        format!("{sign}{}", self.text)
    }
}

#[derive(Debug, Clone)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<Line>,
}

impl Hunk {
    /// `(old_start, old_len, new_start, new_len)` from the `@@` header.
    pub fn range(&self) -> (u32, u32, u32, u32) {
        ranges(&self.header).map_or((0, 0, 0, 0), |(o, n)| {
            let ((os, ol), (ns, nl)) = (parse_range(o), parse_range(n));
            (os, ol, ns, nl)
        })
    }

    /// Last line covered on each side, as `(old, new)`. For a side with no lines, the
    /// header's start is the line *before* the hunk, which is also its "end".
    pub fn end(&self) -> (u32, u32) {
        let (os, ol, ns, nl) = self.range();
        (if ol == 0 { os } else { os + ol - 1 }, if nl == 0 { ns } else { ns + nl - 1 })
    }

    /// The line just before the hunk on each side, as `(old, new)`.
    pub fn before(&self) -> (u32, u32) {
        let (os, ol, ns, nl) = self.range();
        (if ol == 0 { os } else { os - 1 }, if nl == 0 { ns } else { ns - 1 })
    }
}

#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    pub additions: usize,
    pub deletions: usize,
}

impl FileDiff {
    fn new(path: String) -> Self {
        FileDiff {
            path,
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            hunks: vec![],
            additions: 0,
            deletions: 0,
        }
    }

    pub fn line_count(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len() + 1).sum()
    }

    /// Lock files, generated code and vendored files that are rarely worth reading line by line.
    pub fn is_probably_generated(&self) -> bool {
        const NAMES: &[&str] = &[
            "Cargo.lock",
            "package-lock.json",
            "yarn.lock",
            "pnpm-lock.yaml",
            "Podfile.lock",
            "Package.resolved",
            "poetry.lock",
            "uv.lock",
            "go.sum",
            "Gemfile.lock",
        ];
        let name = self.path.rsplit('/').next().unwrap_or(&self.path);
        NAMES.contains(&name)
            || self.path.contains(".generated.")
            || self.path.contains("/generated/")
            || self.path.ends_with(".min.js")
            || self.path.ends_with(".pb.go")
            || self.path.ends_with(".snap")
    }
}

pub fn parse(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = vec![];
    let (mut old_no, mut new_no) = (0u32, 0u32);

    for raw in text.lines() {
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            files.push(FileDiff::new(path_from_git_header(rest)));
            continue;
        }
        let Some(file) = files.last_mut() else { continue };

        // Inside a hunk, content lines win over header detection (a removed line may
        // legitimately start with "--").
        if let Some(hunk) = file.hunks.last_mut() {
            let line = match raw.as_bytes().first() {
                Some(b'+') if !raw.starts_with("+++ ") || hunk_open(hunk) => {
                    file.additions += 1;
                    new_no += 1;
                    Some(Line { kind: LineKind::Added, old_no: None, new_no: Some(new_no), text: raw[1..].into() })
                }
                Some(b'-') if !raw.starts_with("--- ") || hunk_open(hunk) => {
                    file.deletions += 1;
                    old_no += 1;
                    Some(Line { kind: LineKind::Removed, old_no: Some(old_no), new_no: None, text: raw[1..].into() })
                }
                Some(b' ') => {
                    old_no += 1;
                    new_no += 1;
                    Some(Line {
                        kind: LineKind::Context,
                        old_no: Some(old_no),
                        new_no: Some(new_no),
                        text: raw[1..].into(),
                    })
                }
                // Some tools strip the trailing space of empty context lines.
                None if hunk_open(hunk) => {
                    old_no += 1;
                    new_no += 1;
                    Some(Line { kind: LineKind::Context, old_no: Some(old_no), new_no: Some(new_no), text: String::new() })
                }
                Some(b'\\') => {
                    Some(Line { kind: LineKind::Meta, old_no: None, new_no: None, text: raw[1..].trim().into() })
                }
                _ => None,
            };
            if let Some(line) = line {
                hunk.lines.push(line);
                continue;
            }
        }

        if raw.starts_with("@@") {
            let (o, n) = parse_hunk_start(raw);
            old_no = o.saturating_sub(1);
            new_no = n.saturating_sub(1);
            file.hunks.push(Hunk { header: raw.to_string(), lines: vec![] });
        } else if raw.starts_with("new file mode") {
            file.status = FileStatus::Added;
        } else if raw.starts_with("deleted file mode") {
            file.status = FileStatus::Deleted;
        } else if let Some(from) = raw.strip_prefix("rename from ") {
            file.status = FileStatus::Renamed;
            file.old_path = Some(from.to_string());
        } else if let Some(to) = raw.strip_prefix("rename to ") {
            file.path = to.to_string();
        } else if raw.starts_with("Binary files") || raw.starts_with("GIT binary patch") {
            file.binary = true;
        } else if let Some(p) = raw.strip_prefix("+++ ") {
            if let Some(p) = p.strip_prefix("b/") {
                file.path = p.to_string();
            }
        }
    }
    files
}

/// Whether the hunk still expects lines according to its `@@` header counts.
fn hunk_open(hunk: &Hunk) -> bool {
    let (old_len, new_len) = parse_hunk_lens(&hunk.header);
    let old_seen = hunk.lines.iter().filter(|l| matches!(l.kind, LineKind::Context | LineKind::Removed)).count();
    let new_seen = hunk.lines.iter().filter(|l| matches!(l.kind, LineKind::Context | LineKind::Added)).count();
    old_seen < old_len as usize || new_seen < new_len as usize
}

/// `a/foo b/foo` -> `foo` (best effort; the `+++` / `rename to` lines refine it).
fn path_from_git_header(rest: &str) -> String {
    let rest = rest.trim();
    if let Some(i) = rest.rfind(" b/") {
        return rest[i + 3..].to_string();
    }
    rest.strip_prefix("a/").unwrap_or(rest).to_string()
}

fn ranges(header: &str) -> Option<(&str, &str)> {
    let mut parts = header.split_whitespace().skip(1);
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    Some((old, new))
}

fn parse_range(r: &str) -> (u32, u32) {
    let mut it = r.split(',');
    let start = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let len = it.next().map_or(1, |s| s.parse().unwrap_or(1));
    (start, len)
}

fn parse_hunk_start(header: &str) -> (u32, u32) {
    ranges(header).map_or((1, 1), |(o, n)| (parse_range(o).0.max(1), parse_range(n).0.max(1)))
}

fn parse_hunk_lens(header: &str) -> (u32, u32) {
    ranges(header).map_or((0, 0), |(o, n)| (parse_range(o).1, parse_range(n).1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modified_added_renamed() {
        let diff = "\
diff --git a/src/a.rs b/src/a.rs
index 1..2 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,4 @@ fn main() {
 one
-two
+deux
+-- not a header
 three
diff --git a/new.txt b/new.txt
new file mode 100644
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hello
\\ No newline at end of file
diff --git a/old name.md b/new name.md
similarity index 100%
rename from old name.md
rename to new name.md
";
        let files = parse(diff);
        assert_eq!(files.len(), 3);
        let a = &files[0];
        assert_eq!(a.path, "src/a.rs");
        assert_eq!((a.additions, a.deletions), (2, 1));
        let lines = &a.hunks[0].lines;
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[3].text, "-- not a header");
        assert_eq!(lines[4].old_no, Some(3));
        assert_eq!(lines[4].new_no, Some(4));
        assert_eq!(files[1].status, FileStatus::Added);
        assert_eq!(files[1].hunks[0].lines[0].new_no, Some(1));
        assert_eq!(files[1].hunks[0].lines[1].kind, LineKind::Meta);
        assert_eq!(files[2].status, FileStatus::Renamed);
        assert_eq!(files[2].path, "new name.md");
        assert_eq!(files[2].old_path.as_deref(), Some("old name.md"));
    }
}
