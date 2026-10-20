//! Claude conversations: a floating chat opens next to the code (right-click → Ask Claude,
//! `a`, hover cards…); when the reviewer is done, the conversation is filed into the
//! review log on the right, where it can be expanded, continued, or deleted later.
//! Each conversation is its own Claude session, and the log is saved per PR.

use gpui::{point, px, Context, Focusable, Pixels, Point, ScrollHandle, Window};
use protocol::{ChangeContext, Msg};

use crate::app::Prmator;
use crate::model::PrKey;
use crate::store::{SavedConversation, SavedTurn};

#[derive(Clone, Debug, PartialEq)]
pub enum TurnStatus {
    Running,
    Done,
    Failed(String),
}

pub struct Turn {
    /// Bridge request id.
    pub request: u64,
    pub question: String,
    pub answer: String,
    pub activity: Vec<String>,
    pub status: TurnStatus,
}

pub struct Conversation {
    pub id: u64,
    pub context: Option<ChangeContext>,
    pub turns: Vec<Turn>,
    /// Unix seconds.
    pub created: u64,
    /// Filed into the review log (finished at least once).
    pub in_log: bool,
    /// Expanded in the review log.
    pub expanded: bool,
    /// Loaded from disk: Claude has no session for it yet, so the next question replays
    /// the transcript.
    pub restored: bool,
}

impl Conversation {
    pub fn is_running(&self) -> bool {
        self.turns.iter().any(|t| t.status == TurnStatus::Running)
    }

    pub fn title(&self) -> &str {
        self.turns.first().map_or("(no question yet)", |t| t.question.as_str())
    }

    /// Plain-text transcript (copying, and replaying into a new session).
    pub fn transcript(&self) -> String {
        let mut out = String::new();
        if let Some(c) = &self.context {
            out.push_str(&format!("Context: {} ({})\n```diff\n{}\n```\n\n", c.file, c.location, c.snippet.trim_end()));
        }
        for t in &self.turns {
            out.push_str(&format!("Q: {}\n\nA: {}\n\n", t.question, t.answer.trim()));
        }
        out.trim_end().to_string()
    }

    /// First line number mentioned in the context's location ("L12–L20", "-10,6 +10,9").
    pub fn anchor_line(&self) -> Option<u32> {
        let loc = &self.context.as_ref()?.location;
        let start = loc.find(['+', 'L']).map(|i| i + 1).unwrap_or(0);
        let digits: String = loc[start..].chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    }
}

/// The open floating chat.
pub struct ChatPopover {
    pub conversation: u64,
    pub position: Point<Pixels>,
    pub scroll: ScrollHandle,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Unique across restarts (saved conversations keep their ids).
fn new_conversation_id() -> u64 {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let prev = LAST.fetch_max(ms, std::sync::atomic::Ordering::Relaxed);
    if prev >= ms {
        LAST.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
    } else {
        ms
    }
}

pub fn ago(secs: u64) -> String {
    let d = now_secs().saturating_sub(secs);
    match d {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m ago", d / 60),
        3600..=86399 => format!("{}h ago", d / 3600),
        _ => format!("{}d ago", d / 86400),
    }
}

impl Prmator {
    fn conversations_of(&mut self, key: &PrKey) -> &mut Vec<Conversation> {
        self.conversations.entry(key.clone()).or_default()
    }

    pub fn conversation(&self, id: u64) -> Option<&Conversation> {
        let key = &self.pr.as_ref()?.key;
        self.conversations.get(key)?.iter().find(|c| c.id == id)
    }

    fn conversation_mut(&mut self, id: u64) -> Option<&mut Conversation> {
        let key = self.pr.as_ref()?.key.clone();
        self.conversations.get_mut(&key)?.iter_mut().find(|c| c.id == id)
    }

    /// The turn a bridge request belongs to (in any PR, so answers keep streaming in after
    /// switching PRs).
    pub fn turn_for_request(&mut self, request: u64) -> Option<&mut Turn> {
        let (key, conv) = self.requests.get(&request)?.clone();
        let conv = self.conversations.get_mut(&key)?.iter_mut().find(|c| c.id == conv)?;
        conv.turns.iter_mut().find(|t| t.request == request)
    }

    /// Opens a floating chat at `position`, optionally asking `question` right away.
    /// Any chat already open is filed into the log first.
    pub fn open_chat(
        &mut self,
        context: Option<ChangeContext>,
        position: Point<Pixels>,
        question: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_chat(window, cx);
        let Some(pv) = &self.pr else { return };
        let key = pv.key.clone();
        let id = new_conversation_id();
        self.conversations_of(&key).push(Conversation {
            id,
            context,
            turns: vec![],
            created: now_secs(),
            in_log: false,
            expanded: false,
            restored: false,
        });
        self.show_chat(id, position, window, cx);
        if let Some(q) = question {
            self.send_turn(id, q, cx);
        }
    }

    /// Reopens a logged conversation in the floating chat.
    pub fn continue_conversation(&mut self, id: u64, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat.as_ref().is_some_and(|c| c.conversation == id) {
            return;
        }
        self.finish_chat(window, cx);
        self.show_chat(id, position, window, cx);
    }

    fn show_chat(&mut self, id: u64, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.hover = None;
        let scroll = ScrollHandle::new();
        scroll.scroll_to_bottom();
        self.chat = Some(ChatPopover { conversation: id, position, scroll });
        let placeholder = match self.conversation(id).map(|c| c.turns.is_empty()) {
            Some(true) => "Ask Claude about this…",
            _ => "Follow up…",
        };
        self.input.update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        window.focus(&self.input.focus_handle(cx));
        cx.notify();
    }

    /// Where a chat about the whole PR opens: top center of the window.
    pub fn default_chat_position(&self, window: &Window) -> Point<Pixels> {
        let size = window.viewport_size();
        point((size.width - px(CHAT_WIDTH)) / 2., px(110.))
    }

    pub fn send_turn(&mut self, id: u64, question: String, cx: &mut Context<Self>) {
        let request = self.next_request_id();
        let connected = self.connected;
        let Some(pv) = &self.pr else { return };
        let key = pv.key.clone();
        let Some(conv) = self.conversation_mut(id) else { return };
        // Context goes with the first question; the session remembers it after that.
        let context = conv.turns.is_empty().then(|| conv.context.clone()).flatten();
        let history = std::mem::take(&mut conv.restored).then(|| conv.transcript()).filter(|h| !h.is_empty());
        conv.turns.push(Turn {
            request,
            question: question.clone(),
            answer: String::new(),
            activity: vec![],
            status: if connected {
                TurnStatus::Running
            } else {
                TurnStatus::Failed("Claude bridge is not connected yet; try again in a moment.".into())
            },
        });
        self.requests.insert(request, (key.clone(), id));
        self.link.send(Msg::Ask {
            id: request,
            question,
            context,
            repo: key.repo.clone(),
            pr_number: key.number,
            conversation: Some(id),
            history,
        });
        if let Some(chat) = &self.chat {
            chat.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    /// Enter in the chat input.
    pub fn submit_chat(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(id) = self.chat.as_ref().map(|c| c.conversation) else { return };
        if !text.trim().is_empty() {
            self.send_turn(id, text, cx);
        }
    }

    /// Closes the floating chat, filing the conversation into the review log (answers
    /// still streaming keep arriving there).
    pub fn finish_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.chat.take() else { return };
        let Some(pv) = &self.pr else { return };
        let key = pv.key.clone();
        let list = self.conversations_of(&key);
        if let Some(ix) = list.iter().position(|c| c.id == chat.conversation) {
            if list[ix].turns.is_empty() {
                list.remove(ix);
            } else {
                list[ix].in_log = true;
            }
        }
        self.save_log(&key);
        window.focus(&self.diff_focus);
        cx.notify();
    }

    /// Closes the chat and throws the conversation away.
    pub fn discard_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.chat.take() else { return };
        self.delete_conversation(chat.conversation, cx);
        window.focus(&self.diff_focus);
    }

    pub fn delete_conversation(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(key) = self.pr.as_ref().map(|pv| pv.key.clone()) else { return };
        let list = self.conversations_of(&key);
        if let Some(ix) = list.iter().position(|c| c.id == id) {
            let conv = list.remove(ix);
            for t in conv.turns.iter().filter(|t| t.status == TurnStatus::Running) {
                self.link.send(Msg::Cancel { id: t.request });
            }
        }
        if self.chat.as_ref().is_some_and(|c| c.conversation == id) {
            self.chat = None;
        }
        self.save_log(&key);
        cx.notify();
    }

    pub fn stop_turn(&mut self, request: u64, cx: &mut Context<Self>) {
        self.link.send(Msg::Cancel { id: request });
        if let Some(t) = self.turn_for_request(request) {
            t.status = TurnStatus::Done;
        }
        cx.notify();
    }

    pub fn toggle_log_entry(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(c) = self.conversation_mut(id) {
            c.expanded = !c.expanded;
        }
        cx.notify();
    }

    /// Scrolls the diff to the code a conversation is about.
    pub fn jump_to_conversation(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(conv) = self.conversation(id) else { return };
        let Some(file) = conv.context.as_ref().map(|c| c.file.clone()) else { return };
        let line = conv.anchor_line();
        let Some(pv) = &mut self.pr else { return };
        let Some(fi) = pv.file_index(&file) else { return };
        match line {
            Some(n) => {
                pv.jump_to_lines(fi, true, n..n + 1);
            }
            None => pv.jump_to_file(fi),
        }
        cx.notify();
    }

    /// Writes the PR's logged conversations to disk.
    pub fn save_log(&mut self, key: &PrKey) {
        let saved: Vec<SavedConversation> = self
            .conversations
            .get(key)
            .into_iter()
            .flatten()
            .filter(|c| c.in_log)
            .map(|c| SavedConversation {
                id: c.id,
                context: c.context.clone(),
                created: c.created,
                turns: c
                    .turns
                    .iter()
                    .map(|t| SavedTurn { question: t.question.clone(), answer: t.answer.clone() })
                    .collect(),
            })
            .collect();
        self.store.progress(&key.store_key()).log = saved;
        self.store.save();
    }

    /// Loads a PR's saved review log (once per session).
    pub fn load_log(&mut self, key: &PrKey) {
        if self.conversations.contains_key(key) {
            return;
        }
        let saved = self.store.progress(&key.store_key()).log.clone();
        let conversations = saved
            .into_iter()
            .map(|s| Conversation {
                id: s.id,
                context: s.context,
                created: s.created,
                turns: s
                    .turns
                    .into_iter()
                    .map(|t| Turn {
                        request: 0,
                        question: t.question,
                        answer: t.answer,
                        activity: vec![],
                        status: TurnStatus::Done,
                    })
                    .collect(),
                in_log: true,
                expanded: false,
                restored: true,
            })
            .collect();
        self.conversations.insert(key.clone(), conversations);
    }

    /// Streaming updates from the bridge. Returns false when `request` isn't a chat turn.
    pub fn on_chat_msg(&mut self, msg: &Msg) -> bool {
        let request = match msg {
            Msg::Delta { id, .. } | Msg::Activity { id, .. } | Msg::Done { id, .. } => *id,
            Msg::Error { id: Some(id), .. } => *id,
            _ => return false,
        };
        let conversation = self.requests.get(&request).map(|(_, c)| *c);
        let Some(turn) = self.turn_for_request(request) else { return false };
        match msg {
            Msg::Delta { text, .. } => turn.answer.push_str(text),
            Msg::Activity { text, .. } => turn.activity.push(text.clone()),
            Msg::Done { .. } => {
                if turn.status == TurnStatus::Running {
                    turn.status = TurnStatus::Done;
                }
            }
            Msg::Error { text, .. } => turn.status = TurnStatus::Failed(text.clone()),
            _ => {}
        }
        let finished = matches!(msg, Msg::Done { .. } | Msg::Error { .. });
        if let Some(chat) = self.chat.as_ref().filter(|c| Some(c.conversation) == conversation) {
            chat.scroll.scroll_to_bottom();
        }
        // A logged conversation finished a turn: persist the answer.
        if finished {
            if let Some((key, _)) = self.requests.get(&request).cloned() {
                let logged = self.conversations.get(&key).into_iter().flatten().any(|c| Some(c.id) == conversation && c.in_log);
                if logged {
                    self.save_log(&key);
                }
            }
        }
        true
    }

    pub fn fail_running_turns(&mut self, reason: &str) {
        for turn in self.conversations.values_mut().flatten().flat_map(|c| &mut c.turns) {
            if turn.status == TurnStatus::Running {
                turn.status = TurnStatus::Failed(reason.into());
            }
        }
    }
}

pub const CHAT_WIDTH: f32 = 560.;

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(location: &str) -> Conversation {
        Conversation {
            id: 1,
            context: Some(ChangeContext {
                repo: "o/r".into(),
                pr_number: 1,
                pr_title: String::new(),
                file: "a.rs".into(),
                location: location.into(),
                snippet: "+x".into(),
            }),
            turns: vec![],
            created: 0,
            in_log: true,
            expanded: false,
            restored: false,
        }
    }

    #[test]
    fn anchors() {
        assert_eq!(conv("L12–L20").anchor_line(), Some(12));
        assert_eq!(conv("L7").anchor_line(), Some(7));
        assert_eq!(conv("-10,6 +14,9").anchor_line(), Some(14));
        assert_eq!(conv("whole file").anchor_line(), None);
    }

    #[test]
    fn ids_are_unique() {
        let a = new_conversation_id();
        let b = new_conversation_id();
        assert_ne!(a, b);
    }
}
