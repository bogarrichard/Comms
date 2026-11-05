//! Test doubles shared by the workspace. [`FakeProvider`] is also what the
//! TUI's `--demo` mode runs against, so the UI can be exercised without any
//! credentials.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use comms_domain::{
    ContentBlock, ConversationKind, PageCursor, Participant, ProviderKind, RemoteId,
};
use comms_provider_api::{
    Capabilities, ConversationClass, MessagingProvider, OutgoingMessage, Page, PageRequest,
    ProviderError, RemoteConversation, RemoteMessage,
};

/// How the next `send_message` call should behave.
#[derive(Debug, Clone)]
pub enum SendScript {
    /// Fail without applying the message.
    Fail(ProviderError),
    /// The server stores the message, but the caller sees an error
    /// (e.g. response lost to a timeout).
    AppliedThenFail(ProviderError),
}

#[derive(Default)]
struct State {
    chats: Vec<(RemoteConversation, Vec<RemoteMessage>)>,
    channels: Vec<(RemoteConversation, Vec<RemoteMessage>)>,
    send_scripts: VecDeque<SendScript>,
    next_id: u64,
}

pub struct FakeProvider {
    kind: &'static str,
    me: Participant,
    state: Mutex<State>,
    latency: Duration,
    pub send_calls: AtomicUsize,
    pub list_calls: AtomicUsize,
    pub message_calls: AtomicUsize,
}

pub fn person(id: &str, name: &str) -> Participant {
    Participant {
        remote_id: RemoteId::new(id),
        display_name: name.to_owned(),
    }
}

pub fn text_message(
    id: &str,
    sender: &Participant,
    at: DateTime<Utc>,
    text: &str,
) -> RemoteMessage {
    RemoteMessage {
        remote_id: RemoteId::new(id),
        sender: sender.clone(),
        timestamp: at,
        content: vec![ContentBlock::paragraph(text)],
        attachments: vec![],
        edited_at: None,
        deleted_at: None,
        reply_to: None,
        is_system: false,
    }
}

pub fn conversation(
    id: &str,
    kind: ConversationKind,
    title: &str,
    group: Option<&str>,
) -> RemoteConversation {
    RemoteConversation {
        remote_id: RemoteId::new(id),
        kind,
        title: title.to_owned(),
        group: group.map(str::to_owned),
        last_activity: None,
        last_read: None,
        preview: None,
        last_message_from_me: false,
    }
}

impl FakeProvider {
    pub fn empty() -> Self {
        Self {
            me: person("me", "Alex Demo"),
            state: Mutex::new(State::default()),
            kind: "fake",
            latency: Duration::ZERO,
            send_calls: AtomicUsize::new(0),
            list_calls: AtomicUsize::new(0),
            message_calls: AtomicUsize::new(0),
        }
    }

    /// Pretend to be another backend (the UI colours conversations per backend).
    pub fn with_kind(mut self, kind: &'static str) -> Self {
        self.kind = kind;
        self
    }

    pub fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    pub fn me(&self) -> Participant {
        self.me.clone()
    }

    /// Add a conversation. `messages` may be given in any order.
    pub fn add_conversation(&self, mut conv: RemoteConversation, mut messages: Vec<RemoteMessage>) {
        messages.sort_by_key(|m| std::cmp::Reverse(m.timestamp));
        if let Some(last) = messages.first() {
            conv.last_activity.get_or_insert(last.timestamp);
            if conv.preview.is_none() {
                conv.preview = Some(comms_domain::blocks_to_plain_text(&last.content));
            }
            conv.last_message_from_me = last.sender.remote_id == self.me.remote_id;
        }
        let mut st = self.state.lock().expect("lock");
        if conv.kind == ConversationKind::Channel {
            st.channels.push((conv, messages));
        } else {
            st.chats.push((conv, messages));
        }
    }

    /// Queue behaviour for upcoming `send_message` calls (FIFO).
    pub fn script_send(&self, script: SendScript) {
        self.state
            .lock()
            .expect("lock")
            .send_scripts
            .push_back(script);
    }

    /// Messages the "server" currently holds for a conversation, newest first.
    pub fn server_messages(&self, conv: &str) -> Vec<RemoteMessage> {
        let st = self.state.lock().expect("lock");
        st.chats
            .iter()
            .chain(st.channels.iter())
            .find(|(c, _)| c.remote_id.as_str() == conv)
            .map(|(_, m)| m.clone())
            .unwrap_or_default()
    }

    /// Simulate someone else writing in a conversation.
    pub fn inject_incoming(&self, conv: &str, from: &Participant, text: &str) {
        let mut st = self.state.lock().expect("lock");
        st.next_id += 1;
        let id = format!("srv-{}", st.next_id);
        let msg = text_message(&id, from, Utc::now(), text);
        let State {
            chats, channels, ..
        } = &mut *st;
        for (c, msgs) in chats.iter_mut().chain(channels.iter_mut()) {
            if c.remote_id.as_str() == conv {
                c.last_activity = Some(msg.timestamp);
                c.preview = Some(text.to_owned());
                c.last_message_from_me = false;
                msgs.insert(0, msg.clone());
            }
        }
    }

    /// A ready-made data set for `--demo`.
    pub fn demo() -> Self {
        let p = Self::empty();
        let now = Utc::now();
        let ago = |mins: i64| now - ChronoDuration::minutes(mins);
        let me = p.me();
        let anna = person("u-anna", "Anna Kovács");
        let bela = person("u-bela", "Béla Szabó");
        let prof = person("u-prof", "Dr. Nagy Péter");

        let mut c = conversation("chat-anna", ConversationKind::Direct, "Anna Kovács", None);
        c.last_read = Some(ago(300));
        p.add_conversation(
            c,
            vec![
                text_message("a1", &anna, ago(190), "Hi! Did you see the new assignment?"),
                text_message("a2", &me, ago(185), "Not yet, what is it about?"),
                text_message(
                    "a3",
                    &anna,
                    ago(180),
                    "Rust TUI messaging client. Sounds fun, right?",
                ),
                text_message("a4", &anna, ago(5), "Want to team up for the project?"),
            ],
        );

        let c = conversation(
            "chat-grp",
            ConversationKind::Group,
            "Study group – Networks",
            None,
        );
        p.add_conversation(
            c,
            vec![
                text_message("g1", &bela, ago(2000), "Lecture notes are uploaded"),
                text_message("g2", &anna, ago(1990), "Thanks!"),
                text_message("g3", &me, ago(1980), "Exam is on the 14th, right?"),
                text_message("g4", &bela, ago(1970), "Yes, 10:00 in room IB.028"),
            ],
        );

        let c = conversation(
            "chat-prof",
            ConversationKind::Direct,
            "Dr. Nagy Péter",
            None,
        );
        p.add_conversation(
            c,
            vec![text_message(
                "p1",
                &prof,
                ago(4000),
                "Your submission was received. Grades will be published next week.",
            )],
        );

        for (id, name) in [
            ("ch-gen", "General"),
            ("ch-ann", "Announcements"),
            ("ch-lab", "Lab"),
        ] {
            let conv = conversation(
                id,
                ConversationKind::Channel,
                name,
                Some("Software Engineering"),
            );
            let msgs = vec![
                text_message(
                    &format!("{id}-1"),
                    &prof,
                    ago(5000),
                    &format!("Welcome to #{name}!"),
                ),
                text_message(
                    &format!("{id}-2"),
                    &anna,
                    ago(900),
                    "Is the deadline still Friday?",
                ),
            ];
            p.add_conversation(conv, msgs);
        }
        let conv = conversation(
            "ch-net",
            ConversationKind::Channel,
            "General",
            Some("Computer Networks"),
        );
        p.add_conversation(
            conv,
            vec![text_message(
                "net-1",
                &bela,
                ago(3000),
                "Slides for week 7 are in Files.",
            )],
        );
        p
    }

    async fn pause(&self) {
        if !self.latency.is_zero() {
            tokio::time::sleep(self.latency).await;
        }
    }

    fn page<T: Clone>(items: &[T], req: &PageRequest) -> Page<T> {
        let start: usize = req
            .cursor
            .as_ref()
            .and_then(|c| c.as_str().parse().ok())
            .unwrap_or(0);
        let limit = if req.limit == 0 {
            50
        } else {
            req.limit as usize
        };
        let end = (start + limit).min(items.len());
        let slice = items.get(start..end).unwrap_or_default().to_vec();
        let next = (end < items.len()).then(|| PageCursor::new(end.to_string()));
        Page { items: slice, next }
    }
}

#[async_trait]
impl MessagingProvider for FakeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::new(self.kind)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            direct_messages: true,
            group_conversations: true,
            channels: true,
            threads: false,
            ..Capabilities::default()
        }
    }

    async fn current_user(&self) -> Result<Participant, ProviderError> {
        Ok(self.me.clone())
    }

    async fn list_conversations(
        &self,
        class: ConversationClass,
        page: PageRequest,
    ) -> Result<Page<RemoteConversation>, ProviderError> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        self.pause().await;
        let st = self.state.lock().expect("lock");
        let source = match class {
            ConversationClass::Chats => &st.chats,
            ConversationClass::Channels => &st.channels,
        };
        let mut convs: Vec<_> = source.iter().map(|(c, _)| c.clone()).collect();
        convs.sort_by_key(|c| std::cmp::Reverse(c.last_activity));
        Ok(Self::page(&convs, &page))
    }

    async fn get_messages(
        &self,
        conversation: &RemoteId,
        page: PageRequest,
    ) -> Result<Page<RemoteMessage>, ProviderError> {
        self.message_calls.fetch_add(1, Ordering::SeqCst);
        self.pause().await;
        let msgs = self.server_messages(conversation.as_str());
        let known = {
            let st = self.state.lock().expect("lock");
            st.chats
                .iter()
                .chain(st.channels.iter())
                .any(|(c, _)| &c.remote_id == conversation)
        };
        if !known {
            return Err(ProviderError::NotFound);
        }
        Ok(Self::page(&msgs, &page))
    }

    async fn send_message(
        &self,
        conversation: &RemoteId,
        message: OutgoingMessage,
    ) -> Result<RemoteMessage, ProviderError> {
        self.send_calls.fetch_add(1, Ordering::SeqCst);
        self.pause().await;
        let script = self.state.lock().expect("lock").send_scripts.pop_front();
        if let Some(SendScript::Fail(e)) = &script {
            return Err(e.clone());
        }
        let msg = {
            let mut st = self.state.lock().expect("lock");
            st.next_id += 1;
            let id = format!("srv-{}", st.next_id);
            let msg = text_message(&id, &self.me, Utc::now(), &message.text);
            let State {
                chats, channels, ..
            } = &mut *st;
            let target = chats
                .iter_mut()
                .chain(channels.iter_mut())
                .find(|(c, _)| &c.remote_id == conversation)
                .ok_or(ProviderError::NotFound)?;
            target.0.last_activity = Some(msg.timestamp);
            target.0.preview = Some(message.text.clone());
            target.0.last_message_from_me = true;
            target.1.insert(0, msg.clone());
            msg
        };
        match script {
            Some(SendScript::AppliedThenFail(e)) => Err(e),
            _ => Ok(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pagination_yields_every_item_exactly_once() {
        let p = FakeProvider::empty();
        let me = p.me();
        let now = Utc::now();
        let msgs: Vec<_> = (0..23)
            .map(|i| text_message(&format!("m{i}"), &me, now + ChronoDuration::seconds(i), "x"))
            .collect();
        p.add_conversation(conversation("c", ConversationKind::Direct, "C", None), msgs);

        let id = RemoteId::new("c");
        let mut seen = vec![];
        let mut req = PageRequest::first(10);
        loop {
            let page = p.get_messages(&id, req.clone()).await.unwrap();
            seen.extend(page.items.iter().map(|m| m.remote_id.as_str().to_owned()));
            match page.next {
                Some(c) => req.cursor = Some(c),
                None => break,
            }
        }
        assert_eq!(seen.len(), 23);
        let mut dedup = seen.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(dedup.len(), 23);
        assert_eq!(seen[0], "m22", "newest first");
    }

    #[tokio::test]
    async fn applied_then_fail_stores_message_but_reports_error() {
        let p = FakeProvider::demo();
        p.script_send(SendScript::AppliedThenFail(ProviderError::Network {
            may_have_reached_server: true,
            message: "timeout".into(),
        }));
        let id = RemoteId::new("chat-anna");
        let before = p.server_messages("chat-anna").len();
        let res = p
            .send_message(&id, OutgoingMessage { text: "hi".into() })
            .await;
        assert!(res.is_err());
        assert_eq!(p.server_messages("chat-anna").len(), before + 1);
    }

    #[tokio::test]
    async fn unknown_conversation_is_not_found() {
        let p = FakeProvider::demo();
        let r = p
            .get_messages(&RemoteId::new("nope"), PageRequest::first(5))
            .await;
        assert_eq!(r.unwrap_err(), ProviderError::NotFound);
    }

    #[tokio::test]
    async fn demo_has_chats_and_channels() {
        let p = FakeProvider::demo();
        let chats = p
            .list_conversations(ConversationClass::Chats, PageRequest::first(50))
            .await;
        let chans = p
            .list_conversations(ConversationClass::Channels, PageRequest::first(50))
            .await;
        assert_eq!(chats.unwrap().items.len(), 3);
        assert_eq!(chans.unwrap().items.len(), 4);
    }
}
