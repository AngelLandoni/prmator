//! prmator-claude: background WebSocket server that answers PRmator's questions by
//! running the `claude` CLI.
//!
//!   prmator-claude [--port 7788] [--exit-with-parent]
//!
//! Each `Ask` spawns one `claude -p` turn (streaming JSON). Follow-up questions about the
//! same PR `--resume` the previous session so Claude keeps the conversation. `Plan` runs a
//! turn with `--json-schema` and returns a structured review route.
//!
//! Environment:
//!   PRMATOR_CLAUDE_BIN    claude executable (default `claude`)
//!   PRMATOR_CLAUDE_ARGS   extra args, e.g. "--model sonnet"
//!   PRMATOR_ALLOW_API_KEY keep ANTHROPIC_API_KEY / ANTHROPIC_AUTH_TOKEN for the CLI
//!                         (by default they are removed, so it uses your Claude login)
//!   PRMATOR_CODE_DIR      directory with local clones (`$dir/<repo name>`); when the PR's
//!                         repo is there, Claude runs inside it and may read the code.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use futures_util::{SinkExt, StreamExt};
use protocol::{ChangeContext, Msg, ReviewPlan, Story, DEFAULT_PORT};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

const SYSTEM_PROMPT: &str = "\
You are assisting a senior engineer who is reviewing a GitHub pull request inside PRmator, \
a fast PR review tool. They select lines of the diff and ask you about them. \
Answer precisely and concisely: lead with the answer, point out real bugs, risks, edge cases \
and simpler alternatives, and skip praise and filler. Quote code only when it helps. \
You can run `gh pr view` and `gh pr diff` (with --repo) to see the rest of the PR when the \
snippet alone is not enough. Never modify the PR, push, comment or approve.";

const PLAN_INSTRUCTIONS: &str = "\
Plan the order in which a reviewer should read this pull request, like a pipeline: \
start at the entry point of the change (the type, operation, API or function that drives \
everything else, e.g. the main operation a feature adds), then follow the flow to the code \
it calls or that depends on it, then data/model/schema changes, then wiring and config, and \
finish with tests, fixtures and generated files. Group trivially related files into one step \
via `related_files`; every changed file that matters should appear in some step. \
Use 3 to 12 steps. `file` must be a path exactly as listed below. `symbol` is the exact \
identifier to jump to (e.g. `BatchOperation` or `apply`), or null for a whole-file step. \
`why` explains why it comes at this point; `check` says concretely what to verify there. \
Run `gh pr diff` to read the actual changes before deciding.";

const STORY_INSTRUCTIONS: &str = "\
Retell this pull request as an ordered series of small, atomic \"virtual commits\", the way a \
careful engineer would have committed it so that reading the commits in order tells the story \
of the change. You cannot change any code: you only group and order the existing hunks listed \
below. Every hunk id must appear in exactly one commit; never invent ids. \
Order the commits so each one builds on the previous ones: foundations first (types, data \
model, interfaces, enabling refactors), then the core behavior, then integration and wiring, \
then tests and docs (a test may go with the commit it tests if that reads better). Keep \
commits small and focused, typically 1 to 8 hunks; a large new file can be its own commit. \
`title`: an imperative commit subject, at most 72 characters. \
`why`: 2 to 4 sentences on WHY this step exists and what it enables for the reviewer, not a \
restatement of the diff. `next`: one short sentence on what the following commits build on top \
of it (empty for the last one). `summary`: 2 to 3 sentences on the arc of the whole change. \
Use `gh pr view` for the PR description if it helps.";

type Tx = UnboundedSender<Msg>;

#[derive(Default)]
struct State {
    /// (repo, pr, conversation) -> claude session id, for follow-up questions.
    sessions: HashMap<SessionKey, String>,
    /// request id -> kill switch of the running claude process.
    running: HashMap<u64, oneshot::Sender<()>>,
}

type Shared = Arc<Mutex<State>>;

type SessionKey = (String, u64, Option<u64>);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let port = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1)?.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    // Started by the app: exit when the app goes away (its end of our stdin closes).
    if args.iter().any(|a| a == "--exit-with-parent") {
        tokio::spawn(async {
            let mut sink = tokio::io::sink();
            let _ = tokio::io::copy(&mut tokio::io::stdin(), &mut sink).await;
            std::process::exit(0);
        });
    }

    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("port {port} busy (another prmator-claude running?)"))?;
    eprintln!("prmator-claude listening on ws://127.0.0.1:{port}");

    let state: Shared = Default::default();
    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, state).await {
                eprintln!("connection error: {e:#}");
            }
        });
    }
}

/// Only the app may connect. Browsers send an `Origin` header with every WebSocket
/// handshake, so any web page could otherwise reach us on localhost; the app sends none.
/// With `PRMATOR_BRIDGE_TOKEN` set (the app always sets it), the URL must carry it too.
#[allow(clippy::result_large_err)]
fn check_handshake(
    req: &tokio_tungstenite::tungstenite::handshake::server::Request,
    resp: tokio_tungstenite::tungstenite::handshake::server::Response,
) -> Result<
    tokio_tungstenite::tungstenite::handshake::server::Response,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    let refuse = |why: &str| {
        eprintln!("refused connection: {why}");
        let mut r = tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(Some("forbidden".into()));
        *r.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
        Err(r)
    };
    if req.headers().contains_key("origin") {
        return refuse("from a web page");
    }
    if let Some(expected) = std::env::var("PRMATOR_BRIDGE_TOKEN").ok().filter(|t| !t.is_empty()) {
        let given = req.uri().query().unwrap_or_default().split('&').find_map(|kv| kv.strip_prefix("token="));
        if given != Some(expected.as_str()) {
            return refuse("bad token");
        }
    }
    Ok(resp)
}

async fn handle(stream: TcpStream, state: Shared) -> anyhow::Result<()> {
    let ws = tokio_tungstenite::accept_hdr_async(stream, check_handshake).await?;
    let (mut sink, mut source) = ws.split();

    let (tx, mut rx) = unbounded_channel::<Msg>();
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sink.send(Message::Text(msg.to_json().into())).await.is_err() {
                break;
            }
        }
    });

    let _ = tx.send(Msg::Ready { claude_version: claude_version().await });

    while let Some(frame) = source.next().await {
        let frame = frame?;
        let Ok(text) = frame.to_text() else { continue };
        let Some(msg) = Msg::from_json(text) else {
            eprintln!("bad frame: {text}");
            continue;
        };
        let request = match msg {
            Msg::Ask { id, question, context, repo, pr_number, conversation, history } => {
                let mut prompt = build_prompt(&question, context.as_ref(), &repo, pr_number);
                let key = (repo.clone(), pr_number, conversation);
                if let Some(history) = history.filter(|_| !state.lock().unwrap().sessions.contains_key(&key)) {
                    prompt = format!("Earlier in this conversation:\n\n{history}\n\n---\n\n{prompt}");
                }
                Some(Request { id, repo, pr_number, conversation, prompt, kind: Kind::Answer })
            }
            Msg::Plan { id, repo, pr_number, pr_title, outline } => {
                let prompt = format!(
                    "PR {repo}#{pr_number}: {pr_title}\n\n{PLAN_INSTRUCTIONS}\n\nChanged files and symbols:\n{outline}"
                );
                Some(Request { id, repo, pr_number, conversation: None, prompt, kind: Kind::Plan })
            }
            Msg::Story { id, repo, pr_number, pr_title, hunks } => {
                let prompt =
                    format!("PR {repo}#{pr_number}: {pr_title}\n\n{STORY_INSTRUCTIONS}\n\nHunks:\n\n{hunks}");
                // Its own session slot, so it never replaces the PR's plan/chat session.
                Some(Request { id, repo, pr_number, conversation: Some(u64::MAX), prompt, kind: Kind::Story })
            }
            Msg::Cancel { id } => {
                if let Some(kill) = state.lock().unwrap().running.remove(&id) {
                    let _ = kill.send(());
                }
                None
            }
            Msg::ResetSession { repo, pr_number } => {
                state.lock().unwrap().sessions.retain(|(r, n, _), _| !(r == &repo && *n == pr_number));
                None
            }
            _ => None,
        };
        if let Some(request) = request {
            let (state, tx) = (state.clone(), tx.clone());
            tokio::spawn(async move {
                let id = request.id;
                if let Err(e) = run_claude(&state, &tx, request).await {
                    let _ = tx.send(Msg::Error { id: Some(id), text: format!("{e:#}") });
                    let _ = tx.send(Msg::Done { id, cost_usd: None });
                }
                state.lock().unwrap().running.remove(&id);
            });
        }
    }
    Ok(())
}

struct Request {
    id: u64,
    repo: String,
    pr_number: u64,
    conversation: Option<u64>,
    prompt: String,
    kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Free-form answer, streamed.
    Answer,
    /// Structured review plan.
    Plan,
    /// Structured virtual commits; runs in a fresh session.
    Story,
}

/// CLI version plus how it is signed in, e.g. "2.1.303 (Claude Code) · claude.ai login".
async fn claude_version() -> String {
    let out = cli_command().arg("--version").output().await;
    let version = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return "claude CLI not found".into(),
    };
    let auth = cli_command().args(["auth", "status"]).output().await.ok().and_then(|o| {
        let status: Value = serde_json::from_slice(&o.stdout).ok()?;
        Some(match status["authMethod"].as_str()? {
            "claude.ai" => "claude.ai login".to_string(),
            other => format!("{other} auth"),
        })
    });
    match auth {
        Some(auth) => format!("{version} · {auth}"),
        None => version,
    }
}

/// The `claude` CLI, set up to use the user's logged-in Claude account. API-key
/// variables are removed so a key in the environment can't switch it to pay-per-token
/// API billing (set `PRMATOR_ALLOW_API_KEY=1` to keep them).
fn cli_command() -> Command {
    let mut cmd = Command::new(claude_bin());
    if std::env::var_os("PRMATOR_ALLOW_API_KEY").is_none() {
        cmd.env_remove("ANTHROPIC_API_KEY").env_remove("ANTHROPIC_AUTH_TOKEN");
    }
    cmd
}

fn claude_bin() -> String {
    std::env::var("PRMATOR_CLAUDE_BIN").unwrap_or("claude".into())
}

/// A local clone of `owner/name` under PRMATOR_CODE_DIR, if there is one.
fn local_checkout(repo: &str) -> Option<PathBuf> {
    let base = PathBuf::from(std::env::var("PRMATOR_CODE_DIR").ok()?);
    let name = repo.rsplit('/').next()?;
    // The repo name comes over the socket: keep it a plain directory name.
    if name.is_empty() || name.starts_with('.') || name.contains('\\') {
        return None;
    }
    let dir = base.join(name);
    dir.join(".git").exists().then_some(dir)
}

fn build_prompt(question: &str, context: Option<&ChangeContext>, repo: &str, pr: u64) -> String {
    match context {
        Some(c) => format!(
            "PR {repo}#{pr}: {title}\nFile: {file} ({location})\n\n```diff\n{snippet}\n```\n\n{question}",
            title = c.pr_title,
            file = c.file,
            location = c.location,
            snippet = c.snippet.trim_end(),
        ),
        None => format!("PR {repo}#{pr}\n\n{question}"),
    }
}

async fn run_claude(state: &Shared, tx: &Tx, req: Request) -> anyhow::Result<()> {
    let id = req.id;
    let key = (req.repo.clone(), req.pr_number, req.conversation);
    let session = state.lock().unwrap().sessions.get(&key).cloned();
    let checkout = local_checkout(&req.repo);

    let mut allowed = vec!["Bash(gh pr view:*)", "Bash(gh pr diff:*)"];
    if checkout.is_some() {
        allowed.extend(["Read", "Grep", "Glob"]);
    }

    let mut cmd = cli_command();
    cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"])
        .args(["--append-system-prompt", SYSTEM_PROMPT])
        .arg("--allowedTools")
        .args(&allowed);
    match req.kind {
        Kind::Plan => {
            cmd.args(["--json-schema", &ReviewPlan::json_schema().to_string()]);
        }
        Kind::Story => {
            cmd.args(["--json-schema", &Story::json_schema().to_string()]);
        }
        Kind::Answer => {}
    }
    if let Some(sid) = session.as_ref().filter(|_| req.kind != Kind::Story) {
        cmd.args(["--resume", sid]);
    }
    if let Ok(extra) = std::env::var("PRMATOR_CLAUDE_ARGS") {
        cmd.args(extra.split_whitespace());
    }
    cmd.current_dir(checkout.unwrap_or_else(std::env::temp_dir))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().context("failed to start `claude` (is it on PATH?)")?;

    // The prompt goes through stdin so diffs of any size are fine.
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(req.prompt.as_bytes()).await?;
    drop(stdin);

    let (kill_tx, mut kill_rx) = oneshot::channel();
    state.lock().unwrap().running.insert(id, kill_tx);

    let stderr = child.stderr.take().unwrap();
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            buf.push_str(&line);
            buf.push('\n');
        }
        buf
    });

    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut streamed_text = false;
    let mut finished = false;
    loop {
        let line = tokio::select! {
            _ = &mut kill_rx => {
                let _ = child.start_kill();
                let _ = tx.send(Msg::Done { id, cost_usd: None });
                return Ok(());
            }
            line = lines.next_line() => line?,
        };
        let Some(line) = line else { break };
        let Ok(event) = serde_json::from_str::<Value>(&line) else { continue };
        match event["type"].as_str() {
            Some("system") if event["subtype"] == "init" => {
                if let Some(sid) = event["session_id"].as_str() {
                    state.lock().unwrap().sessions.insert(key.clone(), sid.to_string());
                }
            }
            // Plans stream no prose; their tool activity is still shown as progress.
            Some("stream_event") if req.kind == Kind::Answer => {
                let ev = &event["event"];
                if ev["type"] == "content_block_delta" && ev["delta"]["type"] == "text_delta" {
                    if let Some(text) = ev["delta"]["text"].as_str() {
                        streamed_text = true;
                        let _ = tx.send(Msg::Delta { id, text: text.to_string() });
                    }
                }
            }
            Some("assistant") => {
                for block in event["message"]["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        // Only used when partial messages are not available.
                        Some("text") if !streamed_text && req.kind == Kind::Answer => {
                            let text = block["text"].as_str().unwrap_or_default();
                            let _ = tx.send(Msg::Delta { id, text: text.to_string() });
                        }
                        Some("tool_use") if block["name"] != "StructuredOutput" => {
                            let _ = tx.send(Msg::Activity { id, text: describe_tool(block) });
                        }
                        _ => {}
                    }
                }
                // Separate text from consecutive assistant messages.
                if streamed_text {
                    let _ = tx.send(Msg::Delta { id, text: "\n\n".into() });
                }
            }
            Some("result") => {
                if let Some(sid) = event["session_id"].as_str() {
                    state.lock().unwrap().sessions.insert(key.clone(), sid.to_string());
                }
                if event["is_error"].as_bool().unwrap_or(false) {
                    let text = event["result"].as_str().unwrap_or("claude reported an error");
                    let _ = tx.send(Msg::Error { id: Some(id), text: text.to_string() });
                } else if req.kind == Kind::Plan {
                    match parse_structured(&event) {
                        Some(plan) => {
                            let _ = tx.send(Msg::PlanReady { id, plan });
                        }
                        None => {
                            let text = "Claude did not return a valid review plan.".to_string();
                            let _ = tx.send(Msg::Error { id: Some(id), text });
                        }
                    }
                } else if req.kind == Kind::Story {
                    match parse_structured(&event) {
                        Some(story) => {
                            let _ = tx.send(Msg::StoryReady { id, story });
                        }
                        None => {
                            let text = "Claude did not return a valid story.".to_string();
                            let _ = tx.send(Msg::Error { id: Some(id), text });
                        }
                    }
                }
                let _ = tx.send(Msg::Done { id, cost_usd: event["total_cost_usd"].as_f64() });
                finished = true;
            }
            _ => {}
        }
    }

    let status = child.wait().await?;
    if !finished {
        let stderr = stderr_task.await.unwrap_or_default();
        // A stale session id makes --resume fail; drop it so the next try starts fresh.
        state.lock().unwrap().sessions.remove(&key);
        anyhow::bail!("claude exited with {status}: {}", stderr.trim());
    }
    Ok(())
}

/// Structured output from a `result` event: `structured_output`, or JSON in the text result.
fn parse_structured<T: serde::de::DeserializeOwned>(result: &Value) -> Option<T> {
    if let Ok(value) = serde_json::from_value(result["structured_output"].clone()) {
        return Some(value);
    }
    let text = result["result"].as_str()?;
    let json = &text[text.find('{')?..=text.rfind('}')?];
    serde_json::from_str(json).ok()
}

fn describe_tool(block: &Value) -> String {
    let name = block["name"].as_str().unwrap_or("tool");
    let input = &block["input"];
    let detail = input["command"]
        .as_str()
        .or(input["file_path"].as_str())
        .or(input["pattern"].as_str())
        .unwrap_or_default();
    let detail: String = detail.chars().take(100).collect();
    format!("{name} {detail}").trim().to_string()
}
