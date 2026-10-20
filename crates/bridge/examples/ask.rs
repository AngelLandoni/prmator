//! Smoke test for a running bridge:
//!   cargo run -p prmator-claude --example ask -- owner/repo 123 "What does this PR do?"
//!   cargo run -p prmator-claude --example ask -- owner/repo 123 --plan < outline.txt
//!   cargo run -p prmator-claude --example ask -- owner/repo 123 --story < hunks.txt

use futures_util::{SinkExt, StreamExt};
use protocol::{Msg, DEFAULT_PORT};
use std::io::Write;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [repo, pr, question] = &args[..] else {
        anyhow::bail!("usage: ask <owner/repo> <pr number> <question>");
    };
    let port = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT);
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}")).await?;
    let (mut sink, mut source) = ws.split();
    let ask = if question == "--plan" {
        let mut outline = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut outline)?;
        Msg::Plan { id: 1, repo: repo.clone(), pr_number: pr.parse()?, pr_title: String::new(), outline }
    } else if question == "--story" {
        let mut hunks = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut hunks)?;
        Msg::Story { id: 1, repo: repo.clone(), pr_number: pr.parse()?, pr_title: String::new(), hunks }
    } else {
        Msg::Ask {
            id: 1,
            question: question.clone(),
            context: None,
            repo: repo.clone(),
            pr_number: pr.parse()?,
            conversation: None,
            history: None,
        }
    };
    sink.send(Message::Text(ask.to_json().into())).await?;
    while let Some(frame) = source.next().await {
        let Some(msg) = frame?.to_text().ok().and_then(Msg::from_json) else { continue };
        match msg {
            Msg::Ready { claude_version } => eprintln!("[ready: {claude_version}]"),
            Msg::Delta { text, .. } => {
                print!("{text}");
                std::io::stdout().flush()?;
            }
            Msg::Activity { text, .. } => eprintln!("\n[tool: {text}]"),
            Msg::Done { cost_usd, .. } => {
                let _ = cost_usd; // list-price estimate from the CLI, not a charge
                eprintln!("\n[done]");
                break;
            }
            Msg::Error { text, .. } => eprintln!("\n[error: {text}]"),
            Msg::PlanReady { plan, .. } => println!("{}", serde_json::to_string_pretty(&plan)?),
            Msg::StoryReady { story, .. } => println!("{}", serde_json::to_string_pretty(&story)?),
            _ => {}
        }
    }
    Ok(())
}
