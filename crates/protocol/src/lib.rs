//! Messages exchanged between the PRmator app and the `prmator-claude` bridge
//! (JSON text frames over a local WebSocket).
//!
//!   prmator (GPUI)  <--ws-->  prmator-claude  <--stdio-->  claude CLI (one per question)
//!
//! Every question carries an `id` chosen by the app; all replies for it echo that id.

use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 7788;

/// The piece of the PR the user right-clicked on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeContext {
    /// `owner/name`
    pub repo: String,
    pub pr_number: u64,
    pub pr_title: String,
    pub file: String,
    /// Human label such as `L12-L20` or `hunk @@ -10,6 +10,9 @@`.
    pub location: String,
    /// The selected diff lines, in unified-diff form (with +/-/space prefixes).
    pub snippet: String,
}

/// An ordered review route through a PR, produced by Claude.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewPlan {
    /// Two or three sentences: what the PR does and how the pieces fit together.
    pub overview: String,
    pub steps: Vec<PlanStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    /// Short label, e.g. "Entry point: BatchOperation".
    pub title: String,
    /// Path of the file to open for this step (as in the diff).
    pub file: String,
    /// Symbol to jump to inside `file`, when the step is about one.
    #[serde(default)]
    pub symbol: Option<String>,
    /// Why this comes at this point of the review.
    pub why: String,
    /// What to verify while reading it.
    #[serde(default)]
    pub check: String,
    /// Other files that belong to this step.
    #[serde(default)]
    pub related_files: Vec<String>,
}

impl ReviewPlan {
    /// JSON schema handed to `claude --json-schema`.
    pub fn json_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "overview": { "type": "string" },
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" },
                            "file": { "type": "string" },
                            "symbol": { "type": ["string", "null"] },
                            "why": { "type": "string" },
                            "check": { "type": "string" },
                            "related_files": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["title", "file", "why", "check"]
                    }
                }
            },
            "required": ["overview", "steps"]
        })
    }
}

/// A PR retold as an ordered series of small "virtual commits". Each commit groups some of
/// the PR's existing hunks (by id); together they cover every hunk exactly once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Story {
    /// Two or three sentences: the arc of the change.
    pub summary: String,
    pub commits: Vec<StoryCommit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoryCommit {
    /// Commit subject, imperative.
    pub title: String,
    /// Why this step exists and what it enables (not what the diff says).
    pub why: String,
    /// What later commits build on top of this one.
    #[serde(default)]
    pub next: String,
    /// Hunk ids ("h3") from the request.
    pub hunks: Vec<String>,
}

impl Story {
    /// JSON schema handed to `claude --json-schema`.
    pub fn json_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string" },
                "commits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" },
                            "why": { "type": "string" },
                            "next": { "type": "string" },
                            "hunks": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["title", "why", "hunks"]
                    }
                }
            },
            "required": ["summary", "commits"]
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Msg {
    // ---- app -> bridge
    /// Ask about a change. Questions with the same `conversation` continue one claude
    /// session; `history` is a transcript to replay when that session no longer exists
    /// (e.g. a conversation restored after a restart).
    Ask {
        id: u64,
        question: String,
        context: Option<ChangeContext>,
        repo: String,
        pr_number: u64,
        #[serde(default)]
        conversation: Option<u64>,
        #[serde(default)]
        history: Option<String>,
    },
    /// Ask Claude for a review route. `outline` lists the changed files and symbols.
    Plan { id: u64, repo: String, pr_number: u64, pr_title: String, outline: String },
    /// Ask Claude to retell the PR as virtual commits. `hunks` lists every hunk with its id.
    Story { id: u64, repo: String, pr_number: u64, pr_title: String, hunks: String },
    Cancel { id: u64 },
    /// Forget the claude session for a PR (next question starts fresh).
    ResetSession { repo: String, pr_number: u64 },

    // ---- bridge -> app
    Ready { claude_version: String },
    /// Streaming text chunk of the answer.
    Delta { id: u64, text: String },
    /// Claude is using a tool (shown as activity).
    Activity { id: u64, text: String },
    Done { id: u64, cost_usd: Option<f64> },
    /// Result of a `Plan` request (followed by `Done`).
    PlanReady { id: u64, plan: ReviewPlan },
    /// Result of a `Story` request (followed by `Done`).
    StoryReady { id: u64, story: Story },
    Error { id: Option<u64>, text: String },
}

impl Msg {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("Msg serializes")
    }
    pub fn from_json(s: &str) -> Option<Msg> {
        serde_json::from_str(s).ok()
    }
}
