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
    for _ in 0..50 {
        app.on_key(key('k')).await;
    }
    assert_eq!(app.scroll, app.max_scroll);
    let s = screen(&mut app, 60, 14);
    assert!(s.contains("Did you see"), "{s}");
    app.on_key(key('G')).await;
    assert_eq!(app.scroll, 0);
    app.on_key(key('g')).await;
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
