//! Slack "mrkdwn" to structural [`ContentBlock`]s.
//!
//! Handles what matters for reading: ``` fences, `code`, `<url|label>` links,
//! `<@U123>` mentions (names supplied by the caller), `<#C123|name>` channels,
//! `<!here>`, bullet lines and `>` quotes. Bold, italic and strike are not
//! parsed yet; their markers stay in the text.

use std::collections::HashMap;

use comms_domain::{ContentBlock, Inline};

/// User ids mentioned as `<@U123>` in `text`, so the caller can resolve names.
pub fn mention_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<@") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('>') else { break };
        let id = after[..end].split('|').next().unwrap_or("");
        if !id.is_empty() && !ids.iter().any(|i| i == id) {
            ids.push(id.to_owned());
        }
        rest = &after[end + 1..];
    }
    ids
}

pub fn mrkdwn_to_blocks(text: &str, names: &HashMap<String, String>) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();
    for (i, segment) in text.split("```").enumerate() {
        if i % 2 == 1 {
            blocks.push(ContentBlock::CodeBlock {
                text: unescape(segment.trim_matches('\n')),
            });
            continue;
        }
        for line in segment.lines().filter(|l| !l.trim().is_empty()) {
            blocks.push(line_to_block(line, names));
        }
    }
    blocks
}

fn line_to_block(line: &str, names: &HashMap<String, String>) -> ContentBlock {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed
        .strip_prefix("&gt;")
        .or_else(|| trimmed.strip_prefix('>'))
    {
        return ContentBlock::Quote {
            blocks: vec![ContentBlock::Paragraph {
                inlines: inlines(rest.trim_start(), names),
            }],
        };
    }
    if let Some(rest) = trimmed
        .strip_prefix("• ")
        .or_else(|| trimmed.strip_prefix("- "))
    {
        return ContentBlock::ListItem {
            inlines: inlines(rest, names),
        };
    }
    ContentBlock::Paragraph {
        inlines: inlines(line, names),
    }
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn flush(buf: &mut String, out: &mut Vec<Inline>) {
    if !buf.is_empty() {
        out.push(Inline::text(unescape(buf)));
        buf.clear();
    }
}

fn inlines(line: &str, names: &HashMap<String, String>) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        match c {
            '<' => {
                if let Some(end) = rest.find('>') {
                    flush(&mut buf, &mut out);
                    out.push(angle(&rest[1..end], names));
                    rest = &rest[end + 1..];
                    continue;
                }
            }
            '`' => {
                if let Some(end) = rest[1..].find('`').filter(|e| *e > 0) {
                    flush(&mut buf, &mut out);
                    out.push(Inline::Code {
                        text: unescape(&rest[1..=end]),
                    });
                    rest = &rest[end + 2..];
                    continue;
                }
            }
            _ => {}
        }
        buf.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut buf, &mut out);
    out
}

/// The inside of `<...>`.
fn angle(token: &str, names: &HashMap<String, String>) -> Inline {
    let (target, label) = match token.split_once('|') {
        Some((t, l)) => (t, Some(l)),
        None => (token, None),
    };
    if let Some(id) = target.strip_prefix('@') {
        let name = names
            .get(id)
            .map(String::as_str)
            .or(label)
            .unwrap_or(id)
            .to_owned();
        Inline::Mention { name }
    } else if let Some(id) = target.strip_prefix('#') {
        Inline::text(format!("#{}", label.unwrap_or(id)))
    } else if let Some(special) = target.strip_prefix('!') {
        let name = label.unwrap_or(special.split('^').next().unwrap_or(special));
        Inline::Mention {
            name: name.to_owned(),
        }
    } else if target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
    {
        Inline::Link {
            text: unescape(label.unwrap_or(target)),
            url: unescape(target),
        }
    } else {
        Inline::text(format!("<{}>", unescape(token)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> HashMap<String, String> {
        HashMap::from([("U1".to_owned(), "Alice".to_owned())])
    }

    fn para(inlines: Vec<Inline>) -> ContentBlock {
        ContentBlock::Paragraph { inlines }
    }

    #[test]
    fn plain_text_and_entities() {
        assert_eq!(
            mrkdwn_to_blocks("a &lt; b &amp;&amp; c &gt; d", &names()),
            vec![para(vec![Inline::text("a < b && c > d")])]
        );
    }

    #[test]
    fn mentions_use_resolved_names_then_label_then_id() {
        let b = mrkdwn_to_blocks("hi <@U1> <@U2|bob> <@U3>", &names());
        assert_eq!(
            b,
            vec![para(vec![
                Inline::text("hi "),
                Inline::Mention {
                    name: "Alice".into()
                },
                Inline::text(" "),
                Inline::Mention { name: "bob".into() },
                Inline::text(" "),
                Inline::Mention { name: "U3".into() },
            ])]
        );
        assert_eq!(mention_ids("<@U1> and <@U2|bob> and <@U1>"), ["U1", "U2"]);
    }

    #[test]
    fn links_channels_and_specials() {
        let b = mrkdwn_to_blocks(
            "<https://a.example/x?a=1&amp;b=2|docs> <https://b.example> <#C1|general> <!here>",
            &names(),
        );
        assert_eq!(
            b,
            vec![para(vec![
                Inline::Link {
                    text: "docs".into(),
                    url: "https://a.example/x?a=1&b=2".into()
                },
                Inline::text(" "),
                Inline::Link {
                    text: "https://b.example".into(),
                    url: "https://b.example".into()
                },
                Inline::text(" "),
                Inline::text("#general"),
                Inline::text(" "),
                Inline::Mention {
                    name: "here".into()
                },
            ])]
        );
    }

    #[test]
    fn code_spans_fences_lists_and_quotes() {
        let b = mrkdwn_to_blocks(
            "run `ls -l` now\n• one\n&gt; quoted\n```\nfn main() {}\n```\nafter",
            &names(),
        );
        assert_eq!(
            b,
            vec![
                para(vec![
                    Inline::text("run "),
                    Inline::Code {
                        text: "ls -l".into()
                    },
                    Inline::text(" now"),
                ]),
                ContentBlock::ListItem {
                    inlines: vec![Inline::text("one")]
                },
                ContentBlock::Quote {
                    blocks: vec![para(vec![Inline::text("quoted")])]
                },
                ContentBlock::CodeBlock {
                    text: "fn main() {}".into()
                },
                para(vec![Inline::text("after")]),
            ]
        );
    }

    #[test]
    fn unbalanced_markers_stay_literal() {
        assert_eq!(
            mrkdwn_to_blocks("1 < 2 and `open", &names()),
            vec![para(vec![Inline::text("1 < 2 and `open")])]
        );
    }
}
