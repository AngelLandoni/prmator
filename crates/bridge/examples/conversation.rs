//! Checks per-conversation sessions against a running bridge:
//!   cargo run -p prmator-claude --example conversation -- owner/repo 123

use futures_util::{SinkExt, StreamExt};
use protocol::{Msg, DEFAULT_PORT};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (repo, pr): (String, u64) = (args[0].clone(), args[1].parse()?);
    let port = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT);
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}")).await?;
    let (mut sink, mut source) = ws.split();
    let mut next = 1;
    let mut ask = async |q: &str, conversation: u64, history: Option<&str>| -> anyhow::Result<String> {
        next += 1;
        let msg = Msg::Ask {
            id: next,
            question: q.into(),
            context: None,
            repo: repo.clone(),
            pr_number: pr,
            conversation: Some(conversation),
            history: history.map(String::from),
        };
        sink.send(Message::Text(msg.to_json().into())).await?;
        let mut answer = String::new();
        while let Some(frame) = source.next().await {
            match frame?.to_text().ok().and_then(Msg::from_json) {
                Some(Msg::Delta { id, text }) if id == next => answer.push_str(&text),
                Some(Msg::Error { id: Some(id), text }) if id == next => anyhow::bail!(text),
                Some(Msg::Done { id, .. }) if id == next => break,
                _ => {}
            }
        }
        Ok(answer.trim().to_string())
    };
    let no_tools = " Do not run any tools. Answer with one word only.";
    println!("A1: {}", ask(&format!("Remember the code word PELICAN.{no_tools} Reply OK."), 100, None).await?);
    println!("A2: {}", ask(&format!("What code word did I give you?{no_tools}"), 100, None).await?);
    println!("B1 (new conversation): {}", ask(&format!("What code word did I give you? Say NONE if none.{no_tools}"), 200, None).await?);
    let history = "Q: Remember the code word OTTER.\n\nA: OK";
    println!("C1 (restored): {}", ask(&format!("What code word did I give you?{no_tools}"), 300, Some(history)).await?);
    Ok(())
}
