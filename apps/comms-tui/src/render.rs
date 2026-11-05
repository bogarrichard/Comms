//! Turns structural message content into word-wrapped, styled terminal lines.

use comms_domain::{ContentBlock, Inline, TextStyle};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

enum Tok {
    Word(String, Style),
    Space(Style),
    Break,
}

fn text_style(s: TextStyle) -> Style {
    let mut st = Style::new();
    if s.bold {
        st = st.add_modifier(Modifier::BOLD);
    }
    if s.italic {
        st = st.add_modifier(Modifier::ITALIC);
    }
    if s.strike {
        st = st.add_modifier(Modifier::CROSSED_OUT);
    }
    st
}

fn push_words(toks: &mut Vec<Tok>, text: &str, style: Style) {
    let mut word = String::new();
    for c in text.chars() {
        if c == '\n' {
            if !word.is_empty() {
                toks.push(Tok::Word(std::mem::take(&mut word), style));
            }
            toks.push(Tok::Break);
        } else if c.is_whitespace() {
            if !word.is_empty() {
                toks.push(Tok::Word(std::mem::take(&mut word), style));
            }
            toks.push(Tok::Space(style));
        } else if c.is_control() {
            // Never pass control characters (terminal escapes) through.
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        toks.push(Tok::Word(word, style));
    }
}

fn inline_tokens(inlines: &[Inline]) -> Vec<Tok> {
    let mut toks = Vec::new();
    for i in inlines {
        match i {
            Inline::Text { text, style } => push_words(&mut toks, text, text_style(*style)),
            Inline::Mention { name } => push_words(
                &mut toks,
                &format!("@{name}"),
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Inline::Link { text, url } => {
                let shown = if text.is_empty() { url } else { text };
                push_words(
                    &mut toks,
                    shown,
                    Style::new()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::UNDERLINED),
                );
            }
            Inline::Code { text } => push_words(&mut toks, text, Style::new().fg(Color::Magenta)),
            Inline::Break => toks.push(Tok::Break),
        }
    }
    toks
}

/// Greedy word wrap preserving per-word styles. Overlong words are split.
fn wrap(toks: Vec<Tok>, width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    let mut lines: Vec<Vec<Span<'static>>> = vec![vec![]];
    let mut col = 0usize;
    let mut pending_space: Option<Style> = None;
    for t in toks {
        match t {
            Tok::Break => {
                lines.push(vec![]);
                col = 0;
                pending_space = None;
            }
            Tok::Space(s) => {
                if col > 0 {
                    pending_space = Some(s);
                }
            }
            Tok::Word(w, style) => {
                let ww = w.width();
                let space = usize::from(pending_space.is_some() && col > 0);
                if col > 0 && col + space + ww > width {
                    lines.push(vec![]);
                    col = 0;
                    pending_space = None;
                } else if let Some(s) = pending_space.take().filter(|_| col > 0) {
                    lines.last_mut().expect("line").push(Span::styled(" ", s));
                    col += 1;
                }
                if ww <= width {
                    lines.last_mut().expect("line").push(Span::styled(w, style));
                    col += ww;
                } else {
                    let mut chunk = String::new();
                    let mut cw = 0;
                    for ch in w.chars() {
                        let w1 = ch.width().unwrap_or(0);
                        if cw + w1 > width {
                            lines
                                .last_mut()
                                .expect("line")
                                .push(Span::styled(std::mem::take(&mut chunk), style));
                            lines.push(vec![]);
                            cw = 0;
                        }
                        chunk.push(ch);
                        cw += w1;
                    }
                    lines
                        .last_mut()
                        .expect("line")
                        .push(Span::styled(chunk, style));
                    col = cw;
                }
            }
        }
    }
    lines
}

fn prefixed(
    prefix: &str,
    prefix_style: Style,
    lines: Vec<Vec<Span<'static>>>,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|mut spans| {
            spans.insert(0, Span::styled(prefix.to_owned(), prefix_style));
            Line::from(spans)
        })
        .collect()
}

/// Render blocks into lines no wider than `width` columns.
pub fn content_lines(blocks: &[ContentBlock], width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for b in blocks {
        match b {
            ContentBlock::Paragraph { inlines } => {
                out.extend(
                    wrap(inline_tokens(inlines), width)
                        .into_iter()
                        .map(Line::from),
                );
            }
            ContentBlock::ListItem { inlines } => {
                let lines = wrap(inline_tokens(inlines), width.saturating_sub(2));
                for (i, mut spans) in lines.into_iter().enumerate() {
                    spans.insert(0, Span::raw(if i == 0 { "• " } else { "  " }));
                    out.push(Line::from(spans));
                }
            }
            ContentBlock::Quote { blocks } => {
                let inner = content_lines(blocks, width.saturating_sub(2));
                let dim = Style::new().fg(Color::DarkGray);
                for mut l in inner {
                    l.spans.insert(0, Span::styled("│ ", dim));
                    out.push(l);
                }
            }
            ContentBlock::CodeBlock { text } => {
                let style = Style::new().fg(Color::Magenta);
                for raw in text.lines() {
                    let toks = vec![Tok::Word(
                        raw.chars()
                            .filter(|c| !c.is_control() || *c == '\t')
                            .map(|c| if c == '\t' { ' ' } else { c })
                            .collect(),
                        style,
                    )];
                    out.extend(prefixed("  ", style, wrap(toks, width.saturating_sub(2))));
                }
            }
            ContentBlock::Unsupported { fallback, .. } => {
                let mut toks = Vec::new();
                push_words(
                    &mut toks,
                    fallback,
                    Style::new().add_modifier(Modifier::DIM),
                );
                out.extend(wrap(toks, width).into_iter().map(Line::from));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn para(s: &str) -> Vec<ContentBlock> {
        vec![ContentBlock::paragraph(s)]
    }

    #[test]
    fn words_wrap_at_the_width() {
        let l = content_lines(&para("the quick brown fox jumps"), 10);
        assert_eq!(text(&l), ["the quick", "brown fox", "jumps"]);
    }

    #[test]
    fn overlong_words_are_split_and_lines_never_exceed_width() {
        let l = content_lines(&para("abcdefghijklmnop xy"), 6);
        for line in text(&l) {
            assert!(line.width() <= 6, "{line:?}");
        }
        assert_eq!(text(&l).concat().replace(' ', ""), "abcdefghijklmnopxy");
    }

    #[test]
    fn explicit_breaks_and_lists_and_quotes() {
        let blocks = vec![
            ContentBlock::Paragraph {
                inlines: vec![Inline::text("a"), Inline::Break, Inline::text("b")],
            },
            ContentBlock::ListItem {
                inlines: vec![Inline::text("one two three")],
            },
            ContentBlock::Quote {
                blocks: vec![ContentBlock::paragraph("quoted")],
            },
        ];
        assert_eq!(
            text(&content_lines(&blocks, 8)),
            ["a", "b", "• one", "  two", "  three", "│ quoted"]
        );
    }

    #[test]
    fn styles_survive_wrapping() {
        let blocks = vec![ContentBlock::Paragraph {
            inlines: vec![
                Inline::Mention { name: "Ann".into() },
                Inline::Text {
                    text: " bold".into(),
                    style: TextStyle {
                        bold: true,
                        ..Default::default()
                    },
                },
            ],
        }];
        let l = content_lines(&blocks, 40);
        assert_eq!(text(&l), ["@Ann bold"]);
        assert!(l[0].spans.iter().any(|s| s.style.fg == Some(Color::Yellow)));
        assert!(
            l[0].spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn control_characters_are_stripped() {
        let l = content_lines(&para("hi\u{1b}[31m red\u{7}"), 40);
        assert!(!text(&l).concat().contains('\u{1b}'));
        assert!(!text(&l).concat().contains('\u{7}'));
    }

    #[test]
    fn code_blocks_keep_their_lines() {
        let l = content_lines(
            &[ContentBlock::CodeBlock {
                text: "fn a() {\n    1\n}".into(),
            }],
            30,
        );
        assert_eq!(text(&l).len(), 3);
    }

    #[test]
    fn tiny_widths_do_not_panic_or_loop() {
        for w in 0..4 {
            let _ = content_lines(&para("日本語 text with words"), w);
        }
    }
}
