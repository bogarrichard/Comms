//! UI tests: drive the real `App` against the demo backend and inspect what
//! ratatui would put on screen.

use std::time::Duration;

use comms_app::AppEvent;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, Focus, Mode};
use crate::keys::KeyMap;
use crate::ui;

async fn demo_app() -> App {
    let handle = comms_bootstrap::start_demo().await.unwrap();
    let mut app = App::new(handle, KeyMap::default_map()).await;
    settle(&mut app, |a| a.convs.len() >= 7).await;
    app
}

async fn app_for(kind: &'static str) -> App {
    let store = comms_storage::Store::open_in_memory().await.unwrap();
    let provider = comms_testkit::FakeProvider::demo().with_kind(kind);
    let handle = comms_app::AppHandle::start(
        std::sync::Arc::new(provider),
        store,
        comms_app::CoreConfig::default(),
    )
    .await
    .unwrap();
    let mut app = App::new(handle, KeyMap::default_map()).await;
    settle(&mut app, |a| a.convs.len() >= 7).await;
    app
}

/// Foreground colours of every cell on the row that contains `needle`.
fn row_colours(app: &mut App, needle: &str) -> Vec<ratatui::style::Color> {
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    for y in 0..buf.area.height {
        let row: String = (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_owned())
            .collect();
        if row.contains(needle) {
            return (0..buf.area.width).map(|x| buf[(x, y)].fg).collect();
        }
    }
    panic!("no row containing {needle:?}");
}

/// Pump core events into the app until `done` holds.
async fn settle(app: &mut App, done: impl Fn(&App) -> bool) {
    let mut rx = app.handle.subscribe();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    app.refresh_conversations().await;
    app.refresh_messages().await;
    while !done(app) {
        assert!(
            std::time::Instant::now() < deadline,
            "UI never reached the expected state"
        );
        if let Ok(Ok(ev)) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            app.on_event(ev).await;
        } else {
            app.refresh_conversations().await;
            app.refresh_messages().await;
        }
    }
}

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}
fn code(k: KeyCode) -> KeyEvent {
    KeyEvent::new(k, KeyModifiers::NONE)
}

async fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        app.on_key(key(c)).await;
    }
}

fn screen(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn conversation_list_shows_chats_then_channels_with_unread_marker() {
    let mut app = demo_app().await;
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("Teams · All"), "{s}");
    assert!(s.contains("● › Anna Kovács"), "unread chat is marked:\n{s}");
    assert!(s.contains("» Study group"), "{s}");
    assert!(s.contains("# General · Software Engi"), "{s}");
    let chat = s.find("Anna Kov").unwrap();
    let chan = s.find("# Announcements").unwrap();
    assert!(chat < chan, "chats come before channels");
    assert!(s.contains("NORMAL"));
}

#[tokio::test]
async fn opening_a_conversation_shows_history_and_clears_unread() {
    let mut app = demo_app().await;
    app.on_key(key('j')).await;
    app.on_key(key('k')).await;
    app.on_key(code(KeyCode::Enter)).await;
    settle(&mut app, |a| a.messages.len() == 4).await;
    assert_eq!(app.focus, Focus::Messages);
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("Did you see the new assignment?"), "{s}");
    assert!(s.contains("Want to team up for the project?"), "{s}");
    assert!(s.contains("You"), "own messages are labelled:\n{s}");
    settle(&mut app, |a| {
        a.convs
            .iter()
            .all(|c| !c.unread || c.title != "Anna Kovács")
    })
    .await;
    assert!(!screen(&mut app, 100, 30).contains("● › Anna"));
}

#[tokio::test]
async fn composing_and_sending_shows_the_message() {
    let mut app = demo_app().await;
    app.on_key(code(KeyCode::Enter)).await;
    settle(&mut app, |a| a.messages.len() == 4).await;

    app.on_key(key('i')).await;
    assert_eq!(app.mode, Mode::Compose);
    type_str(&mut app, "hello from the test").await;
    assert!(screen(&mut app, 100, 30).contains("hello from the test"));
    app.on_key(code(KeyCode::Enter)).await;
    assert!(app.input.is_blank(), "composer is cleared after sending");
    settle(&mut app, |a| a.messages.len() == 5).await;
    app.on_key(code(KeyCode::Esc)).await;
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("hello from the test"), "{s}");
    assert_eq!(app.mode, Mode::Normal);
}

#[tokio::test]
async fn alt_enter_inserts_a_newline_instead_of_sending() {
    let mut app = demo_app().await;
    app.on_key(code(KeyCode::Enter)).await;
    app.on_key(key('i')).await;
    type_str(&mut app, "a").await;
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT))
        .await;
    type_str(&mut app, "b").await;
    assert_eq!(app.input.text(), "a\nb");
    assert_eq!(app.mode, Mode::Compose);
}

#[tokio::test]
async fn blank_messages_are_not_sent_and_drafts_are_kept_per_conversation() {
    let mut app = demo_app().await;
    app.on_key(code(KeyCode::Enter)).await;
    app.on_key(key('i')).await;
    type_str(&mut app, "   ").await;
    app.on_key(code(KeyCode::Enter)).await;
    assert_eq!(app.input.text(), "   ", "blank input stays, nothing sent");

    app.input.clear();
    type_str(&mut app, "draft one").await;
    app.on_key(code(KeyCode::Esc)).await;
    app.on_key(code(KeyCode::Esc)).await; // back to the list
    app.on_key(key('j')).await;
    app.on_key(code(KeyCode::Enter)).await;
    assert_eq!(
        app.input.text(),
        "",
        "a different conversation has its own draft"
    );
    app.on_key(code(KeyCode::Esc)).await;
    app.on_key(key('k')).await;
    app.on_key(code(KeyCode::Enter)).await;
    assert_eq!(app.input.text(), "draft one");
}

#[tokio::test]
async fn filter_cycles_and_search_narrows_the_list() {
    let mut app = demo_app().await;
    app.on_key(code(KeyCode::Tab)).await;
    let s = screen(&mut app, 100, 30);
    assert!(
        s.contains("Teams · Chats") && !s.contains("# General"),
        "{s}"
    );
    app.on_key(code(KeyCode::Tab)).await;
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("Teams · Channels") && !s.contains("Anna"), "{s}");
    app.on_key(code(KeyCode::Tab)).await;

    app.on_key(key('/')).await;
    assert_eq!(app.mode, Mode::Search);
    type_str(&mut app, "net").await;
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("/net"), "{s}");
    assert!(
        s.contains("Computer Netw") && s.contains("Study group – Networks"),
        "{s}"
    );
    assert!(!s.contains("Anna"), "{s}");
    app.on_key(code(KeyCode::Enter)).await;
    assert!(app.open.is_some(), "Enter in search opens the first match");
    type_str(&mut app, "").await;
}

#[tokio::test]
async fn search_with_no_matches_and_escape_restores_the_list() {
    let mut app = demo_app().await;
    app.on_key(key('/')).await;
    type_str(&mut app, "zzzz").await;
    assert!(screen(&mut app, 100, 30).contains("No matches"));
    app.on_key(code(KeyCode::Esc)).await;
    assert!(screen(&mut app, 100, 30).contains("Anna Kovács"));
}

#[tokio::test]
async fn help_overlay_lists_the_configured_keys() {
    let mut app = demo_app().await;
    app.on_key(key('?')).await;
    let s = screen(&mut app, 100, 40);
    assert!(
        s.contains("Keys") && s.contains("open conversation") && s.contains("Alt+Enter"),
        "{s}"
    );
    app.on_key(key('x')).await;
    assert_eq!(app.mode, Mode::Normal);
}

#[tokio::test]
async fn channel_conversations_open_and_accept_messages() {
    let mut app = demo_app().await;
    app.on_key(code(KeyCode::Tab)).await;
    app.on_key(code(KeyCode::Tab)).await;
    app.on_key(code(KeyCode::Enter)).await;
    settle(&mut app, |a| !a.messages.is_empty()).await;
    let s = screen(&mut app, 100, 30);
    assert!(
        s.contains("in Computer Networks") || s.contains("in Software Engineering"),
        "{s}"
    );
    app.on_key(key('i')).await;
    type_str(&mut app, "posting to a channel").await;
    app.on_key(code(KeyCode::Enter)).await;
    settle(&mut app, |a| a.messages.iter().any(|m| m.is_mine)).await;
}

#[tokio::test]
async fn works_on_a_very_small_terminal_one_panel_at_a_time() {
    let mut app = demo_app().await;
    let s = screen(&mut app, 40, 12);
    assert!(
        s.contains("Anna Kovács") && !s.contains("Press i"),
        "list only:\n{s}"
    );
    app.on_key(code(KeyCode::Enter)).await;
    settle(&mut app, |a| a.messages.len() == 4).await;
    let s = screen(&mut app, 40, 12);
    assert!(
        s.contains("Press i to write") && !s.contains("Study group"),
        "chat only:\n{s}"
    );
    app.on_key(key('i')).await;
    type_str(
        &mut app,
        "a fairly long message that has to wrap inside a narrow input box",
    )
    .await;
    let _ = screen(&mut app, 40, 12);
}

#[tokio::test]
async fn degenerate_terminal_sizes_never_panic() {
    let mut app = demo_app().await;
    app.on_key(code(KeyCode::Enter)).await;
    settle(&mut app, |a| a.messages.len() == 4).await;
    app.on_key(key('i')).await;
    type_str(&mut app, "x\ny\nz").await;
    for (w, h) in [
        (1, 1),
        (5, 3),
        (20, 4),
        (69, 10),
        (70, 10),
        (200, 5),
        (120, 60),
    ] {
        let _ = screen(&mut app, w, h);
    }
    app.on_key(key('?')).await;
    let _ = screen(&mut app, 10, 5);
}

#[tokio::test]
async fn scrolling_stays_within_bounds() {
    let mut app = demo_app().await;
    app.on_key(code(KeyCode::Enter)).await;
    settle(&mut app, |a| a.messages.len() == 4).await;
    let _ = screen(&mut app, 60, 14); // short body so there is something to scroll
    assert!(app.max_scroll > 0);
    // j/k move the message cursor; the renderer scrolls it into view.
    for _ in 0..50 {
        app.on_key(key('k')).await;
    }
    let s = screen(&mut app, 60, 14);
    assert_eq!(app.scroll, app.max_scroll);
    assert!(s.contains("Did you see"), "{s}");
    app.on_key(key('G')).await;
    let _ = screen(&mut app, 60, 14);
    assert_eq!(app.scroll, 0);
    app.on_key(key('g')).await;
    let _ = screen(&mut app, 60, 14);
    assert_eq!(app.scroll, app.max_scroll);
}

#[tokio::test]
async fn quit_key_stops_the_loop_and_back_returns_to_the_list() {
    let mut app = demo_app().await;
    app.on_key(code(KeyCode::Enter)).await;
    assert_eq!(app.focus, Focus::Messages);
    app.on_key(code(KeyCode::Esc)).await;
    assert_eq!(app.focus, Focus::Sidebar);
    assert!(!app.quit);
    app.on_key(key('q')).await;
    assert!(app.quit);
}

#[tokio::test]
async fn failed_and_uncertain_sends_are_visible_and_retry_is_offered() {
    let mut app = demo_app().await;
    app.on_key(code(KeyCode::Enter)).await;
    settle(&mut app, |a| a.messages.len() == 4).await;
    app.on_event(AppEvent::MessageSendFailed {
        message: comms_domain::MessageId::new(),
        reason: "timed out".into(),
        uncertain: true,
    })
    .await;
    let s = screen(&mut app, 120, 30);
    assert!(s.contains("may or may not have been delivered"), "{s}");
    app.on_key(key('R')).await;
    assert!(screen(&mut app, 120, 30).contains("No failed message"));
}

#[tokio::test]
async fn connection_state_and_channel_problems_are_shown() {
    let mut app = demo_app().await;
    app.on_event(AppEvent::ConnectionStatusChanged(
        comms_app::ConnectionStatus::Offline("no route".into()),
    ))
    .await;
    assert!(screen(&mut app, 120, 30).contains("offline (no route)"));
    app.on_event(AppEvent::ConnectionStatusChanged(
        comms_app::ConnectionStatus::AuthRequired,
    ))
    .await;
    assert!(screen(&mut app, 120, 30).contains("comms login"));
    app.on_event(AppEvent::ChannelsUnavailable(
        "admin consent required".into(),
    ))
    .await;
    assert!(screen(&mut app, 120, 30).contains("Channels unavailable: admin consent required"));
}

#[tokio::test]
async fn bracketed_paste_goes_into_the_composer() {
    let mut app = demo_app().await;
    app.on_key(code(KeyCode::Enter)).await;
    app.on_key(key('i')).await;
    app.on_paste("line one\nline two").await;
    assert_eq!(app.input.text(), "line one\nline two");
}

// ------------------------------------------------------------- per-service colour

#[tokio::test]
async fn signal_is_blue_and_teams_is_purple() {
    use crate::theme::Theme;
    let mut signal = app_for("signal").await;
    let mut teams = app_for("msgraph").await;
    assert_eq!(signal.theme, Theme::SIGNAL);
    assert_eq!(teams.theme, Theme::TEAMS);

    // The service name in the sidebar title is drawn in the accent colour.
    let title = row_colours(&mut signal, "Signal · All");
    assert!(title.contains(&Theme::SIGNAL.accent), "{title:?}");
    assert!(!title.contains(&Theme::TEAMS.accent));
    let title = row_colours(&mut teams, "Teams · All");
    assert!(title.contains(&Theme::TEAMS.accent), "{title:?}");
    assert!(!title.contains(&Theme::SIGNAL.accent));
}

#[tokio::test]
async fn status_badge_and_focus_border_follow_the_accent() {
    use crate::theme::Theme;
    let mut app = app_for("signal").await;
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| ui::draw(f, &mut app)).unwrap();
    let buf = term.backend().buffer().clone();
    // NORMAL badge: accent background on the last row.
    let last = buf.area.height - 1;
    let badge = (0..buf.area.width)
        .map(|x| &buf[(x, last)])
        .find(|c| c.symbol() == "N");
    assert_eq!(badge.map(|c| c.bg), Some(Theme::SIGNAL.accent));
    // Focused sidebar border (top-left corner) uses the accent.
    assert_eq!(buf[(0, 0)].fg, Theme::SIGNAL.accent);
}

#[tokio::test]
async fn the_demo_keeps_the_teams_look_and_names_the_service() {
    let mut app = demo_app().await;
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("Teams · All"), "{s}");
    assert_eq!(app.theme, crate::theme::Theme::TEAMS);
}

#[tokio::test]
async fn unknown_backends_fall_back_to_the_neutral_look() {
    let mut app = app_for("irc").await;
    assert_eq!(app.theme, crate::theme::Theme::DEFAULT);
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("Comms · All"), "{s}");
}

// ---------------------------------------------------------------- Slack layout

async fn slack_app() -> App {
    let store = comms_storage::Store::open_in_memory().await.unwrap();
    let handle = comms_app::AppHandle::start(
        std::sync::Arc::new(comms_testkit::FakeProvider::demo_slack()),
        store,
        comms_app::CoreConfig::default(),
    )
    .await
    .unwrap();
    let mut app = App::new(handle, KeyMap::default_map()).await;
    settle(&mut app, |a| a.convs.len() >= 6).await;
    app
}

/// Open the conversation with this title through the real key path.
async fn open_title(app: &mut App, title: &str) {
    app.focus = Focus::Sidebar;
    let idx = app
        .visible()
        .iter()
        .position(|c| c.title == title)
        .unwrap_or_else(|| panic!("no conversation {title}"));
    app.selected = idx;
    app.on_key(code(KeyCode::Enter)).await;
    let want = title.to_owned();
    settle(app, |a| {
        a.open.as_ref().is_some_and(|c| c.title == want) && !a.messages.is_empty()
    })
    .await;
}

#[tokio::test]
async fn slack_shows_the_workspace_rail_with_home_activity_dms_and_channels() {
    let mut app = slack_app().await;
    assert_eq!(app.theme, crate::theme::Theme::SLACK);
    let s = screen(&mut app, 110, 30);
    assert!(
        s.contains("Home") && s.contains("Activity") && s.contains("DMs") && s.contains("Channels"),
        "{s}"
    );
    // The workspace tile carries the initial, drawn in the accent colour.
    let tile = row_colours(&mut app, "    S    ");
    assert!(
        tile.contains(&crate::theme::Theme::SLACK.accent),
        "{tile:?}"
    );
    // Each entry shows how many of its conversations have unread messages.
    let activity = app.unread_in(crate::app::RailItem::Activity);
    assert!(activity >= 1);
    assert!(
        s.contains("Activity") && s.contains(&format!("{activity} ")),
        "{s}"
    );
    assert!(app.rail_visible);
}

#[tokio::test]
async fn the_rail_filters_the_conversation_list() {
    let mut app = slack_app().await;
    let _ = screen(&mut app, 110, 30); // the renderer decides whether the rail fits
    // Esc/h from the list moves to the rail.
    app.on_key(key('h')).await;
    assert_eq!(app.focus, Focus::Rail);

    app.on_key(key('j')).await; // Activity = unread only
    assert!(app.visible().iter().all(|c| c.unread));
    assert!(!app.visible().is_empty());

    app.on_key(key('j')).await; // DMs
    let s = screen(&mut app, 110, 30);
    assert!(s.contains("Anna Kovács") && !s.contains("# general"), "{s}");

    app.on_key(key('j')).await; // Channels
    assert!(
        app.visible()
            .iter()
            .all(|c| c.kind == comms_domain::ConversationKind::Channel)
    );
    assert!(!app.visible().is_empty());
    let s = screen(&mut app, 110, 30);
    assert!(s.contains("# general") && !s.contains("Anna Kovács"), "{s}");

    app.on_key(key('k')).await;
    app.on_key(key('k')).await;
    app.on_key(key('k')).await; // Home
    assert!(app.visible().len() >= 6);

    // Enter/l goes back into the list.
    app.on_key(code(KeyCode::Enter)).await;
    assert_eq!(app.focus, Focus::Sidebar);
}

/// Move the message cursor (with `k`) to the first message that has replies.
async fn select_threaded_message(app: &mut App) {
    for _ in 0..20 {
        let has_replies = app
            .cursor_message()
            .and_then(|m| m.remote_id.clone())
            .is_some_and(|id| app.threads().iter().any(|(r, _)| *r == id));
        if has_replies {
            return;
        }
        app.on_key(key('k')).await;
    }
    panic!("no threaded message found");
}

#[tokio::test]
async fn the_thread_pane_opens_beside_the_chat_and_replies_leave_the_channel_view() {
    let mut app = slack_app().await;
    open_title(&mut app, "lab-notes").await;

    // Before opening: replies are folded into "N replies" under their root.
    let s = screen(&mut app, 120, 30);
    assert!(s.contains("4 replies"), "{s}");
    assert!(!s.contains("Which folder should"), "replies hidden: {s}");
    assert!(!s.contains("Thread"), "pane closed: {s}");

    select_threaded_message(&mut app).await;
    app.on_key(key('t')).await;
    assert!(app.thread.is_some());
    let s = screen(&mut app, 120, 30);
    assert!(s.contains("Thread") && s.contains("4 replies"), "{s}");
    assert!(
        s.contains("Please upload") && s.contains("Which folder should"),
        "root and replies are in the pane: {s}"
    );
    assert!(s.contains("run 7 is uploaded"), "{s}");

    // `t` again closes it.
    app.on_key(key('t')).await;
    assert!(app.thread.is_none());
    let s = screen(&mut app, 120, 30);
    assert!(!s.contains("Thread"), "{s}");
}

#[tokio::test]
async fn threads_can_be_stepped_and_are_reset_when_switching_conversation() {
    let mut app = slack_app().await;
    open_title(&mut app, "general").await;
    assert_eq!(app.threads().len(), 1);
    select_threaded_message(&mut app).await;
    app.on_key(key('t')).await;
    let first = app.thread.clone();
    assert!(first.is_some());
    // Only one thread: stepping stays on it.
    app.on_key(key(']')).await;
    app.on_key(key('[')).await;
    assert_eq!(app.thread, first);

    open_title(&mut app, "random").await;
    assert!(
        app.thread.is_none(),
        "thread pane closes with the conversation"
    );
    app.on_key(key('t')).await;
    assert!(app.thread.is_none());
    assert!(
        app.toast
            .as_ref()
            .is_some_and(|t| t.text.contains("no replies")),
        "a message without replies says so"
    );
}

#[tokio::test]
async fn on_a_narrower_terminal_the_thread_replaces_the_chat_and_the_rail_goes() {
    let mut app = slack_app().await;
    open_title(&mut app, "lab-notes").await;
    select_threaded_message(&mut app).await;
    app.on_key(key('t')).await;

    // 90 columns: rail yes, but the chat cannot keep 40 columns beside a pane.
    let s = screen(&mut app, 90, 30);
    assert!(s.contains("Home"), "{s}");
    assert!(s.contains("Thread") && s.contains("Which folder"), "{s}");

    // Under 80 columns the rail is dropped and focus cannot be stuck on it.
    app.focus = Focus::Rail;
    let s = screen(&mut app, 76, 24);
    assert!(!s.contains("Channels") && !s.contains("Activity"), "{s}");
    assert!(!app.rail_visible);
    assert_ne!(app.focus, Focus::Rail);
}

#[tokio::test]
async fn teams_and_signal_keep_their_single_sidebar_layout() {
    for kind in ["msgraph", "signal"] {
        let mut app = app_for(kind).await;
        let s = screen(&mut app, 110, 30);
        assert!(!s.contains("Channels") && !s.contains("DMs"), "{kind}: {s}");
        assert!(!app.rail_visible);
        // `h` does nothing special without a rail.
        app.focus = Focus::Sidebar;
        app.on_key(key('h')).await;
        assert_eq!(app.focus, Focus::Sidebar);
    }
}

#[tokio::test]
async fn slack_uses_green_blue_grey_for_selection_and_thread_markers() {
    use crate::theme::Theme;
    let mut app = slack_app().await;
    open_title(&mut app, "lab-notes").await;
    app.focus = Focus::Sidebar;
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    term.draw(|f| ui::draw(f, &mut app)).unwrap();
    let buf = term.backend().buffer().clone();
    let bgs: Vec<_> = buf.content().iter().map(|c| c.bg).collect();
    // Selected conversation row and active rail item use the blue.
    assert!(bgs.contains(&Theme::SLACK.secondary));
    // The "N replies" marker is blue, timestamps are grey.
    let marker = row_colours(&mut app, "4 replies");
    assert!(marker.contains(&Theme::SLACK.secondary), "{marker:?}");
    assert!(marker.contains(&Theme::SLACK.muted), "{marker:?}");
}

async fn type_command(app: &mut App, text: &str, enter: bool) {
    app.on_key(key(':')).await;
    for c in text.chars() {
        app.on_key(key(c)).await;
    }
    if enter {
        app.on_key(code(KeyCode::Enter)).await;
    }
}

fn with_apps(app: &mut App) {
    app.apps = ["slack", "teams", "signal"].map(String::from).to_vec();
    app.app_name = "slack".to_owned();
}

#[tokio::test]
async fn colon_opens_the_command_line_and_shows_what_you_type() {
    let mut app = demo_app().await;
    type_command(&mut app, "sw", false).await;
    assert_eq!(app.mode, Mode::Command);
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("COMMAND") && s.contains(":sw"), "{s}");
}

#[tokio::test]
async fn escape_and_backspacing_past_the_colon_leave_the_command_line() {
    let mut app = demo_app().await;
    type_command(&mut app, "x", false).await;
    app.on_key(code(KeyCode::Esc)).await;
    assert_eq!(app.mode, Mode::Normal);
    type_command(&mut app, "x", false).await;
    app.on_key(code(KeyCode::Backspace)).await;
    assert_eq!(app.mode, Mode::Command);
    app.on_key(code(KeyCode::Backspace)).await;
    assert_eq!(app.mode, Mode::Normal);
}

#[tokio::test]
async fn q_and_help_and_unknown_commands() {
    let mut app = demo_app().await;
    type_command(&mut app, "help", true).await;
    assert_eq!(app.mode, Mode::Help);
    app.on_key(key('x')).await;
    type_command(&mut app, "bogus", true).await;
    assert!(
        app.toast
            .as_ref()
            .is_some_and(|t| t.error && t.text.contains("bogus"))
    );
    assert!(!app.quit);
    type_command(&mut app, "q", true).await;
    assert!(app.quit);
}

#[tokio::test]
async fn switch_asks_the_main_loop_to_change_app() {
    let mut app = demo_app().await;
    with_apps(&mut app);
    type_command(&mut app, "switch teams", true).await;
    assert_eq!(app.switch_to.as_deref(), Some("teams"));
    assert_eq!(app.mode, Mode::Normal);
}

#[tokio::test]
async fn switch_without_a_name_lists_apps_and_rejects_bad_ones() {
    let mut app = demo_app().await;
    with_apps(&mut app);
    type_command(&mut app, "switch", true).await;
    let t = app.toast.as_ref().unwrap();
    assert!(
        t.text.contains("slack*") && t.text.contains("teams"),
        "{}",
        t.text
    );
    type_command(&mut app, "switch slack", true).await;
    assert!(app.toast.as_ref().unwrap().text.contains("Already"));
    type_command(&mut app, "switch irc", true).await;
    assert!(app.toast.as_ref().unwrap().error);
    assert!(app.switch_to.is_none());
}

#[tokio::test]
async fn switching_is_unavailable_without_configured_apps() {
    let mut app = demo_app().await;
    type_command(&mut app, "switch teams", true).await;
    assert!(app.toast.as_ref().unwrap().error);
    assert!(app.switch_to.is_none());
}

#[tokio::test]
async fn tab_completes_commands_and_app_names() {
    let mut app = demo_app().await;
    with_apps(&mut app);
    type_command(&mut app, "sw", false).await;
    app.on_key(code(KeyCode::Tab)).await;
    assert_eq!(app.command, "switch");
    app.on_key(key(' ')).await;
    app.on_key(key('t')).await;
    app.on_key(code(KeyCode::Tab)).await;
    assert_eq!(app.command, "switch teams");
    // "s" is ambiguous (slack, signal): left alone.
    app.command = "switch s".to_owned();
    app.on_key(code(KeyCode::Tab)).await;
    assert_eq!(app.command, "switch s");
}

#[tokio::test]
async fn the_cursor_starts_on_the_newest_message_and_moves_with_j_and_k() {
    let mut app = slack_app().await;
    open_title(&mut app, "lab-notes").await;
    let shown = app.displayed();
    let newest = app.messages[*shown.last().unwrap()].id;
    assert_eq!(app.cursor, Some(newest));

    app.on_key(key('k')).await;
    assert_ne!(app.cursor, Some(newest));
    app.on_key(key('j')).await;
    assert_eq!(app.cursor, Some(newest));
    // At the newest message `j` stays put; replies hidden in a thread are skipped.
    app.on_key(key('j')).await;
    assert_eq!(app.cursor, Some(newest));
    for idx in app.displayed() {
        let m = &app.messages[idx];
        assert!(
            m.reply_to.is_none()
                || !app
                    .messages
                    .iter()
                    .any(|r| r.remote_id.as_ref() == m.reply_to.as_ref())
        );
    }
}

#[tokio::test]
async fn the_selected_message_gets_a_gutter_bar_only_while_the_chat_has_focus() {
    let mut app = slack_app().await;
    open_title(&mut app, "lab-notes").await;
    let s = screen(&mut app, 120, 30);
    assert!(s.contains("▌"), "{s}");
    let bars = s.lines().filter(|l| l.contains("▌")).count();
    assert!(bars >= 1);

    app.on_key(code(KeyCode::Esc)).await; // back to the list
    let s = screen(&mut app, 120, 30);
    assert!(!s.contains("▌"), "{s}");
}

#[tokio::test]
async fn moving_the_cursor_up_scrolls_the_chat_to_keep_it_visible() {
    let mut app = slack_app().await;
    open_title(&mut app, "lab-notes").await;
    // A short window so the oldest message is off screen.
    let _ = screen(&mut app, 120, 12);
    assert_eq!(app.scroll, 0);
    for _ in 0..30 {
        app.on_key(key('k')).await;
    }
    let s = screen(&mut app, 120, 12);
    let first = app.displayed()[0];
    assert_eq!(app.cursor, Some(app.messages[first].id));
    assert!(app.scroll > 0, "scrolled up to the oldest: {s}");
    assert!(s.contains("▌"), "{s}");
}

#[tokio::test]
async fn the_status_bar_shows_the_keys_that_work_where_you_are() {
    let mut app = slack_app().await;
    let s = screen(&mut app, 120, 30);
    assert!(s.contains("open") && s.contains("search"), "{s}");
    open_title(&mut app, "lab-notes").await;
    let s = screen(&mut app, 120, 30);
    assert!(
        s.contains("select") && s.contains("thread") && !s.contains("search"),
        "{s}"
    );
    app.on_key(key('i')).await;
    let s = screen(&mut app, 120, 30);
    assert!(s.contains("Alt+Enter new line"), "{s}");
}
