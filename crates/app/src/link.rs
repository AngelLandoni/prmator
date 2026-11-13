//! WebSocket client to the `prmator-claude` bridge. Runs on its own thread with a tokio
//! runtime and (re)connects forever; starts the bridge if nothing is listening.

use std::process::{Child, Stdio};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::{Msg, DEFAULT_PORT};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio_tungstenite::tungstenite::Message;

pub enum LinkEvent {
    Connected,
    Disconnected,
    Msg(Msg),
}

#[derive(Clone)]
pub struct Link(tokio::sync::mpsc::UnboundedSender<Msg>);

impl Link {
    pub fn send(&self, msg: Msg) {
        let _ = self.0.send(msg);
    }
}

pub fn start() -> (Link, async_channel::Receiver<LinkEvent>) {
    let (out_tx, out_rx) = unbounded_channel();
    let (ev_tx, ev_rx) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("claude-link".into())
        .spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime")
                .block_on(run(out_rx, ev_tx));
        })
        .expect("spawn link thread");
    (Link(out_tx), ev_rx)
}

async fn run(mut out_rx: UnboundedReceiver<Msg>, events: async_channel::Sender<LinkEvent>) {
    let explicit = std::env::var("PRMATOR_BRIDGE_URL").ok();
    let base = explicit.clone().unwrap_or_else(|| format!("ws://127.0.0.1:{DEFAULT_PORT}"));
    let url = format!("{}/?token={}", base.trim_end_matches('/'), bridge_token());
    // Kept alive for the app's lifetime: the bridge exits when this pipe closes.
    let mut bridge: Option<Child> = None;

    loop {
        let ws = match tokio_tungstenite::connect_async(&url).await {
            Ok((ws, _)) => ws,
            Err(_) => {
                let exited = bridge.as_mut().is_some_and(|c| c.try_wait().ok().flatten().is_some());
                if explicit.is_none() && (bridge.is_none() || exited) {
                    bridge = spawn_bridge();
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        let (mut sink, mut source) = ws.split();
        let _ = events.send(LinkEvent::Connected).await;

        loop {
            tokio::select! {
                Some(msg) = out_rx.recv() => {
                    if sink.send(Message::Text(msg.to_json().into())).await.is_err() {
                        break;
                    }
                }
                frame = source.next() => {
                    let Some(Ok(frame)) = frame else { break };
                    if let Some(msg) = frame.to_text().ok().and_then(Msg::from_json) {
                        let _ = events.send(LinkEvent::Msg(msg)).await;
                    }
                }
            }
        }
        let _ = events.send(LinkEvent::Disconnected).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The secret the bridge asks of every connection, so other local programs (or web pages
/// opening a WebSocket to localhost) can't run Claude through it. Random per app launch;
/// set `PRMATOR_BRIDGE_TOKEN` to share one with a bridge started by hand.
fn bridge_token() -> &'static str {
    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TOKEN.get_or_init(|| {
        std::env::var("PRMATOR_BRIDGE_TOKEN").ok().filter(|t| !t.is_empty()).unwrap_or_else(|| {
            use std::io::Read;
            let mut bytes = [0u8; 24];
            std::fs::File::open("/dev/urandom")
                .and_then(|mut f| f.read_exact(&mut bytes))
                .expect("read /dev/urandom");
            bytes.iter().map(|b| format!("{b:02x}")).collect()
        })
    })
}

/// Starts `prmator-claude` from next to our own executable, tied to our lifetime.
fn spawn_bridge() -> Option<Child> {
    let exe = std::env::current_exe().ok()?.parent()?.join("prmator-claude");
    if !exe.exists() {
        eprintln!("prmator-claude not found next to the app; build it with `cargo build`");
        return None;
    }
    match std::process::Command::new(&exe)
        .arg("--exit-with-parent")
        .env("PRMATOR_BRIDGE_TOKEN", bridge_token())
        .stdin(Stdio::piped())
        .spawn()
    {
        Ok(child) => {
            eprintln!("started {}", exe.display());
            Some(child)
        }
        Err(e) => {
            eprintln!("failed to start {}: {e}", exe.display());
            None
        }
    }
}
