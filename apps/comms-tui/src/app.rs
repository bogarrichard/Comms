//! Frontend state and input handling. Holds no business logic: everything
//! that touches data goes through the `AppHandle` commands and local queries.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use comms_app::{AppEvent, AppHandle, Command, ConnectionStatus};
use comms_domain::{Conversation, ConversationId, ConversationKind, Message, SendState};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::input::InputBuffer;
use crate::keys::{Action, KeyMap};
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Compose,
    Search,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Messages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Chats,
    Channels,
}

impl Filter {
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Chats => "Chats",
            Filter::Channels => "Channels",
        }
    }

    fn next(self) -> Self {
        match self {
            Filter::All => Filter::Chats,
            Filter::Chats => Filter::Channels,
            Filter::Channels => Filter::All,
        }
    }

    fn admits(self, c: &Conversation) -> bool {
        match self {
            Filter::All => true,
            Filter::Chats => c.kind != ConversationKind::Channel,
            Filter::Channels => c.kind == ConversationKind::Channel,
        }
    }
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    until: Instant,
}

pub struct App {
    pub handle: AppHandle,
    pub theme: Theme,
    pub keymap: KeyMap,
    pub convs: Vec<Conversation>,
    pub filter: Filter,
    pub query: String,
    pub selected: usize,
    pub open: Option<Conversation>,
    pub messages: Vec<Message>,
    pub view_limit: u32,
    /// Lines scrolled up from the newest message.
    pub scroll: usize,
    /// Set by the renderer: the largest useful `scroll`.
    pub max_scroll: usize,
    pub mode: Mode,
    pub focus: Focus,
    pub input: InputBuffer,
    drafts: HashMap<ConversationId, InputBuffer>,
    pub connection: ConnectionStatus,
    pub syncing: bool,
    pub toast: Option<Toast>,
    pub channels_note: Option<String>,
    pub has_older: bool,
    loading_older: bool,
    pub quit: bool,
    pub tick: u64,
}

const INITIAL_VIEW: u32 = 300;

impl App {
    pub async fn new(handle: AppHandle, keymap: KeyMap) -> Self {
        let connection = handle.connection_status();
        let theme = Theme::for_provider(handle.provider_kind().as_str());
        let mut app = Self {
            handle,
            theme,
            keymap,
            convs: vec![],
            filter: Filter::All,
            query: String::new(),
            selected: 0,
            open: None,
            messages: vec![],
            view_limit: INITIAL_VIEW,
            scroll: 0,
            max_scroll: 0,
            mode: Mode::Normal,
            focus: Focus::Sidebar,
            input: InputBuffer::default(),
            drafts: HashMap::new(),
            connection,
            syncing: false,
            toast: None,
            channels_note: None,
            has_older: false,
            loading_older: false,
            quit: false,
            tick: 0,
        };
        // Local-first: show whatever is cached before any network answer.
        app.refresh_conversations().await;
        app
    }

    pub fn visible(&self) -> Vec<&Conversation> {
        let q = self.query.to_lowercase();
        self.convs
            .iter()
            .filter(|c| self.filter.admits(c))
            .filter(|c| {
                q.is_empty()
                    || c.title.to_lowercase().contains(&q)
                    || c.group
                        .as_deref()
                        .is_some_and(|g| g.to_lowercase().contains(&q))
            })
            .collect()
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some(Toast {
            text: text.into(),
            error,
            until: Instant::now() + Duration::from_secs(if error { 10 } else { 4 }),
        });
    }

    /// Advance animations. Returns whether the screen needs redrawing.
    pub fn on_tick(&mut self) -> bool {
        self.tick = self.tick.wrapping_add(1);
        let mut redraw = self.syncing;
        if self
            .toast
            .as_ref()
            .is_some_and(|t| Instant::now() >= t.until)
        {
            self.toast = None;
            redraw = true;
        }
        redraw
    }

    // ------------------------------------------------------------ data

    pub async fn refresh_conversations(&mut self) {
        let selected_id = self.visible().get(self.selected).map(|c| c.id);
        match self.handle.conversations().await {
            Ok(list) => self.convs = list,
            Err(e) => return self.toast(format!("Could not read conversations: {e}"), true),
        }
        if let Some(open) = &self.open
            && let Some(fresh) = self.convs.iter().find(|c| c.id == open.id)
        {
            self.open = Some(fresh.clone());
        }
        let vis = self.visible();
        self.selected = selected_id
            .and_then(|id| vis.iter().position(|c| c.id == id))
            .unwrap_or_else(|| self.selected.min(vis.len().saturating_sub(1)));
    }

    pub async fn refresh_messages(&mut self) {
        let Some(open) = &self.open else { return };
        let id = open.id;
        match self.handle.messages(id, self.view_limit).await {
            Ok(m) => self.messages = m,
            Err(e) => self.toast(format!("Could not read messages: {e}"), true),
        }
        self.has_older = self.handle.has_older(id).await.unwrap_or(false);
    }

    async fn open_selected(&mut self) {
        let Some(conv) = self.visible().get(self.selected).map(|c| (*c).clone()) else {
            return;
        };
        if self.open.as_ref().map(|c| c.id) != Some(conv.id) {
            if let Some(prev) = &self.open {
                self.drafts.insert(prev.id, std::mem::take(&mut self.input));
            }
            self.input = self.drafts.remove(&conv.id).unwrap_or_default();
            self.messages.clear();
            self.view_limit = INITIAL_VIEW;
            self.loading_older = false;
        }
        self.scroll = 0;
        self.focus = Focus::Messages;
        self.handle.send(Command::OpenConversation(conv.id));
        self.open = Some(conv);
        self.refresh_messages().await;
    }

    fn request_older(&mut self) {
        let Some(open) = &self.open else { return };
        if self.loading_older {
            return;
        }
        if !self.has_older {
            return self.toast("You have reached the start of this conversation", false);
        }
        self.loading_older = true;
        self.handle.send(Command::LoadOlderMessages(open.id));
        self.toast("Loading older messages…", false);
    }

    pub async fn on_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::ConversationsUpdated => self.refresh_conversations().await,
            AppEvent::MessagesUpdated(id) => {
                if self.open.as_ref().map(|c| c.id) == Some(id) {
                    self.refresh_messages().await;
                }
            }
            AppEvent::MessageSendFailed {
                reason, uncertain, ..
            } => {
                let text = if uncertain {
                    format!(
                        "Message may or may not have been delivered ({reason}). Press R to resend if it does not appear."
                    )
                } else {
                    format!("Message not sent: {reason}. Press R to retry.")
                };
                self.toast(text, true);
            }
            AppEvent::ConnectionStatusChanged(c) => self.connection = c,
            AppEvent::SyncActivity(b) => self.syncing = b,
            AppEvent::ChannelsUnavailable(msg) => {
                self.channels_note = Some(msg.clone());
                self.toast(format!("Channels unavailable: {msg}"), true);
            }
            AppEvent::OlderMessagesLoaded {
                conversation,
                loaded,
                reached_start,
            } => {
                if self.open.as_ref().map(|c| c.id) == Some(conversation) {
                    self.loading_older = false;
                    self.view_limit += 200;
                    self.refresh_messages().await;
                    if reached_start && loaded == 0 {
                        self.toast("You have reached the start of this conversation", false);
                    } else {
                        self.toast(format!("Loaded {loaded} older messages"), false);
                    }
                }
            }
            AppEvent::Notice(n) => self.toast(n, true),
            AppEvent::MessageSendStarted(_)
            | AppEvent::MessageSendSucceeded(_)
            | AppEvent::SyncCompleted { .. } => {}
        }
    }

    // ------------------------------------------------------------ input

    pub async fn on_paste(&mut self, text: &str) {
        if self.mode == Mode::Compose {
            self.input.insert_str(text);
        } else if self.mode == Mode::Search {
            self.query.push_str(&text.replace('\n', " "));
            self.selected = 0;
        }
    }

    pub async fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        match self.mode {
            Mode::Help => self.mode = Mode::Normal,
            Mode::Compose => self.compose_key(key),
            Mode::Search => self.search_key(key).await,
            Mode::Normal => {
                if let Some(action) = self.keymap.lookup(key) {
                    self.action(action).await;
                }
            }
        }
    }

    fn compose_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            // Ctrl+Enter is only reported by terminals with enhanced keyboard
            // support; Enter alone sends everywhere.
            KeyCode::Enter if alt || shift => self.input.newline(),
            KeyCode::Enter => self.send_input(),
            KeyCode::Char('j') if ctrl => self.input.newline(),
            KeyCode::Char('u') if ctrl => self.input.clear(),
            KeyCode::Char('c') if ctrl => self.mode = Mode::Normal,
            KeyCode::Char(c) if !ctrl && !alt => self.input.insert_char(c),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Up => self.input.up(),
            KeyCode::Down => self.input.down(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            _ => {}
        }
    }

    fn send_input(&mut self) {
        let Some(open) = &self.open else { return };
        if self.input.is_blank() {
            return;
        }
        let text = self.input.take();
        self.handle.send(Command::SendMessage {
            conversation: open.id,
            text,
        });
        self.scroll = 0;
    }

    async fn search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.query.clear();
                self.mode = Mode::Normal;
                self.selected = 0;
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                self.open_selected().await;
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
            }
            KeyCode::Down => self.move_selection(1),
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.push(c);
                self.selected = 0;
            }
            _ => {}
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let n = self.visible().len();
        if n == 0 {
            self.selected = 0;
            return;
        }
        self.selected = self.selected.saturating_add_signed(delta).min(n - 1);
    }

    fn scroll_by(&mut self, delta: isize) {
        if delta < 0 && self.scroll >= self.max_scroll && self.max_scroll > 0 {
            // Already at the top: pulling further up asks for older history.
            self.request_older();
        }
        self.scroll = self
            .scroll
            .saturating_add_signed(-delta)
            .min(self.max_scroll);
    }

    async fn action(&mut self, action: Action) {
        let page = 10isize;
        match action {
            Action::Quit => self.quit = true,
            Action::Down if self.focus == Focus::Sidebar => self.move_selection(1),
            Action::Up if self.focus == Focus::Sidebar => self.move_selection(-1),
            Action::PageDown if self.focus == Focus::Sidebar => self.move_selection(page),
            Action::PageUp if self.focus == Focus::Sidebar => self.move_selection(-page),
            Action::Top if self.focus == Focus::Sidebar => self.selected = 0,
            Action::Bottom if self.focus == Focus::Sidebar => {
                self.selected = self.visible().len().saturating_sub(1);
            }
            Action::Down => self.scroll_by(1),
            Action::Up => self.scroll_by(-1),
            Action::PageDown => self.scroll_by(page),
            Action::PageUp => self.scroll_by(-page),
            Action::Top => self.scroll = self.max_scroll,
            Action::Bottom => self.scroll = 0,
            Action::Open => {
                if self.focus == Focus::Sidebar {
                    self.open_selected().await;
                } else if self.open.is_some() {
                    self.mode = Mode::Compose;
                }
            }
            Action::Back => {
                if self.focus == Focus::Messages {
                    self.focus = Focus::Sidebar;
                } else if !self.query.is_empty() {
                    self.query.clear();
                    self.selected = 0;
                }
            }
            Action::Compose => {
                if self.open.is_some() {
                    self.mode = Mode::Compose;
                } else {
                    self.toast("Open a conversation first (Enter)", false);
                }
            }
            Action::Search => {
                self.focus = Focus::Sidebar;
                self.mode = Mode::Search;
            }
            Action::Help => self.mode = Mode::Help,
            Action::NextFilter => {
                self.filter = self.filter.next();
                self.selected = 0;
            }
            Action::Refresh => {
                self.handle.send(Command::RefreshConversations);
                if let Some(open) = &self.open {
                    self.handle.send(Command::OpenConversation(open.id));
                }
                self.toast("Refreshing…", false);
            }
            Action::LoadOlder => self.request_older(),
            Action::Retry => {
                let failed = self.messages.iter().rev().find(|m| {
                    m.is_mine && matches!(m.send_state, SendState::Failed | SendState::Uncertain)
                });
                match failed {
                    Some(m) => {
                        self.handle.send(Command::RetryMessage(m.id));
                        self.toast("Resending…", false);
                    }
                    None => self.toast("No failed message to resend", false),
                }
            }
        }
    }
}
