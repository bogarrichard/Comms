//! Drawing. Pure function of `App` state; never touches the network.

use chrono::{Local, NaiveDate};
use comms_app::ConnectionStatus;
use comms_domain::{Conversation, ConversationKind, Message, SendState};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus, Mode};
use crate::keys::Action;
use crate::render::content_lines;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let [main, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    if area.width < 70 {
        // Small terminals: one panel at a time.
        match (app.focus, app.open.is_some()) {
            (Focus::Messages, true) => draw_chat(f, app, main),
            _ => draw_sidebar(f, app, main),
        }
    } else {
        let side = (area.width * 3 / 10).clamp(24, 42);
        let [left, right] =
            Layout::horizontal([Constraint::Length(side), Constraint::Min(10)]).areas(main);
        draw_sidebar(f, app, left);
        draw_chat(f, app, right);
    }
    draw_status(f, app, status);
    if app.mode == Mode::Help {
        draw_help(f, app, area);
    }
}

fn border(focused: bool) -> Style {
    Style::new().fg(if focused {
        Color::Cyan
    } else {
        Color::DarkGray
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
        .border_style(border(focused))
        .title(format!(" Comms · {} ", app.filter.label()));
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
                style = style.fg(Color::Cyan);
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
        let hl = Style::new().add_modifier(Modifier::REVERSED);
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

fn name_color(name: &str) -> Color {
    const PALETTE: [Color; 6] = [
        Color::Green,
        Color::Magenta,
        Color::Yellow,
        Color::Blue,
        Color::LightRed,
        Color::LightCyan,
    ];
    let h = name
        .bytes()
        .fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize));
    PALETTE[h % PALETTE.len()]
}

fn message_lines(msgs: &[Message], width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut last_day: Option<NaiveDate> = None;
    let body_w = width.saturating_sub(2).max(1);
    for m in msgs {
        let local = m.timestamp.with_timezone(&Local);
        if last_day != Some(local.date_naive()) {
            last_day = Some(local.date_naive());
            if !out.is_empty() {
                out.push(Line::raw(""));
            }
            out.push(
                Line::from(Span::styled(
                    format!("── {} ──", local.format("%a %e %b %Y")),
                    Style::new().fg(Color::DarkGray),
                ))
                .alignment(Alignment::Center),
            );
        }
        let name_style = if m.is_mine {
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::new()
                .fg(name_color(&m.sender.display_name))
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
                Style::new().fg(Color::DarkGray),
            ),
        ];
        if m.reply_to.is_some() {
            head.push(Span::styled("  ↳ reply", Style::new().fg(Color::DarkGray)));
        }
        if m.edited_at.is_some() {
            head.push(Span::styled("  (edited)", Style::new().fg(Color::DarkGray)));
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
        out.push(Line::from(head));

        let mut body: Vec<Line<'static>> = if m.deleted_at.is_some() {
            vec![Line::from(Span::styled(
                "(message deleted)",
                Style::new()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
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
        out.push(Line::raw(""));
    }
    out.pop();
    out
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
        .border_style(border(focused));
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
            let lines = message_lines(&app.messages, inner.width as usize);
            let h = inner.height as usize;
            app.max_scroll = lines.len().saturating_sub(h);
            app.scroll = app.scroll.min(app.max_scroll);
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
        .border_style(border(composing))
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
        Mode::Normal => (" NORMAL ", Color::Blue),
        Mode::Compose => (" INSERT ", Color::Green),
        Mode::Search => (" SEARCH ", Color::Yellow),
        Mode::Help => (" HELP ", Color::Magenta),
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
        Span::styled(spin, Style::new().fg(Color::Cyan)),
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
    match &app.toast {
        Some(t) => left.push(Span::styled(
            t.text.clone(),
            Style::new().fg(if t.error { Color::Red } else { Color::Cyan }),
        )),
        None => left.push(Span::styled(
            format!(
                "{} help · {} search · {} filter · {} quit",
                app.keymap.labels(Action::Help),
                app.keymap.labels(Action::Search),
                app.keymap.labels(Action::NextFilter),
                app.keymap.labels(Action::Quit),
            ),
            Style::new().fg(Color::DarkGray),
        )),
    }
    f.render_widget(Paragraph::new(Line::from(left)), left_area);
    f.render_widget(
        Paragraph::new(right).alignment(Alignment::Right),
        right_area,
    );
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
                .border_style(border(true)),
        ),
        popup,
    );
}
