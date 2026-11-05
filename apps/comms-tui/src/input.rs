//! Small multi-line text editor for the message composer.

use unicode_width::UnicodeWidthChar;

#[derive(Debug, Default, Clone)]
pub struct InputBuffer {
    chars: Vec<char>,
    cursor: usize,
}

impl InputBuffer {
    pub fn is_blank(&self) -> bool {
        self.chars.iter().all(|c| c.is_whitespace())
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
    }

    /// Take the content, leaving the buffer empty.
    pub fn take(&mut self) -> String {
        let t = self.text();
        self.clear();
        t
    }

    pub fn insert_char(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars().filter(|c| *c != '\r') {
            self.insert_char(c);
        }
    }

    pub fn newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.chars.len());
    }

    fn line_start(&self, from: usize) -> usize {
        self.chars[..from]
            .iter()
            .rposition(|c| *c == '\n')
            .map_or(0, |i| i + 1)
    }

    fn line_end(&self, from: usize) -> usize {
        self.chars[from..]
            .iter()
            .position(|c| *c == '\n')
            .map_or(self.chars.len(), |i| from + i)
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start(self.cursor);
    }

    pub fn end(&mut self) {
        self.cursor = self.line_end(self.cursor);
    }

    pub fn up(&mut self) {
        let start = self.line_start(self.cursor);
        if start == 0 {
            self.cursor = 0;
            return;
        }
        let col = self.cursor - start;
        let prev_start = self.line_start(start - 1);
        self.cursor = (prev_start + col).min(start - 1);
    }

    pub fn down(&mut self) {
        let end = self.line_end(self.cursor);
        if end >= self.chars.len() {
            self.cursor = self.chars.len();
            return;
        }
        let col = self.cursor - self.line_start(self.cursor);
        let next_start = end + 1;
        self.cursor = (next_start + col).min(self.line_end(next_start));
    }

    /// Visual lines wrapped to `width` columns and the cursor's (row, column).
    pub fn layout(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(1);
        let mut lines = vec![String::new()];
        let mut col = 0usize;
        let mut cursor_pos = (0, 0);
        for (i, c) in self.chars.iter().enumerate() {
            if i == self.cursor {
                cursor_pos = (lines.len() - 1, col);
            }
            if *c == '\n' {
                lines.push(String::new());
                col = 0;
                continue;
            }
            let w = c.width().unwrap_or(0);
            if col + w > width {
                lines.push(String::new());
                col = 0;
                if i == self.cursor {
                    cursor_pos = (lines.len() - 1, 0);
                }
            }
            lines.last_mut().expect("line").push(*c);
            col += w;
        }
        if self.cursor == self.chars.len() {
            if col >= width {
                lines.push(String::new());
                cursor_pos = (lines.len() - 1, 0);
            } else {
                cursor_pos = (lines.len() - 1, col);
            }
        }
        (lines, cursor_pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(s: &str) -> InputBuffer {
        let mut b = InputBuffer::default();
        b.insert_str(s);
        b
    }

    #[test]
    fn typing_editing_and_taking() {
        let mut b = buf("helo");
        b.left();
        b.insert_char('l');
        assert_eq!(b.text(), "hello");
        b.backspace();
        b.delete();
        assert_eq!(b.text(), "hel");
        assert_eq!(b.take(), "hel");
        assert!(b.is_blank());
    }

    #[test]
    fn multiline_navigation() {
        let mut b = buf("abc\nde\nfghij");
        b.up();
        assert_eq!(b.layout(80).1, (1, 2));
        b.up();
        assert_eq!(b.layout(80).1, (0, 2));
        b.down();
        b.down();
        assert_eq!(b.layout(80).1, (2, 2));
        b.home();
        assert_eq!(b.layout(80).1, (2, 0));
        b.end();
        assert_eq!(b.layout(80).1, (2, 5));
    }

    #[test]
    fn wrapping_and_cursor_at_wrap_boundary() {
        let b = buf("abcdef");
        let (lines, cur) = b.layout(3);
        assert_eq!(lines, ["abc", "def", ""]);
        assert_eq!(cur, (2, 0));
        let (lines, cur) = buf("abcd").layout(3);
        assert_eq!(lines, ["abc", "d"]);
        assert_eq!(cur, (1, 1));
    }

    #[test]
    fn wide_characters_count_two_columns() {
        let (lines, cur) = buf("日本語").layout(4);
        assert_eq!(lines, ["日本", "語"]);
        assert_eq!(cur, (1, 2));
    }

    #[test]
    fn paste_normalises_line_endings_and_blank_detection() {
        let b = buf("a\r\nb");
        assert_eq!(b.text(), "a\nb");
        assert!(buf("  \n ").is_blank());
        assert!(!buf(" x ").is_blank());
    }

    #[test]
    fn edge_cases_never_panic() {
        let mut b = InputBuffer::default();
        b.backspace();
        b.delete();
        b.left();
        b.up();
        b.down();
        b.home();
        b.end();
        assert_eq!(b.layout(0).0, [""]);
    }
}
