//! GitHub access through the `gh` CLI (reuses the user's existing `gh auth login`).
//! All functions block; call them from the background executor.

use std::process::Command;

use anyhow::{bail, Context as _};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Filter {
    ReviewRequested,
    Mine,
    Involved,
}

impl Filter {
    pub const ALL: [Filter; 3] = [Filter::ReviewRequested, Filter::Mine, Filter::Involved];

    pub fn label(self) -> &'static str {
        match self {
            Filter::ReviewRequested => "To review",
            Filter::Mine => "Mine",
            Filter::Involved => "Involved",
        }
    }

    /// Searches whose results make up the list. "To review" keeps the PRs already
    /// reviewed: GitHub drops the review request once you review.
    fn search_qualifiers(self) -> &'static [&'static str] {
        match self {
            Filter::ReviewRequested => &["review-requested:@me", "reviewed-by:@me -author:@me"],
            Filter::Mine => &["author:@me"],
            Filter::Involved => &["involves:@me"],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Author {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub name_with_owner: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrSummary {
    pub number: u64,
    pub title: String,
    pub repository: Repository,
    pub author: Author,
    pub updated_at: String,
    pub url: String,
    pub is_draft: bool,
    /// Branches, to find stacked PRs (one PR's base is another's head).
    #[serde(default)]
    pub head_ref_name: String,
    #[serde(default)]
    pub base_ref_name: String,
    /// Where the signed-in user stands with this PR's review.
    #[serde(skip)]
    pub my_review: Option<MyReview>,
}

/// The signed-in user's reviews of a PR.
#[derive(Debug, Clone, PartialEq)]
pub struct MyReview {
    /// The verdict that counts: the last approval or change request, else "commented".
    pub state: MyReviewState,
    /// Reviewed, but not the PR as it is now: why.
    pub rereview: Option<Rereview>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MyReviewState {
    Approved,
    ChangesRequested,
    Commented,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rereview {
    /// The author asked for another review.
    Requested,
    /// Commits were pushed after the last review.
    NewCommits,
}

impl MyReview {
    /// Short label for the PR lists: "✓ approved", "↻ re-review · new commits", …
    pub fn label(&self) -> String {
        let verdict = match self.state {
            MyReviewState::Approved => "approved",
            MyReviewState::ChangesRequested => "changes requested",
            MyReviewState::Commented => "reviewed",
        };
        match self.rereview {
            None => format!("✓ {verdict}"),
            Some(Rereview::Requested) => "↻ re-review · asked again".into(),
            Some(Rereview::NewCommits) => "↻ re-review · new commits".into(),
        }
    }

    /// From the PR's GraphQL node (`headRefOid`, `myReviews`, `reviewRequests`).
    fn from_node(node: &Value, me: &str) -> Option<MyReview> {
        let reviews = node["myReviews"]["nodes"].as_array()?;
        let last = reviews.last()?;
        let state = reviews
            .iter()
            .rev()
            .find_map(|r| match r["state"].as_str()? {
                "APPROVED" => Some(MyReviewState::Approved),
                "CHANGES_REQUESTED" => Some(MyReviewState::ChangesRequested),
                _ => None,
            })
            .unwrap_or(MyReviewState::Commented);
        let requested = node["reviewRequests"]["nodes"]
            .as_array()
            .is_some_and(|r| r.iter().any(|r| r["requestedReviewer"]["login"].as_str() == Some(me)));
        let head = node["headRefOid"].as_str();
        let rereview = if requested {
            Some(Rereview::Requested)
        } else if head.is_some() && last["commit"]["oid"].as_str() != head {
            Some(Rereview::NewCommits)
        } else {
            None
        };
        Some(MyReview { state, rereview })
    }
}

impl PrSummary {
    pub fn repo(&self) -> &str {
        &self.repository.name_with_owner
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrDetail {
    pub title: String,
    pub author: Author,
    pub url: String,
    pub head_ref_name: String,
    pub base_ref_name: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    #[serde(default)]
    pub review_decision: String,
    #[serde(default)]
    pub head_ref_oid: String,
    #[serde(default)]
    pub base_ref_oid: String,
    #[serde(default)]
    pub body: String,
    /// OPEN, CLOSED or MERGED.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub comments: Vec<IssueComment>,
    #[serde(default)]
    pub reviews: Vec<Review>,
    #[serde(default)]
    pub review_requests: Vec<ReviewRequest>,
    #[serde(default)]
    pub status_check_rollup: Vec<Check>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Label {
    pub name: String,
    #[serde(default)]
    pub color: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueComment {
    pub author: Option<Author>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub author: Option<Author>,
    #[serde(default)]
    pub body: String,
    /// APPROVED, CHANGES_REQUESTED, COMMENTED, DISMISSED or PENDING.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub submitted_at: Option<String>,
}

/// A requested reviewer: a user (`login`) or a team (`name`).
#[derive(Debug, Clone, Deserialize)]
pub struct ReviewRequest {
    pub login: Option<String>,
    pub name: Option<String>,
}

/// A CI check run or commit status.
#[derive(Debug, Clone, Deserialize)]
pub struct Check {
    pub name: Option<String>,
    pub context: Option<String>,
    /// Check runs: SUCCESS, FAILURE, SKIPPED, … (when completed).
    pub conclusion: Option<String>,
    /// Commit statuses: SUCCESS, FAILURE, PENDING, ….
    pub state: Option<String>,
}

/// An inline review comment on a line of the diff.
#[derive(Debug, Clone, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    pub path: String,
    pub line: Option<u32>,
    pub original_line: Option<u32>,
    /// LEFT (old side) or RIGHT (new side).
    #[serde(default)]
    pub side: Option<String>,
    pub in_reply_to_id: Option<u64>,
    pub user: Option<Author>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub diff_hunk: String,
}

impl Check {
    /// "success", "failure", "pending", "skipped", "neutral", … (lowercase).
    pub fn outcome(&self) -> String {
        self.conclusion.clone().or_else(|| self.state.clone()).unwrap_or_else(|| "PENDING".into()).to_lowercase()
    }
}

fn gh(args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("gh")
        .args(args)
        .output()
        .context("failed to run `gh` (install GitHub CLI and run `gh auth login`)")?;
    if !out.status.success() {
        bail!("gh {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The PR fields the lists use, in GraphQL (`$me`: the signed-in user's login).
const PR_FIELDS: &str = "number title url isDraft updatedAt headRefName baseRefName headRefOid \
     author { login } repository { nameWithOwner } \
     myReviews: reviews(author: $me, last: 20, states: [APPROVED, CHANGES_REQUESTED, COMMENTED, DISMISSED]) { nodes { state commit { oid } } } \
     reviewRequests(first: 30) { nodes { requestedReviewer { ... on User { login } } } }";

/// The signed-in user's login (asked once).
fn me() -> anyhow::Result<&'static str> {
    static ME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(me) = ME.get() {
        return Ok(me);
    }
    let login = viewer_login()?;
    Ok(ME.get_or_init(|| login))
}

fn graphql(query: &str, vars: &[(&str, &str)]) -> anyhow::Result<Value> {
    let query = format!("query={query}");
    let vars: Vec<String> = vars.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let mut args = vec!["api", "graphql", "-f", &query];
    for v in &vars {
        args.extend(["-f", v.as_str()]);
    }
    let json: Value = serde_json::from_str(&gh(&args)?)?;
    if let Some(e) = json["errors"].get(0) {
        bail!("GitHub: {}", e["message"].as_str().unwrap_or("query failed"));
    }
    Ok(json)
}

/// PRs from GraphQL nodes (a deleted author comes back as `null`).
fn pr_nodes(nodes: &Value, me: &str) -> Vec<PrSummary> {
    let Some(nodes) = nodes.as_array() else { return vec![] };
    nodes
        .iter()
        .filter(|n| n.get("number").is_some())
        .filter_map(|n| {
            let mut n = n.clone();
            if n["author"].is_null() {
                n["author"] = serde_json::json!({ "login": "ghost" });
            }
            let mut pr: PrSummary = serde_json::from_value(n.clone()).ok()?;
            pr.my_review = MyReview::from_node(&n, me);
            Some(pr)
        })
        .collect()
}

pub fn list_prs(filter: Filter) -> anyhow::Result<Vec<PrSummary>> {
    let query = format!(
        "query($q: String!, $me: String!) {{ search(query: $q, type: ISSUE, first: 100) {{ nodes {{ ... on PullRequest {{ {PR_FIELDS} }} }} }} }}"
    );
    let me = me()?;
    let search = |qualifier: &str| -> anyhow::Result<Vec<PrSummary>> {
        let q = format!("is:pr is:open archived:false sort:updated-desc {qualifier}");
        let json = graphql(&query, &[("q", &q), ("me", me)])?;
        Ok(pr_nodes(&json["data"]["search"]["nodes"], me))
    };
    let results: Vec<anyhow::Result<Vec<PrSummary>>> = std::thread::scope(|s| {
        let handles: Vec<_> = filter.search_qualifiers().iter().map(|q| s.spawn(move || search(q))).collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("search panicked")))).collect()
    });
    let mut seen = std::collections::HashSet::new();
    let mut prs = vec![];
    for result in results {
        for pr in result? {
            if seen.insert((pr.repo().to_string(), pr.number)) {
                prs.push(pr);
            }
        }
    }
    prs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(prs)
}

/// Every open PR of a repo (up to 500), to follow stacks beyond the PRs in a list.
pub fn open_prs(repo: &str) -> anyhow::Result<Vec<PrSummary>> {
    let (owner, name) = repo.split_once('/').context("repo should be owner/name")?;
    let query = format!(
        "query($owner: String!, $name: String!, $after: String, $me: String!) {{ repository(owner: $owner, name: $name) {{ \
         pullRequests(states: OPEN, first: 100, after: $after) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ {PR_FIELDS} }} }} }} }}"
    );
    let me = me()?;
    let mut prs = vec![];
    let mut after: Option<String> = None;
    for _ in 0..5 {
        let mut vars = vec![("owner", owner), ("name", name), ("me", me)];
        if let Some(a) = &after {
            vars.push(("after", a));
        }
        let json = graphql(&query, &vars)?;
        let page = &json["data"]["repository"]["pullRequests"];
        prs.extend(pr_nodes(&page["nodes"], me));
        match page["pageInfo"]["endCursor"].as_str() {
            Some(cursor) if page["pageInfo"]["hasNextPage"] == true => after = Some(cursor.to_string()),
            _ => break,
        }
    }
    Ok(prs)
}

pub fn pr_detail(repo: &str, number: u64) -> anyhow::Result<PrDetail> {
    let json = gh(&[
        "pr",
        "view",
        &number.to_string(),
        "--repo",
        repo,
        "--json",
        "title,author,url,headRefName,baseRefName,additions,deletions,changedFiles,reviewDecision,headRefOid,baseRefOid,\
         body,state,isDraft,createdAt,labels,comments,reviews,reviewRequests,statusCheckRollup",
    ])?;
    Ok(serde_json::from_str(&json)?)
}

/// A new review comment on lines of the diff (GitHub's review comment fields).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NewComment {
    pub path: String,
    pub body: String,
    /// Last line of the range, on `side`.
    pub line: u32,
    /// RIGHT for new/context lines, LEFT for removed lines.
    pub side: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_side: Option<&'static str>,
}

/// COMMENT, APPROVE or REQUEST_CHANGES.
pub fn review_payload(commit_id: &str, body: &str, event: &str, comments: &[NewComment]) -> Value {
    serde_json::json!({ "commit_id": commit_id, "body": body, "event": event, "comments": comments })
}

/// Sends a JSON request to the GitHub API. With `PRMATOR_DRY_RUN` set, prints it instead
/// (for testing without touching real PRs).
fn gh_post(path: &str, payload: &Value) -> anyhow::Result<Value> {
    if std::env::var_os("PRMATOR_DRY_RUN").is_some() {
        eprintln!("[dry run] POST {path}\n{}", serde_json::to_string_pretty(payload)?);
        return Ok(serde_json::json!({ "dry_run": true }));
    }
    let mut child = Command::new("gh")
        .args(["api", "--method", "POST", path, "--input", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to run `gh`")?;
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(payload.to_string().as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        // GitHub explains failures in the JSON body ("message", "errors").
        let body: Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
        let mut msg = body["message"].as_str().unwrap_or_default().to_string();
        for e in body["errors"].as_array().into_iter().flatten() {
            let detail = e.as_str().map(String::from).or_else(|| e["message"].as_str().map(String::from));
            if let Some(d) = detail {
                msg.push_str(&format!(" — {d}"));
            }
        }
        if msg.is_empty() {
            msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        }
        bail!("GitHub refused: {msg}");
    }
    Ok(serde_json::from_slice(&out.stdout).unwrap_or_default())
}

/// Self-test of the POST path against GitHub's side-effect-free markdown endpoint.
pub fn selftest_post(text: Option<&str>) -> anyhow::Result<Value> {
    let payload = match text {
        Some(t) => serde_json::json!({ "text": t, "mode": "markdown" }),
        None => serde_json::json!({ "mode": "not-a-mode" }),
    };
    gh_post("markdown", &payload)
}

/// Submits a review (with its pending comments) in one go.
pub fn submit_review(repo: &str, number: u64, commit_id: &str, body: &str, event: &str, comments: &[NewComment]) -> anyhow::Result<()> {
    gh_post(&format!("repos/{repo}/pulls/{number}/reviews"), &review_payload(commit_id, body, event, comments))?;
    Ok(())
}

/// Posts one comment right away (outside a review).
pub fn post_comment(repo: &str, number: u64, commit_id: &str, comment: &NewComment) -> anyhow::Result<()> {
    let mut payload = serde_json::to_value(comment)?;
    payload["commit_id"] = Value::String(commit_id.to_string());
    gh_post(&format!("repos/{repo}/pulls/{number}/comments"), &payload)?;
    Ok(())
}

/// Replies to an inline comment thread.
pub fn reply_to_comment(repo: &str, number: u64, comment_id: u64, body: &str) -> anyhow::Result<()> {
    gh_post(&format!("repos/{repo}/pulls/{number}/comments/{comment_id}/replies"), &serde_json::json!({ "body": body }))?;
    Ok(())
}

/// Inline review comments (the threads on diff lines).
pub fn review_comments(repo: &str, number: u64) -> anyhow::Result<Vec<ReviewComment>> {
    let out = gh(&["api", "--paginate", &format!("repos/{repo}/pulls/{number}/comments?per_page=100"), "--jq", ".[] | tojson"])?;
    out.lines().filter(|l| !l.trim().is_empty()).map(|l| Ok(serde_json::from_str(l)?)).collect()
}

/// The PR's unified diff. Falls back to the per-file API when the diff endpoint refuses
/// (GitHub caps it for very large PRs).
pub fn pr_diff(repo: &str, number: u64) -> anyhow::Result<String> {
    match gh(&["pr", "diff", &number.to_string(), "--repo", repo]) {
        Ok(diff) => Ok(diff),
        Err(e) => pr_diff_from_files(repo, number).with_context(|| format!("{e:#}")),
    }
}

fn pr_diff_from_files(repo: &str, number: u64) -> anyhow::Result<String> {
    let out = gh(&[
        "api",
        "--paginate",
        &format!("repos/{repo}/pulls/{number}/files?per_page=100"),
        "--jq",
        ".[] | tojson",
    ])?;
    let mut diff = String::new();
    for line in out.lines() {
        let f: Value = serde_json::from_str(line)?;
        let path = f["filename"].as_str().unwrap_or_default();
        let old = f["previous_filename"].as_str().unwrap_or(path);
        diff.push_str(&format!("diff --git a/{old} b/{path}\n"));
        match f["status"].as_str() {
            Some("added") => diff.push_str("new file mode 100644\n"),
            Some("removed") => diff.push_str("deleted file mode 100644\n"),
            Some("renamed") => diff.push_str(&format!("rename from {old}\nrename to {path}\n")),
            _ => {}
        }
        match f["patch"].as_str() {
            Some(patch) => {
                diff.push_str(&format!("--- a/{old}\n+++ b/{path}\n{patch}\n"));
            }
            None => diff.push_str("Binary files differ\n"),
        }
    }
    Ok(diff)
}

/// `owner/repo#123` or `https://github.com/owner/repo/pull/123[/...]`.
pub fn parse_pr_ref(s: &str) -> Option<(String, u64)> {
    if let Some((repo, n)) = s.split_once('#') {
        return Some((repo.to_string(), n.parse().ok()?));
    }
    let path = s.strip_prefix("https://github.com/")?;
    let parts: Vec<&str> = path.split('/').collect();
    match parts[..] {
        [owner, name, "pull", n, ..] => Some((format!("{owner}/{name}"), n.parse().ok()?)),
        _ => None,
    }
}

impl PrSummary {
    /// Placeholder until `pr_detail` fills it in (PR opened from the command line).
    pub fn stub(repo: String, number: u64) -> Self {
        PrSummary {
            number,
            title: format!("{repo}#{number}"),
            url: format!("https://github.com/{repo}/pull/{number}"),
            repository: Repository { name_with_owner: repo },
            author: Author { login: String::new() },
            updated_at: String::new(),
            is_draft: false,
            head_ref_name: String::new(),
            base_ref_name: String::new(),
            my_review: None,
        }
    }
}

/// The commit the PR diff is computed against (merge base of base and head).
pub fn merge_base(repo: &str, base: &str, head: &str) -> anyhow::Result<String> {
    let sha = gh(&[
        "api",
        &format!("repos/{repo}/compare/{base}...{head}"),
        "--jq",
        ".merge_base_commit.sha",
    ])?;
    Ok(sha.trim().to_string())
}

/// Raw contents of `path` at commit `sha`.
pub fn file_content(repo: &str, sha: &str, path: &str) -> anyhow::Result<String> {
    let encoded: String = path
        .chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '#' => "%23".to_string(),
            '?' => "%3F".to_string(),
            '%' => "%25".to_string(),
            c => c.to_string(),
        })
        .collect();
    gh(&[
        "api",
        "-H",
        "Accept: application/vnd.github.raw+json",
        &format!("repos/{repo}/contents/{encoded}?ref={sha}"),
    ])
}

/// The signed-in user's login.
pub fn viewer_login() -> anyhow::Result<String> {
    Ok(gh(&["api", "user", "--jq", ".login"])?.trim().to_string())
}

pub fn open_in_browser(url: &str) {
    let _ = Command::new("open").arg(url).spawn();
}

/// "2026-10-19T10:27:00Z" -> "3h ago" (relative to now, coarse).
pub fn relative_time(iso: &str) -> String {
    let Some(then) = parse_iso(iso) else { return iso.to_string() };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(then);
    let secs = (now - then).max(0);
    match secs {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

fn parse_iso(s: &str) -> Option<i64> {
    // YYYY-MM-DDTHH:MM:SSZ
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (h, mi, se) = (n(11..13)?, n(14..16)?, n(17..19)?);
    // Days from civil (Howard Hinnant).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + se)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn my_review_status() {
        let node = |reviews: Value, requested: &[&str]| {
            serde_json::json!({
                "headRefOid": "head",
                "myReviews": { "nodes": reviews },
                "reviewRequests": { "nodes": requested.iter().map(|l| serde_json::json!({ "requestedReviewer": { "login": l } })).collect::<Vec<_>>() },
            })
        };
        let review = |state: &str, oid: &str| serde_json::json!({ "state": state, "commit": { "oid": oid } });
        assert_eq!(MyReview::from_node(&node(serde_json::json!([]), &[]), "me"), None);
        // Approved, then commented on the current head: still approved, up to date.
        let r = MyReview::from_node(&node(serde_json::json!([review("APPROVED", "old"), review("COMMENTED", "head")]), &[]), "me").unwrap();
        assert_eq!((r.state, r.rereview), (MyReviewState::Approved, None));
        assert_eq!(r.label(), "✓ approved");
        // Commits since: re-review.
        let r = MyReview::from_node(&node(serde_json::json!([review("COMMENTED", "old")]), &["other"]), "me").unwrap();
        assert_eq!((r.state, r.rereview), (MyReviewState::Commented, Some(Rereview::NewCommits)));
        // Asked again wins over new commits.
        let r = MyReview::from_node(&node(serde_json::json!([review("CHANGES_REQUESTED", "old")]), &["me"]), "me").unwrap();
        assert_eq!((r.state, r.rereview), (MyReviewState::ChangesRequested, Some(Rereview::Requested)));
    }
}
