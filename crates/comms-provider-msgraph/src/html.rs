//! Converts the HTML used by Teams message bodies into structural
//! [`ContentBlock`]s. Deliberately small: unknown markup degrades to its text.

use comms_domain::{ContentBlock, Inline, TextStyle};

/// Convert a Graph `itemBody`. `content_type` is `"text"` or `"html"`.
pub fn body_to_blocks(content_type: &str, content: &str) -> Vec<ContentBlock> {
    if content_type.eq_ignore_ascii_case("text") {
        return text_to_blocks(content);
    }
    html_to_blocks(content)
}

fn text_to_blocks(text: &str) -> Vec<ContentBlock> {
    let text = text.trim();
    if text.is_empty() {
        return vec![];
    }
    let mut inlines = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            inlines.push(Inline::Break);
        }
        inlines.push(Inline::text(line));
    }
    vec![ContentBlock::Paragraph { inlines }]
}

#[derive(Default)]
struct Builder {
    frames: Vec<Vec<ContentBlock>>,
    inlines: Vec<Inline>,
    bold: u32,
    italic: u32,
    strike: u32,
    in_list_item: bool,
    skip: u32,
    link: Option<(String, String)>,
    mention: Option<String>,
    code: Option<String>,
    pre: Option<String>,
}

impl Builder {
    fn new() -> Self {
        Self {
            frames: vec![vec![]],
            ..Self::default()
        }
    }

    fn style(&self) -> TextStyle {
        TextStyle {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
        }
    }

    fn push_text(&mut self, raw: &str) {
        if self.skip > 0 || raw.is_empty() {
            return;
        }
        if let Some(pre) = &mut self.pre {
            pre.push_str(raw);
            return;
        }
        let collapsed = collapse_whitespace(raw);
        if let Some(m) = &mut self.mention {
            m.push_str(&collapsed);
        } else if let Some((_, text)) = &mut self.link {
            text.push_str(&collapsed);
        } else if let Some(c) = &mut self.code {
            c.push_str(&collapsed);
        } else {
            self.push_inline_text(&collapsed);
        }
    }

    fn push_inline_text(&mut self, text: &str) {
        let mut text = text.to_owned();
        let at_line_start = matches!(self.inlines.last(), None | Some(Inline::Break));
        let after_space = matches!(
            self.inlines.last(),
            Some(Inline::Text { text: t, .. }) if t.ends_with(' ')
        );
        if (at_line_start || after_space) && text.starts_with(' ') {
            text.remove(0);
        }
        if text.is_empty() {
            return;
        }
        let style = self.style();
        if let Some(Inline::Text {
            text: prev,
            style: prev_style,
        }) = self.inlines.last_mut()
            && *prev_style == style
        {
            prev.push_str(&text);
            return;
        }
        self.inlines.push(Inline::Text { text, style });
    }

    fn push_inline(&mut self, inline: Inline) {
        if self.skip == 0 {
            self.inlines.push(inline);
        }
    }

    fn line_break(&mut self) {
        if self.skip > 0 {
            return;
        }
        if let Some(pre) = &mut self.pre {
            pre.push('\n');
        } else {
            self.inlines.push(Inline::Break);
        }
    }

    fn flush(&mut self) {
        // Trim trailing breaks / whitespace.
        while matches!(self.inlines.last(), Some(Inline::Break)) {
            self.inlines.pop();
        }
        if let Some(Inline::Text { text, .. }) = self.inlines.last_mut() {
            let trimmed = text.trim_end().len();
            text.truncate(trimmed);
            if text.is_empty() {
                self.inlines.pop();
            }
        }
        while matches!(self.inlines.first(), Some(Inline::Break)) {
            self.inlines.remove(0);
        }
        if self.inlines.is_empty() {
            return;
        }
        let inlines = std::mem::take(&mut self.inlines);
        let block = if self.in_list_item {
            ContentBlock::ListItem { inlines }
        } else {
            ContentBlock::Paragraph { inlines }
        };
        self.frames.last_mut().expect("frame").push(block);
    }

    fn finish(mut self) -> Vec<ContentBlock> {
        self.flush();
        while self.frames.len() > 1 {
            self.close_quote();
        }
        self.frames.pop().unwrap_or_default()
    }

    fn open_quote(&mut self) {
        self.flush();
        self.frames.push(vec![]);
    }

    fn close_quote(&mut self) {
        self.flush();
        if self.frames.len() > 1 {
            let blocks = self.frames.pop().unwrap_or_default();
            if !blocks.is_empty() {
                self.frames
                    .last_mut()
                    .expect("frame")
                    .push(ContentBlock::Quote { blocks });
            }
        }
    }

    fn open(&mut self, tag: &str, attrs: &Attrs) {
        match tag {
            "script" | "style" | "head" => self.skip += 1,
            "br" => self.line_break(),
            "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "table"
            | "tr" | "hr" => self.flush(),
            "li" => {
                self.flush();
                self.in_list_item = true;
            }
            "blockquote" => self.open_quote(),
            "b" | "strong" => self.bold += 1,
            "i" | "em" => self.italic += 1,
            "s" | "strike" | "del" => self.strike += 1,
            "a" => {
                if self.link.is_none() && self.skip == 0 {
                    self.link = Some((
                        attrs.get("href").unwrap_or_default().to_owned(),
                        String::new(),
                    ));
                }
            }
            "at" => {
                if self.skip == 0 {
                    self.mention = Some(String::new());
                }
            }
            "code" => {
                if self.pre.is_none() && self.skip == 0 {
                    self.code = Some(String::new());
                }
            }
            "pre" => {
                self.flush();
                self.pre = Some(String::new());
            }
            "emoji" => {
                if let Some(alt) = attrs.get("alt") {
                    self.push_text(alt);
                }
            }
            "img" => {
                let is_emoji = attrs.get("itemtype").is_some_and(|t| t.contains("Emoji"));
                match attrs.get("alt") {
                    Some(alt) if is_emoji && !alt.is_empty() => self.push_text(alt),
                    _ => self.push_inline(Inline::text("[image]")),
                }
            }
            _ => {}
        }
    }

    fn close(&mut self, tag: &str) {
        match tag {
            "script" | "style" | "head" => self.skip = self.skip.saturating_sub(1),
            "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "table"
            | "tr" => self.flush(),
            "li" => {
                self.flush();
                self.in_list_item = false;
            }
            "blockquote" => self.close_quote(),
            "b" | "strong" => self.bold = self.bold.saturating_sub(1),
            "i" | "em" => self.italic = self.italic.saturating_sub(1),
            "s" | "strike" | "del" => self.strike = self.strike.saturating_sub(1),
            "a" => {
                if let Some((url, text)) = self.link.take() {
                    let text = text.trim().to_owned();
                    if !(url.is_empty() && text.is_empty()) {
                        self.push_inline(Inline::Link { text, url });
                    }
                }
            }
            "at" => {
                if let Some(name) = self.mention.take() {
                    self.push_inline(Inline::Mention {
                        name: name.trim().to_owned(),
                    });
                }
            }
            "code" => {
                if let Some(text) = self.code.take()
                    && !text.is_empty()
                {
                    self.push_inline(Inline::Code { text });
                }
            }
            "pre" => {
                if let Some(text) = self.pre.take() {
                    let text = text.trim_matches('\n').to_owned();
                    if !text.is_empty() {
                        self.frames
                            .last_mut()
                            .expect("frame")
                            .push(ContentBlock::CodeBlock { text });
                    }
                }
            }
            _ => {}
        }
    }
}

struct Attrs(Vec<(String, String)>);

impl Attrs {
    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parse `name attr="v" attr2='v' attr3=v` into (lowercase name, attrs, self_closing).
fn parse_tag(inner: &str) -> (String, Attrs, bool) {
    let inner = inner.trim();
    let self_closing = inner.ends_with('/');
    let inner = inner.trim_end_matches('/').trim_end();
    let name_end = inner
        .find(|c: char| c.is_whitespace())
        .unwrap_or(inner.len());
    let name = inner[..name_end].to_ascii_lowercase();
    let mut attrs = Vec::new();
    let mut rest = inner[name_end..].trim_start();
    while !rest.is_empty() {
        let key_end = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(rest.len());
        let key = rest[..key_end].to_ascii_lowercase();
        rest = rest[key_end..].trim_start();
        let mut value = String::new();
        if let Some(r) = rest.strip_prefix('=') {
            let r = r.trim_start();
            if let Some(q) = r.chars().next().filter(|c| *c == '"' || *c == '\'') {
                let body = &r[1..];
                let end = body.find(q).unwrap_or(body.len());
                value = decode_entities(&body[..end]);
                rest = body.get(end + 1..).unwrap_or("").trim_start();
            } else {
                let end = r.find(char::is_whitespace).unwrap_or(r.len());
                value = decode_entities(&r[..end]);
                rest = r[end..].trim_start();
            }
        }
        if !key.is_empty() {
            attrs.push((key, value));
        }
    }
    (name, Attrs(attrs), self_closing)
}

pub fn html_to_blocks(html: &str) -> Vec<ContentBlock> {
    let mut b = Builder::new();
    let mut rest = html;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            b.push_text(&decode_entities(rest));
            break;
        };
        if lt > 0 {
            b.push_text(&decode_entities(&rest[..lt]));
        }
        rest = &rest[lt..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |e| &after[e + 3..]);
            continue;
        }
        // Find the closing '>' while respecting quoted attribute values.
        let mut quote: Option<char> = None;
        let mut end = None;
        for (i, ch) in rest.char_indices().skip(1) {
            match (quote, ch) {
                (Some(q), c) if c == q => quote = None,
                (None, '"' | '\'') => quote = Some(ch),
                (None, '>') => {
                    end = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            // A stray '<' with no closing '>': treat it as text.
            b.push_text(&decode_entities(rest));
            break;
        };
        let inner = &rest[1..end];
        rest = &rest[end + 1..];
        if let Some(closing) = inner.strip_prefix('/') {
            b.close(&closing.trim().to_ascii_lowercase());
        } else if inner.starts_with('!') || inner.starts_with('?') {
            // doctype / processing instruction
        } else {
            let (name, attrs, self_closing) = parse_tag(inner);
            b.open(&name, &attrs);
            if self_closing && !matches!(name.as_str(), "br" | "img" | "hr") {
                b.close(&name);
            }
        }
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use comms_domain::blocks_to_plain_text;

    fn plain(html: &str) -> String {
        blocks_to_plain_text(&html_to_blocks(html))
    }

    #[test]
    fn plain_text_bodies_keep_line_breaks() {
        let b = body_to_blocks("text", "line one\nline two");
        assert_eq!(blocks_to_plain_text(&b), "line one\nline two");
    }

    #[test]
    fn paragraphs_and_entities() {
        assert_eq!(
            plain("<p>Tom &amp; Jerry</p><p>a &lt; b &#39;x&#39; &#x41;</p>"),
            "Tom & Jerry\na < b 'x' A"
        );
    }

    #[test]
    fn nested_divs_do_not_create_blank_lines() {
        assert_eq!(plain("<div><div><div>hello</div></div></div>"), "hello");
    }

    #[test]
    fn br_inside_paragraph() {
        assert_eq!(plain("<p>a<br>b<br/>c</p>"), "a\nb\nc");
    }

    #[test]
    fn mentions_links_and_styles() {
        let blocks = html_to_blocks(
            r#"Hello <at id="0">Jane Smith</at>, see <a href="https://x.test/a?b=1&amp;c=2">this</a> <b>now</b> <i>please</i>"#,
        );
        let ContentBlock::Paragraph { inlines } = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert!(inlines.contains(&Inline::Mention {
            name: "Jane Smith".into()
        }));
        assert!(inlines.contains(&Inline::Link {
            text: "this".into(),
            url: "https://x.test/a?b=1&c=2".into()
        }));
        assert!(inlines.iter().any(|i| matches!(
            i,
            Inline::Text { text, style } if text == "now" && style.bold
        )));
        assert!(inlines.iter().any(|i| matches!(
            i,
            Inline::Text { text, style } if text == "please" && style.italic
        )));
        assert_eq!(
            blocks_to_plain_text(&blocks),
            "Hello @Jane Smith, see this now please"
        );
    }

    #[test]
    fn code_and_pre() {
        let blocks = html_to_blocks(
            "<p>run <code>cargo test</code></p><pre>fn main() {\n    1 &lt; 2\n}</pre>",
        );
        assert!(matches!(&blocks[0], ContentBlock::Paragraph { inlines }
            if inlines.contains(&Inline::Code { text: "cargo test".into() })));
        assert_eq!(
            blocks[1],
            ContentBlock::CodeBlock {
                text: "fn main() {\n    1 < 2\n}".into()
            }
        );
    }

    #[test]
    fn blockquote_becomes_quote_block() {
        let blocks = html_to_blocks("<blockquote><p>original</p></blockquote><p>reply</p>");
        assert!(matches!(&blocks[0], ContentBlock::Quote { blocks } if blocks.len() == 1));
        assert_eq!(blocks_to_plain_text(&blocks), "> original\nreply");
    }

    #[test]
    fn lists() {
        assert_eq!(plain("<ul><li>one</li><li>two</li></ul>"), "• one\n• two");
    }

    #[test]
    fn emoji_and_images() {
        assert_eq!(
            plain(
                r#"hi <emoji id="smile" alt="😀" title="Smile"></emoji> <img src="https://x/hostedContents/1/$value" alt="image">"#
            ),
            "hi 😀 [image]"
        );
        assert_eq!(
            plain(r#"<img itemtype="http://schema.skype.com/Emoji" alt="👍" src="x">"#),
            "👍"
        );
    }

    #[test]
    fn attachment_tags_scripts_and_comments_are_dropped() {
        assert_eq!(
            plain(
                r#"<!-- hidden --><script>alert(1)</script><p>text</p><attachment id="123"></attachment>"#
            ),
            "text"
        );
    }

    #[test]
    fn whitespace_is_collapsed_but_not_inside_pre() {
        assert_eq!(plain("<p>a \n\n   b</p>"), "a b");
    }

    #[test]
    fn malformed_html_never_panics() {
        for s in [
            "<",
            "<<>>",
            "<a href=\"x",
            "</b></i>",
            "&",
            "&#xZZ;",
            "<p><b>x</p>",
            "<br/><br/>",
            "",
        ] {
            let _ = html_to_blocks(s);
        }
        assert_eq!(plain("5 < 6"), "5 < 6");
    }

    #[test]
    fn quoted_gt_in_attributes() {
        assert_eq!(plain(r#"<a href="x" title="a>b">link</a>"#), "link");
    }
}
