//! Drawing. Pure function of `App` state; never touches the network.

use chrono::{Local, NaiveDate};
use comms_app::ConnectionStatus;
use comms_domain::{Conversation, ConversationKind, Message, RemoteId, SendState};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus, Mode};
use crate::keys::Action;
use crate::render::content_lines;
use crate::theme::Theme;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Odd inner width (this minus the border) so the tile and the name centre on the same column.
const RAIL_WIDTH: u16 = 22;
/// Narrower than this and the workspace rail is dropped.
const RAIL_MIN_TOTAL: u16 = 80;
const THREAD_WIDTH: u16 = 38;
/// The thread pane sits beside the chat only when the chat keeps this much.
const THREAD_BESIDE_MIN: u16 = 40 + THREAD_WIDTH;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let [main, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    app.rail_visible = app.theme.workspace_ui && area.width >= RAIL_MIN_TOTAL;
    if !app.rail_visible && app.focus == Focus::Rail {
        app.focus = Focus::Sidebar;
    }
    let main = if app.rail_visible {
        let [rail, rest] =
            Layout::horizontal([Constraint::Length(RAIL_WIDTH), Constraint::Min(10)]).areas(main);
        draw_rail(f, app, rail);
        rest
    } else {
        main
    };
    if main.width < 70 {
        // Small terminals: one panel at a time.
        match (app.focus, app.open.is_some()) {
            (Focus::Messages, true) => draw_content(f, app, main),
            _ => draw_sidebar(f, app, main),
        }
    } else {
        let side = (main.width * 3 / 10).clamp(24, 42);
        let [left, right] =
            Layout::horizontal([Constraint::Length(side), Constraint::Min(10)]).areas(main);
        draw_sidebar(f, app, left);
        draw_content(f, app, right);
    }
    draw_status(f, app, status);
    if app.mode == Mode::Help {
        draw_help(f, app, area);
    }
}

/// The chat, plus the thread pane when one is open: beside the chat if there
/// is room, otherwise in its place.
fn draw_content(f: &mut Frame, app: &mut App, area: Rect) {
    if app.thread.is_none() {
        return draw_chat(f, app, area);
    }
    if area.width >= THREAD_BESIDE_MIN {
        let [chat, thread] =
            Layout::horizontal([Constraint::Min(10), Constraint::Length(THREAD_WIDTH)]).areas(area);
        draw_chat(f, app, chat);
        draw_thread(f, app, thread);
    } else {
        draw_thread(f, app, area);
    }
}

fn draw_rail(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Rail;
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(border(app, focused));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 9 || inner.height < 8 {
        return;
    }
    let t = &app.theme;

    // The workspace "image": a tile with the workspace's initial.
    let name = app.workspace_name();
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map_or('S', |c| c.to_ascii_uppercase());
    // 9x5 cells is about square (cells are twice as tall as wide) and
    // centres exactly in the odd inner width.
    let tile_w = 9usize;
    let tile_h = 5usize;
    let big = inner.height >= 12 && inner.width as usize >= tile_w;
    let pad = (inner.width as usize).saturating_sub(tile_w) / 2;
    let tile = Style::new().bg(t.accent).fg(Color::Black);
    let has_icon = app.icon.is_some();
    let mut lines: Vec<Line> = Vec::new();
    if big {
        for row in 0..tile_h {
            if has_icon {
                // The icon image is drawn over these rows afterwards.
                lines.push(Line::raw(""));
                continue;
            }
            let text = if row == tile_h / 2 {
                let side = (tile_w - 1) / 2;
                format!("{}{initial}{}", " ".repeat(side), " ".repeat(side))
            } else {
                " ".repeat(tile_w)
            };
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(pad)),
                Span::styled(text, tile.add_modifier(Modifier::BOLD)),
            ]));
        }
        lines.push(Line::raw(""));
    }
    lines.push(
        Line::from(Span::styled(
            truncate(&name, inner.width as usize),
            Style::new().fg(t.muted).add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center),
    );
    lines.push(Line::raw(""));

    let active = app.rail_item();
    for item in crate::app::RailItem::ALL {
        let unread = app.unread_in(item);
        // Label on the left, the unread count right-aligned.
        let badge = if unread > 0 {
            format!("{unread} ")
        } else {
            String::new()
        };
        let room = (inner.width as usize).saturating_sub(badge.width());
        let label = truncate(&format!("  {}  {}", item.icon(), item.label()), room);
        let fill = room.saturating_sub(label.width());
        let text = format!("{label}{}{badge}", " ".repeat(fill));
        let style = if item == active {
            // Green under the cursor, blue when it is only where we are.
            let bg = if focused { t.accent } else { t.secondary };
            Style::new()
                .bg(bg)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(t.muted)
        };
        lines.push(Line::from(Span::styled(text, style)));
    }
    f.render_widget(Paragraph::new(lines), inner);
    if let Some(icon) = app.icon.as_mut().filter(|_| big) {
        let w = (tile_w as u16).min(inner.width);
        let rect = Rect {
            x: inner.x + (inner.width - w) / 2,
            y: inner.y,
            width: w,
            height: tile_h as u16,
        };
        f.render_stateful_widget(ratatui_image::StatefulImage::default(), rect, icon);
    }
}

fn draw_thread(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let replies = app.thread_replies();
    let title = Line::from(vec![
        Span::styled(
            " Thread ",
            Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "· {} {} · {} close ",
                replies.len(),
                if replies.len() == 1 {
                    "reply"
                } else {
                    "replies"
                },
                app.keymap.labels(Action::Thread)
            ),
            Style::new().fg(t.muted),
        ),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(t.secondary))
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let w = inner.width as usize;
    let h = inner.height as usize;

    let mut root_lines = match app.thread_root() {
        Some(m) => message_lines(std::slice::from_ref(m), w, t, false),
        None => vec![Line::from(Span::styled(
            "(original message not loaded)",
            Style::new().fg(t.muted).add_modifier(Modifier::ITALIC),
        ))],
    };
    root_lines.truncate((h / 3).max(2));
    // Inside the pane every message is a reply; the tag would only repeat.
    let owned: Vec<Message> = replies
        .iter()
        .map(|m| Message {
            reply_to: None,
            ..(*m).clone()
        })
        .collect();
    let reply_lines = message_lines(&owned, w, t, false);

    let mut lines = root_lines;
    lines.push(Line::from(Span::styled(
        "─".repeat(w),
        Style::new().fg(t.muted),
    )));
    let room = h.saturating_sub(lines.len());
    let start = reply_lines.len().saturating_sub(room);
    lines.extend(reply_lines[start..].iter().cloned());
    f.render_widget(Paragraph::new(lines), inner);
}

fn border(app: &App, focused: bool) -> Style {
    Style::new().fg(if focused {
        app.theme.accent
    } else {
        app.theme.muted
    })
}

fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

fn kind_icon(k: ConversationKind) -> &'static str {
    match k {
        ConversationKind::Direct => "›",
        ConversationKind::Group => "»",
        ConversationKind::Meeting => "◷",
        ConversationKind::Channel => "#",
    }
}

fn conv_label(c: &Conversation) -> String {
    match (&c.group, c.kind) {
        (Some(g), ConversationKind::Channel) => format!("{} · {g}", c.title),
        _ => c.title.clone(),
    }
}

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Sidebar && app.mode != Mode::Compose;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border(app, focused))
        .title(Line::from(vec![
            Span::styled(
                format!(" {}", app.theme.name),
                Style::new()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" · {} ", app.filter.label())),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let searching = app.mode == Mode::Search || !app.query.is_empty();
    let list_h = inner.height.saturating_sub(u16::from(searching));
    let list_area = Rect {
        height: list_h,
        ..inner
    };
    let width = inner.width as usize;

    let visible = app.visible();
    let open_id = app.open.as_ref().map(|c| c.id);
    let items: Vec<ListItem> = visible
        .iter()
        .map(|c| {
            let marker = if c.unread {
                Span::styled("● ", Style::new().fg(Color::Yellow))
            } else {
                Span::raw("  ")
            };
            let mut style = Style::new();
            if c.unread {
                style = style.add_modifier(Modifier::BOLD);
            }
            if Some(c.id) == open_id {
                style = style.fg(app.theme.accent);
            }
            let label = truncate(
                &format!("{} {}", kind_icon(c.kind), conv_label(c)),
                width.saturating_sub(2),
            );
            ListItem::new(Line::from(vec![marker, Span::styled(label, style)]))
        })
        .collect();
    let empty = items.is_empty();
    let mut state = ListState::default();
    if !empty {
        state.select(Some(app.selected.min(visible.len() - 1)));
    }
    drop(visible);
    if empty {
        let msg = if !app.query.is_empty() {
            "No matches"
        } else if app.syncing {
            "Loading conversations…"
        } else {
            "No conversations"
        };
        f.render_widget(
            Paragraph::new(msg).style(Style::new().fg(Color::DarkGray)),
            list_area,
        );
    } else {
        let hl = if app.theme.workspace_ui {
            let bg = if focused {
                app.theme.accent
            } else {
                app.theme.secondary
            };
            Style::new()
                .bg(bg)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().add_modifier(Modifier::REVERSED)
        };
        f.render_stateful_widget(List::new(items).highlight_style(hl), list_area, &mut state);
    }
    if searching {
        let row = Rect {
            y: inner.y + list_h,
            height: 1,
            ..inner
        };
        let cursor = if app.mode == Mode::Search { "▏" } else { "" };
        f.render_widget(
            Paragraph::new(format!("/{}{cursor}", app.query)).style(Style::new().fg(Color::Yellow)),
            row,
        );
    }
}

fn name_color(name: &str, palette: &[Color]) -> Color {
    let h = name
        .bytes()
        .fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize));
    palette[h % palette.len()]
}

/// `threaded`: hide replies whose root is in `msgs` and show "N replies"
/// under the root instead (the thread pane shows them).
fn message_lines(
    msgs: &[Message],
    width: usize,
    theme: &Theme,
    threaded: bool,
) -> Vec<Line<'static>> {
    message_layout(msgs, width, theme, threaded).0
}

/// One message's place in the laid-out lines: the index into the input
/// slice and the `start..end` line range (end exclusive).
type MessageSpan = (usize, usize, usize);

/// Every line starts with a one-column gutter, where the chat draws the
/// selection bar.
fn message_layout(
    msgs: &[Message],
    width: usize,
    theme: &Theme,
    threaded: bool,
) -> (Vec<Line<'static>>, Vec<MessageSpan>) {
    let width = width.saturating_sub(1);
    let accent = theme.accent;
    let muted = theme.muted;
    let all = msgs;
    let in_list = |id: &RemoteId| msgs.iter().any(|m| m.remote_id.as_ref() == Some(id));
    let msgs: Vec<(usize, &Message)> = msgs
        .iter()
        .enumerate()
        .filter(|(_, m)| !(threaded && m.reply_to.as_ref().is_some_and(in_list)))
        .collect();
    let mut spans: Vec<MessageSpan> = Vec::new();
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut last_day: Option<NaiveDate> = None;
    let body_w = width.saturating_sub(2).max(1);
    for (orig, m) in msgs.iter().copied() {
        let local = m.timestamp.with_timezone(&Local);
        // A message that opens a new day owns that day's heading line.
        let mut heading_at = None;
        if last_day != Some(local.date_naive()) {
            last_day = Some(local.date_naive());
            if !out.is_empty() {
                out.push(Line::raw(""));
            }
            heading_at = Some(out.len());
            out.push(
                Line::from(Span::styled(
                    format!("── {} ──", local.format("%a %e %b %Y")),
                    Style::new().fg(muted),
                ))
                .alignment(Alignment::Center),
            );
        }
        let name_style = if m.is_mine {
            Style::new().fg(accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new()
                .fg(name_color(&m.sender.display_name, theme.names))
                .add_modifier(Modifier::BOLD)
        };
        let mut head = vec![
            Span::styled(
                if m.is_mine {
                    "You".to_owned()
                } else {
                    m.sender.display_name.clone()
                },
                name_style,
            ),
            Span::styled(
                format!("  {}", local.format("%H:%M")),
                Style::new().fg(muted),
            ),
        ];
        if m.reply_to.is_some() {
            head.push(Span::styled("  ↳ reply", Style::new().fg(muted)));
        }
        if m.edited_at.is_some() {
            head.push(Span::styled("  (edited)", Style::new().fg(muted)));
        }
        match m.send_state {
            SendState::Sent => {}
            SendState::Pending => {
                head.push(Span::styled("  sending…", Style::new().fg(Color::Yellow)))
            }
            SendState::Failed => {
                head.push(Span::styled(
                    "  ✗ not sent (R to retry)",
                    Style::new().fg(Color::Red),
                ));
            }
            SendState::Uncertain => head.push(Span::styled(
                "  ? may not have been sent (R to resend)",
                Style::new().fg(Color::Red),
            )),
        }
        let start = heading_at.unwrap_or(out.len());
        out.push(Line::from(head));

        let mut body: Vec<Line<'static>> = if m.deleted_at.is_some() {
            vec![Line::from(Span::styled(
                "(message deleted)",
                Style::new().fg(muted).add_modifier(Modifier::ITALIC),
            ))]
        } else {
            content_lines(&m.content, body_w)
        };
        for a in &m.attachments {
            body.push(Line::from(Span::styled(
                truncate(&format!("[attachment] {}", a.name), body_w),
                Style::new().fg(Color::Blue),
            )));
        }
        for mut l in body {
            l.spans.insert(0, Span::raw("  "));
            out.push(l);
        }
        if threaded && let Some(id) = &m.remote_id {
            let replies: Vec<&Message> = all
                .iter()
                .filter(|r| r.reply_to.as_ref() == Some(id) && r.deleted_at.is_none())
                .collect();
            if let Some(last) = replies.last() {
                out.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!(
                            "↳ {} {}",
                            replies.len(),
                            if replies.len() == 1 {
                                "reply"
                            } else {
                                "replies"
                            }
                        ),
                        Style::new()
                            .fg(theme.secondary)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            "  last {}",
                            last.timestamp.with_timezone(&Local).format("%H:%M")
                        ),
                        Style::new().fg(muted),
                    ),
                ]));
            }
        }
        spans.push((orig, start, out.len()));
        out.push(Line::raw(""));
    }
    out.pop();
    for line in &mut out {
        line.spans.insert(0, Span::raw(" "));
    }
    (out, spans)
}

fn draw_chat(f: &mut Frame, app: &mut App, area: Rect) {
    let composing = app.mode == Mode::Compose;
    let input_h = if composing {
        let w = area.width.saturating_sub(2) as usize;
        let (lines, _) = app.input.layout(w);
        (lines.len() as u16 + 2).clamp(3, 8)
    } else {
        3
    }
    .min(area.height.saturating_sub(3).max(1));
    let [header, body, input] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(input_h),
    ])
    .areas(area);

    // Header
    let head_line = match &app.open {
        Some(c) => {
            let mut spans = vec![Span::styled(
                format!(" {} {}", kind_icon(c.kind), c.title),
                Style::new().add_modifier(Modifier::BOLD),
            )];
            if let Some(g) = &c.group {
                spans.push(Span::styled(
                    format!("  in {g}"),
                    Style::new().fg(Color::DarkGray),
                ));
            }
            if c.kind == ConversationKind::Channel && app.channels_note.is_some() {
                spans.push(Span::styled(
                    "  (limited access – see status)",
                    Style::new().fg(Color::Red),
                ));
            }
            Line::from(spans)
        }
        None => Line::from(Span::styled(
            " No conversation open",
            Style::new().fg(Color::DarkGray),
        )),
    };
    f.render_widget(Paragraph::new(head_line), header);

    // Messages
    let focused = app.focus == Focus::Messages && !composing;
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border(app, focused));
    if app.has_older && app.open.is_some() {
        block = block.title(format!(
            " ▲ older messages available ({}) ",
            app.keymap.labels(Action::LoadOlder)
        ));
    }
    let inner = block.inner(body);
    f.render_widget(block, body);
    if inner.width > 0 && inner.height > 0 {
        if app.open.is_none() {
            let hint = Paragraph::new(format!(
                "Select a conversation with {} and press {}.\nPress {} for help.",
                app.keymap.labels(Action::Down),
                app.keymap.labels(Action::Open),
                app.keymap.labels(Action::Help),
            ))
            .style(Style::new().fg(Color::DarkGray));
            f.render_widget(hint, inner);
        } else if app.messages.is_empty() {
            let msg = if app.syncing {
                "Loading messages…"
            } else {
                "No messages yet"
            };
            f.render_widget(
                Paragraph::new(msg).style(Style::new().fg(Color::DarkGray)),
                inner,
            );
        } else {
            let (mut lines, spans) = message_layout(
                &app.messages,
                inner.width as usize,
                &app.theme,
                app.theme.workspace_ui,
            );
            let h = inner.height as usize;
            app.max_scroll = lines.len().saturating_sub(h);
            // The selected message's lines.
            let selected = app
                .cursor
                .and_then(|id| app.messages.iter().position(|m| m.id == id))
                .and_then(|i| spans.iter().find(|(o, ..)| *o == i))
                .map(|&(_, s, e)| (s, e));
            if app.reveal_cursor {
                app.reveal_cursor = false;
                if let Some((s, e)) = selected {
                    let end = lines.len() - app.scroll.min(app.max_scroll);
                    let first = end.saturating_sub(h);
                    if e > end {
                        app.scroll = lines.len() - e;
                    } else if s < first {
                        app.scroll = lines.len() - (s + h).min(lines.len());
                    }
                }
            }
            app.scroll = app.scroll.min(app.max_scroll);
            if app.focus == Focus::Messages
                && let Some((s, e)) = selected
            {
                for line in &mut lines[s..e] {
                    line.spans[0] = Span::styled("▌", Style::new().fg(app.theme.accent));
                }
            }
            let end = lines.len() - app.scroll;
            let start = end.saturating_sub(h);
            f.render_widget(Paragraph::new(lines[start..end].to_vec()), inner);
            if app.scroll > 0 {
                let tag = format!(" ↓ {} more lines ", app.scroll);
                let w = tag.width() as u16;
                if inner.width > w {
                    let r = Rect {
                        x: inner.x + inner.width - w,
                        y: inner.y + inner.height - 1,
                        width: w,
                        height: 1,
                    };
                    f.render_widget(
                        Paragraph::new(tag)
                            .style(Style::new().bg(Color::DarkGray).fg(Color::White)),
                        r,
                    );
                }
            }
        }
    }

    // Input
    let title = if composing {
        " Message — Enter: send · Alt+Enter: new line · Esc: leave ".to_owned()
    } else if app.open.is_some() {
        format!(" Press {} to write ", app.keymap.labels(Action::Compose))
    } else {
        " Message ".to_owned()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border(app, composing))
        .title(title);
    let inner = block.inner(input);
    f.render_widget(block, input);
    if composing && inner.width > 0 && inner.height > 0 {
        let (lines, (row, col)) = app.input.layout(inner.width as usize);
        let h = inner.height as usize;
        let first = (row + 1).saturating_sub(h);
        let shown: Vec<Line> = lines
            .iter()
            .skip(first)
            .take(h)
            .map(|l| Line::raw(l.clone()))
            .collect();
        f.render_widget(Paragraph::new(shown), inner);
        f.set_cursor_position((inner.x + col as u16, inner.y + (row - first) as u16));
    } else if !composing && !app.input.is_blank() {
        f.render_widget(
            Paragraph::new("(draft saved)").style(Style::new().fg(Color::DarkGray)),
            inner,
        );
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let (badge, color) = match app.mode {
        Mode::Normal => (" NORMAL ", app.theme.accent),
        Mode::Compose => (" INSERT ", Color::Green),
        Mode::Search => (" SEARCH ", Color::Yellow),
        Mode::Help => (" HELP ", Color::Cyan),
        Mode::Command => (" COMMAND ", Color::Magenta),
    };
    let conn = match &app.connection {
        ConnectionStatus::Online => Span::styled("● online", Style::new().fg(Color::Green)),
        ConnectionStatus::Connecting => {
            Span::styled("◌ connecting", Style::new().fg(Color::Yellow))
        }
        ConnectionStatus::Offline(why) => Span::styled(
            format!("○ offline ({})", truncate(why, 30)),
            Style::new().fg(Color::Red),
        ),
        ConnectionStatus::AuthRequired => Span::styled(
            "⚠ sign-in required: quit and run `comms login`",
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    };
    let spin = if app.syncing {
        SPINNER[(app.tick as usize) % SPINNER.len()]
    } else {
        " "
    };
    let right = Line::from(vec![
        Span::styled(spin, Style::new().fg(app.theme.accent)),
        Span::raw(" "),
        conn,
        Span::raw(" "),
    ]);
    let right_w = right.width() as u16;

    let [left_area, right_area] = Layout::horizontal([
        Constraint::Min(1),
        Constraint::Length(right_w.min(area.width)),
    ])
    .areas(area);
    let mut left = vec![
        Span::styled(
            badge,
            Style::new()
                .bg(color)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    if app.mode == Mode::Command {
        left.push(Span::raw(format!(":{}▏", app.command)));
        f.render_widget(Paragraph::new(Line::from(left)), left_area);
        f.render_widget(
            Paragraph::new(right).alignment(Alignment::Right),
            right_area,
        );
        return;
    }
    match &app.toast {
        Some(t) => left.push(Span::styled(
            t.text.clone(),
            Style::new().fg(if t.error {
                Color::Red
            } else {
                app.theme.accent
            }),
        )),
        None => left.push(Span::styled(hints(app), Style::new().fg(Color::DarkGray))),
    }
    f.render_widget(Paragraph::new(Line::from(left)), left_area);
    f.render_widget(
        Paragraph::new(right).alignment(Alignment::Right),
        right_area,
    );
}

/// The keys that matter right now, for the status bar.
fn hints(app: &App) -> String {
    let k = |a: Action| app.keymap.labels(a);
    match (app.mode, app.focus) {
        (Mode::Compose, _) => "Enter send · Alt+Enter new line · Esc stop writing".to_owned(),
        (Mode::Search, _) => "type to filter · Enter keep · Esc clear".to_owned(),
        (_, Focus::Rail) => format!(
            "{} choose · {} back to the list · {} help",
            k(Action::Down),
            k(Action::FocusRight),
            k(Action::Help)
        ),
        (_, Focus::Sidebar) => format!(
            "{} open · {} search · {} filter · {} command · {} help · {} quit",
            k(Action::Open),
            k(Action::Search),
            k(Action::NextFilter),
            k(Action::Command),
            k(Action::Help),
            k(Action::Quit)
        ),
        (_, Focus::Messages) => format!(
            "{} select · {} write · {} thread · {} older · {} list · {} help",
            k(Action::Down),
            k(Action::Compose),
            k(Action::Thread),
            k(Action::LoadOlder),
            k(Action::FocusLeft),
            k(Action::Help)
        ),
    }
}

fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Action::ALL
        .iter()
        .map(|a| {
            Line::from(vec![
                Span::styled(
                    format!("{:<22}", app.keymap.labels(*a)),
                    Style::new().fg(Color::Cyan),
                ),
                Span::raw(a.description()),
            ])
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "While writing:",
        Style::new().add_modifier(Modifier::BOLD),
    ));
    for (k, d) in [
        ("Enter", "send"),
        ("Alt+Enter / Ctrl+J", "new line"),
        ("Esc", "stop writing (draft is kept)"),
        ("Ctrl+U", "clear"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("{k:<22}"), Style::new().fg(Color::Cyan)),
            Span::raw(d),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Commands (after the command key):",
        Style::new().add_modifier(Modifier::BOLD),
    ));
    for (k, d) in [
        (
            "switch <app>",
            "change app (Tab completes; no name lists them)",
        ),
        ("refresh", "refresh now"),
        ("help / quit", "this help / leave"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("{k:<22}"), Style::new().fg(Color::Cyan)),
            Span::raw(d),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Press any key to close",
        Style::new().fg(Color::DarkGray),
    ));

    let w = 62.min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let popup = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Keys ")
                .border_style(border(app, true)),
        ),
        popup,
    );
}
