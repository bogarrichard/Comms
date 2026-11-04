//! Structural rich text. Providers convert their native format (HTML for
//! Teams) into these blocks; frontends render them.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextStyle {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Inline {
    Text {
        text: String,
        style: TextStyle,
    },
    Mention {
        name: String,
    },
    Link {
        text: String,
        url: String,
    },
    Code {
        text: String,
    },
    /// Hard line break inside a paragraph.
    Break,
}

impl Inline {
    pub fn text(s: impl Into<String>) -> Self {
        Inline::Text {
            text: s.into(),
            style: TextStyle::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Paragraph {
        inlines: Vec<Inline>,
    },
    ListItem {
        inlines: Vec<Inline>,
    },
    Quote {
        blocks: Vec<ContentBlock>,
    },
    CodeBlock {
        text: String,
    },
    /// Content the provider could not map; `fallback` is always shown.
    Unsupported {
        kind: String,
        fallback: String,
    },
}

impl ContentBlock {
    pub fn paragraph(text: impl Into<String>) -> Self {
        ContentBlock::Paragraph {
            inlines: vec![Inline::text(text)],
        }
    }
}

fn inlines_to_plain(inlines: &[Inline], out: &mut String) {
    for i in inlines {
        match i {
            Inline::Text { text, .. } | Inline::Code { text } => out.push_str(text),
            Inline::Mention { name } => {
                out.push('@');
                out.push_str(name);
            }
            Inline::Link { text, url } => {
                if text.is_empty() || text == url {
                    out.push_str(url);
                } else {
                    out.push_str(text);
                }
            }
            Inline::Break => out.push('\n'),
        }
    }
}

/// Plain-text rendering used for previews, notifications and search indexing.
pub fn blocks_to_plain_text(blocks: &[ContentBlock]) -> String {
    let mut out = String::new();
    for b in blocks {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        match b {
            ContentBlock::Paragraph { inlines } => inlines_to_plain(inlines, &mut out),
            ContentBlock::ListItem { inlines } => {
                out.push_str("• ");
                inlines_to_plain(inlines, &mut out);
            }
            ContentBlock::Quote { blocks } => {
                for line in blocks_to_plain_text(blocks).lines() {
                    out.push_str("> ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
            ContentBlock::CodeBlock { text } => out.push_str(text),
            ContentBlock::Unsupported { fallback, .. } => out.push_str(fallback),
        }
    }
    out.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_joins_blocks_and_mentions() {
        let blocks = vec![
            ContentBlock::Paragraph {
                inlines: vec![
                    Inline::Mention { name: "Ann".into() },
                    Inline::text(" hi"),
                    Inline::Break,
                    Inline::Link {
                        text: "site".into(),
                        url: "https://x.y".into(),
                    },
                ],
            },
            ContentBlock::ListItem {
                inlines: vec![Inline::text("one")],
            },
            ContentBlock::Quote {
                blocks: vec![ContentBlock::paragraph("quoted")],
            },
        ];
        assert_eq!(
            blocks_to_plain_text(&blocks),
            "@Ann hi\nsite\n• one\n> quoted"
        );
    }

    #[test]
    fn link_without_text_shows_url() {
        let b = vec![ContentBlock::Paragraph {
            inlines: vec![Inline::Link {
                text: String::new(),
                url: "https://a.b".into(),
            }],
        }];
        assert_eq!(blocks_to_plain_text(&b), "https://a.b");
    }

    #[test]
    fn blocks_serde_round_trip() {
        let blocks = vec![
            ContentBlock::CodeBlock {
                text: "let x = 1;".into(),
            },
            ContentBlock::Unsupported {
                kind: "card".into(),
                fallback: "[card]".into(),
            },
        ];
        let json = serde_json::to_string(&blocks).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<ContentBlock>>(&json).unwrap(),
            blocks
        );
    }

    #[test]
    fn empty_content_is_empty_string() {
        assert_eq!(blocks_to_plain_text(&[]), "");
    }
}
