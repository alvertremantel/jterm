use std::io::Write;
use std::path::Path;

use color_eyre::eyre::{Result, WrapErr};
use ropey::Rope;
use tempfile::NamedTempFile;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone)]
pub enum BufferOp {
    Insert {
        pos: usize,
        text: String,
    },
    Delete {
        pos: usize,
        text: String,
    },
    Replace {
        pos: usize,
        old_text: String,
        new_text: String,
    },
}

impl BufferOp {
    fn inverse(&self) -> BufferOp {
        match self {
            BufferOp::Insert { pos, text } => BufferOp::Delete {
                pos: *pos,
                text: text.clone(),
            },
            BufferOp::Delete { pos, text } => BufferOp::Insert {
                pos: *pos,
                text: text.clone(),
            },
            BufferOp::Replace {
                pos,
                old_text,
                new_text,
            } => BufferOp::Replace {
                pos: *pos,
                old_text: new_text.clone(),
                new_text: old_text.clone(),
            },
        }
    }
}

pub struct RopeBuffer {
    rope: Rope,
    undo_stack: Vec<BufferOp>,
    redo_stack: Vec<BufferOp>,
    dirty: bool,
    save_point: usize,
    version: u64,
}

impl Default for RopeBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl RopeBuffer {
    pub fn new() -> Self {
        Self {
            rope: Rope::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            dirty: false,
            save_point: 0,
            version: 0,
        }
    }

    pub fn from_text(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            dirty: false,
            save_point: 0,
            version: 0,
        }
    }

    pub fn save_file(&mut self, path: &Path) -> Result<()> {
        let content = self.rope.to_string();
        let dir = path.parent().unwrap_or(Path::new("."));
        let mut tmp = NamedTempFile::new_in(dir)
            .wrap_err_with(|| format!("Failed to create temp file in {}", dir.display()))?;
        tmp.write_all(content.as_bytes())
            .wrap_err("Failed to write buffer to temp file")?;
        tmp.flush().wrap_err("Failed to flush temp file")?;
        tmp.persist(path)
            .wrap_err_with(|| format!("Failed to persist temp file to {}", path.display()))?;
        self.dirty = false;
        self.save_point = self.undo_stack.len();
        Ok(())
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn insert_char(&mut self, pos: usize, ch: char) {
        let pos = pos.min(self.rope.len_chars());
        let text = ch.to_string();
        self.apply(BufferOp::Insert { pos, text });
    }

    pub fn insert_str(&mut self, pos: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        let pos = pos.min(self.rope.len_chars());
        self.apply(BufferOp::Insert {
            pos,
            text: text.to_string(),
        });
    }

    pub fn delete_range(&mut self, pos: usize, len: usize) -> String {
        if len == 0 {
            return String::new();
        }
        let pos = pos.min(self.rope.len_chars());
        let end = (pos + len).min(self.rope.len_chars());
        if pos >= end {
            return String::new();
        }
        let deleted: String = self.rope.slice(pos..end).to_string();
        self.apply(BufferOp::Delete {
            pos,
            text: deleted.clone(),
        });
        deleted
    }

    pub fn replace_range(&mut self, pos: usize, len: usize, replacement: &str) {
        let pos = pos.min(self.rope.len_chars());
        let end = (pos + len).min(self.rope.len_chars());
        if pos >= end {
            if !replacement.is_empty() {
                self.insert_str(pos, replacement);
            }
            return;
        }
        let old_text: String = self.rope.slice(pos..end).to_string();
        if old_text == replacement {
            return;
        }
        self.apply(BufferOp::Replace {
            pos,
            old_text,
            new_text: replacement.to_string(),
        });
    }

    pub fn undo(&mut self) -> bool {
        if let Some(op) = self.undo_stack.pop() {
            let inverse = op.inverse();
            self.apply_raw(&inverse);
            self.redo_stack.push(op);
            self.version += 1;
            self.update_dirty();
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some(op) = self.redo_stack.pop() {
            self.apply_raw(&op);
            self.undo_stack.push(op);
            self.version += 1;
            self.update_dirty();
            true
        } else {
            false
        }
    }

    fn apply(&mut self, op: BufferOp) {
        self.apply_raw(&op);
        self.undo_stack.push(op);
        self.redo_stack.clear();
        self.version += 1;
        self.mark_dirty();
    }

    fn apply_raw(&mut self, op: &BufferOp) {
        match op {
            BufferOp::Insert { pos, text } => {
                self.rope.insert(*pos, text);
            }
            BufferOp::Delete { pos, text } => {
                let end = pos + text.chars().count();
                self.rope.remove(*pos..end);
            }
            BufferOp::Replace {
                pos,
                old_text,
                new_text,
            } => {
                let end = pos + old_text.chars().count();
                self.rope.remove(*pos..end);
                if !new_text.is_empty() {
                    self.rope.insert(*pos, new_text);
                }
            }
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn update_dirty(&mut self) {
        self.dirty = self.undo_stack.len() != self.save_point;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub anchor_line: usize,
    pub anchor_col: usize,
    pub head_line: usize,
    pub head_col: usize,
}

impl Selection {
    pub fn normalized(&self) -> ((usize, usize), (usize, usize)) {
        let start = (self.anchor_line, self.anchor_col);
        let end = (self.head_line, self.head_col);
        if start <= end {
            (start, end)
        } else {
            (end, start)
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct EditorState {
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub scroll_offset: usize,
    pub selection: Option<Selection>,
    pub desired_col: usize,
}

impl EditorState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn line_len_for(rope: &Rope, idx: usize) -> usize {
        Self::line_len(rope, idx)
    }

    fn line_len(rope: &Rope, idx: usize) -> usize {
        if idx >= rope.len_lines() {
            return 0;
        }
        let line = rope.line(idx);
        let len = line.len_chars();
        if len > 0 && line.char(len - 1) == '\n' {
            if len > 1 && line.char(len - 2) == '\r' {
                len - 2
            } else {
                len - 1
            }
        } else {
            len
        }
    }

    fn line_count(rope: &Rope) -> usize {
        rope.len_lines().max(1)
    }

    fn clamp_to_buffer(&mut self, rope: &Rope) {
        let max_line = Self::line_count(rope).saturating_sub(1);
        self.cursor_line = self.cursor_line.min(max_line);
        let max_col = Self::line_len(rope, self.cursor_line);
        self.cursor_col = self.cursor_col.min(max_col);
    }

    pub fn move_cursor(&mut self, dir: Direction, rope: &Rope) {
        match dir {
            Direction::Up => {
                if self.cursor_line > 0 {
                    self.cursor_line -= 1;
                    let max_col = Self::line_len(rope, self.cursor_line);
                    self.cursor_col = self.desired_col.min(max_col);
                    if self.cursor_col < max_col {
                        let line_text: String = rope.line(self.cursor_line).chars().collect();
                        let content = line_text.trim_end_matches(&['\n', '\r'][..]);
                        let next = next_grapheme_boundary(content, self.cursor_col);
                        self.cursor_col = prev_grapheme_boundary(content, next);
                    }
                }
            }
            Direction::Down => {
                let max_line = Self::line_count(rope).saturating_sub(1);
                if self.cursor_line < max_line {
                    self.cursor_line += 1;
                    let max_col = Self::line_len(rope, self.cursor_line);
                    self.cursor_col = self.desired_col.min(max_col);
                    if self.cursor_col < max_col {
                        let line_text: String = rope.line(self.cursor_line).chars().collect();
                        let content = line_text.trim_end_matches(&['\n', '\r'][..]);
                        let next = next_grapheme_boundary(content, self.cursor_col);
                        self.cursor_col = prev_grapheme_boundary(content, next);
                    }
                }
            }
            Direction::Left => {
                if self.cursor_col > 0 {
                    let line_text: String = rope.line(self.cursor_line).chars().collect();
                    let content = line_text.trim_end_matches(&['\n', '\r'][..]);
                    self.cursor_col = prev_grapheme_boundary(content, self.cursor_col);
                } else if self.cursor_line > 0 {
                    self.cursor_line -= 1;
                    self.cursor_col = Self::line_len(rope, self.cursor_line);
                }
                self.desired_col = self.cursor_col;
            }
            Direction::Right => {
                let line_len = Self::line_len(rope, self.cursor_line);
                if self.cursor_col < line_len {
                    let line_text: String = rope.line(self.cursor_line).chars().collect();
                    let content = line_text.trim_end_matches(&['\n', '\r'][..]);
                    self.cursor_col = next_grapheme_boundary(content, self.cursor_col);
                } else {
                    let max_line = Self::line_count(rope).saturating_sub(1);
                    if self.cursor_line < max_line {
                        self.cursor_line += 1;
                        self.cursor_col = 0;
                    }
                }
                self.desired_col = self.cursor_col;
            }
        }
    }

    pub fn move_cursor_to(&mut self, line: usize, col: usize, rope: &Rope) {
        self.cursor_line = line;
        self.cursor_col = col;
        self.clamp_to_buffer(rope);
        self.desired_col = self.cursor_col;
    }

    pub fn ensure_cursor_visible(&mut self, viewport_height: usize) {
        if viewport_height == 0 {
            return;
        }
        if self.cursor_line < self.scroll_offset {
            self.scroll_offset = self.cursor_line;
        }
        let bottom = self.scroll_offset + viewport_height.saturating_sub(1);
        if self.cursor_line > bottom {
            self.scroll_offset = self
                .cursor_line
                .saturating_sub(viewport_height.saturating_sub(1));
        }
    }

    pub fn home(&mut self) {
        self.cursor_col = 0;
        self.desired_col = 0;
    }

    pub fn end(&mut self, rope: &Rope) {
        self.cursor_col = Self::line_len(rope, self.cursor_line);
        self.desired_col = self.cursor_col;
    }

    pub fn page_up(&mut self, viewport_height: usize, rope: &Rope) {
        let jump = viewport_height.saturating_sub(1).max(1);
        self.cursor_line = self.cursor_line.saturating_sub(jump);
        let max_col = Self::line_len(rope, self.cursor_line);
        self.cursor_col = self.desired_col.min(max_col);
        self.ensure_cursor_visible(viewport_height);
        self.scroll_offset = self.scroll_offset.saturating_sub(jump);
    }

    pub fn page_down(&mut self, viewport_height: usize, rope: &Rope) {
        let jump = viewport_height.saturating_sub(1).max(1);
        let max_line = Self::line_count(rope).saturating_sub(1);
        self.cursor_line = (self.cursor_line + jump).min(max_line);
        let max_col = Self::line_len(rope, self.cursor_line);
        self.cursor_col = self.desired_col.min(max_col);
        self.ensure_cursor_visible(viewport_height);
    }

    pub fn start_selection(&mut self) {
        self.selection = Some(Selection {
            anchor_line: self.cursor_line,
            anchor_col: self.cursor_col,
            head_line: self.cursor_line,
            head_col: self.cursor_col,
        });
    }

    pub fn extend_selection(&mut self) {
        if let Some(sel) = &mut self.selection {
            sel.head_line = self.cursor_line;
            sel.head_col = self.cursor_col;
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    pub fn selected_char_range(&self, rope: &Rope) -> Option<(usize, usize)> {
        let sel = self.selection.as_ref()?;
        let ((sl, sc), (el, ec)) = sel.normalized();
        let start_char = rope.line_to_char(sl) + sc;
        let end_char = rope.line_to_char(el) + ec;
        Some((start_char, end_char))
    }

    pub fn select_word(&mut self, rope: &Rope) {
        if self.cursor_line >= rope.len_lines() {
            return;
        }
        let line = rope.line(self.cursor_line);
        let line_len = Self::line_len(rope, self.cursor_line);
        if line_len == 0 {
            return;
        }

        let col = self.cursor_col.min(line_len.saturating_sub(1));
        let ch = line.char(col);
        let is_word_char = |c: char| c.is_alphanumeric() || c == '_';

        if !is_word_char(ch) {
            let line_text: String = line.chars().collect();
            let content = line_text.trim_end_matches(&['\n', '\r'][..]);
            let next = next_grapheme_boundary(content, col);
            self.selection = Some(Selection {
                anchor_line: self.cursor_line,
                anchor_col: col,
                head_line: self.cursor_line,
                head_col: next.min(line_len),
            });
            return;
        }

        let mut start = col;
        while start > 0 && is_word_char(line.char(start - 1)) {
            start -= 1;
        }

        let mut end = col;
        while end < line_len && is_word_char(line.char(end)) {
            end += 1;
        }

        self.selection = Some(Selection {
            anchor_line: self.cursor_line,
            anchor_col: start,
            head_line: self.cursor_line,
            head_col: end,
        });
        self.cursor_col = end;
        self.desired_col = end;
    }
}

pub fn screen_to_buffer_pos(
    screen_col: u16,
    screen_row: u16,
    area_x: u16,
    area_y: u16,
    scroll_offset: usize,
    gutter_w: u16,
) -> (usize, usize) {
    let rel_row = screen_row.saturating_sub(area_y) as usize;
    let rel_col = screen_col.saturating_sub(area_x + gutter_w) as usize;
    let line = scroll_offset + rel_row;
    (line, rel_col)
}

pub fn buffer_to_screen_pos(
    line: usize,
    col: usize,
    scroll_offset: usize,
    gutter_w: u16,
    area_x: u16,
    area_y: u16,
    area_height: u16,
) -> Option<(u16, u16)> {
    if line < scroll_offset {
        return None;
    }
    let rel_line = line - scroll_offset;
    if rel_line >= area_height as usize {
        return None;
    }
    let sx = area_x + gutter_w + col as u16;
    let sy = area_y + rel_line as u16;
    Some((sx, sy))
}

pub fn gutter_width(total_lines: usize) -> u16 {
    let digits = if total_lines == 0 {
        1
    } else {
        ((total_lines as f64).log10().floor() as u16) + 1
    };
    digits.max(3) + 1
}

/// Compute the display width of a string by summing the terminal cell width
/// of each Unicode grapheme cluster. This is the single source of truth for
/// display-width computation across the workspace — it correctly handles
/// CJK wide characters, ZWJ emoji sequences, and combining marks.
pub fn grapheme_display_width(s: &str) -> usize {
    s.graphemes(true).map(UnicodeWidthStr::width).sum()
}

pub fn char_col_to_display_col(line_text: &str, char_col: usize) -> usize {
    line_text
        .chars()
        .take(char_col)
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

pub fn display_col_to_char_col(line_text: &str, display_col: usize) -> usize {
    let mut accum = 0;
    let mut char_pos = 0;
    for grapheme in line_text.graphemes(true) {
        let g_width = grapheme_display_width(grapheme);
        if accum >= display_col {
            return char_pos;
        }
        accum += g_width;
        char_pos += grapheme.chars().count();
    }
    char_pos
}

pub fn truncate_to_display_width(s: &str, max_width: usize) -> &str {
    let mut width = 0;
    for (i, c) in s.char_indices() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if width + cw > max_width {
            return &s[..i];
        }
        width += cw;
    }
    s
}

pub fn nth_char_byte_offset(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len())
}

pub fn prev_grapheme_boundary(line_text: &str, char_col: usize) -> usize {
    let mut prev = 0;
    let mut current = 0;
    for grapheme in line_text.graphemes(true) {
        current += grapheme.chars().count();
        if current >= char_col {
            return prev;
        }
        prev = current;
    }
    prev
}

pub fn next_grapheme_boundary(line_text: &str, char_col: usize) -> usize {
    let mut current = 0;
    for grapheme in line_text.graphemes(true) {
        let next = current + grapheme.chars().count();
        if current >= char_col && next > char_col {
            return next;
        }
        current = next;
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_delete_replace_round_trip() {
        let mut buffer = RopeBuffer::from_text("abc");
        buffer.insert_str(3, "def");
        assert_eq!(buffer.text(), "abcdef");
        assert_eq!(buffer.delete_range(1, 2), "bc");
        assert_eq!(buffer.text(), "adef");
        buffer.replace_range(1, 2, "ZZ");
        assert_eq!(buffer.text(), "aZZf");
    }

    #[test]
    fn replace_range_undo_is_atomic() {
        let mut buffer = RopeBuffer::from_text("hello world");
        buffer.replace_range(0, 5, "goodbye");

        assert_eq!(buffer.text(), "goodbye world");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "hello world");
    }

    #[test]
    fn replace_range_redo_reapplies_change() {
        let mut buffer = RopeBuffer::from_text("hello world");
        buffer.replace_range(0, 5, "goodbye");
        buffer.undo();

        assert!(buffer.redo());
        assert_eq!(buffer.text(), "goodbye world");
    }

    #[test]
    fn save_file_clears_dirty_and_persists_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        let mut buffer = RopeBuffer::from_text("hello");
        buffer.insert_str(5, " world");
        assert!(buffer.is_dirty());

        buffer.save_file(&path).unwrap();

        assert_eq!(std::fs::read_to_string(path).unwrap(), "hello world");
        assert!(!buffer.is_dirty());
    }

    #[test]
    fn cursor_movement_and_visibility_work() {
        let rope = Rope::from_str("hello\nworld\n!");
        let mut state = EditorState::new();
        state.move_cursor(Direction::Down, &rope);
        state.move_cursor(Direction::Right, &rope);
        state.move_cursor(Direction::Right, &rope);
        assert_eq!((state.cursor_line, state.cursor_col), (1, 2));

        state.cursor_line = 5;
        state.cursor_col = 99;
        state.move_cursor_to(5, 99, &rope);
        assert_eq!((state.cursor_line, state.cursor_col), (2, 1));

        state.cursor_line = 10;
        state.ensure_cursor_visible(3);
        assert_eq!(state.scroll_offset, 8);
    }

    #[test]
    fn selection_normalization_and_char_range_are_stable() {
        let rope = Rope::from_str("abc\ndef\n");
        let state = EditorState {
            selection: Some(Selection {
                anchor_line: 1,
                anchor_col: 2,
                head_line: 0,
                head_col: 1,
            }),
            ..EditorState::default()
        };
        assert_eq!(
            state.selection.as_ref().unwrap().normalized(),
            ((0, 1), (1, 2))
        );
        assert_eq!(state.selected_char_range(&rope), Some((1, 6)));
    }

    #[test]
    fn unicode_width_helpers_and_grapheme_boundaries_work() {
        let text = "a界👍b";
        assert_eq!(char_col_to_display_col(text, 2), 3);
        assert_eq!(display_col_to_char_col(text, 3), 2);
        assert_eq!(truncate_to_display_width(text, 3), "a界");
        assert_eq!(nth_char_byte_offset(text, 2), "a界".len());
        assert_eq!(prev_grapheme_boundary(text, 3), 2);
        assert_eq!(next_grapheme_boundary(text, 2), 3);
    }

    #[test]
    fn grapheme_display_width_empty_string() {
        assert_eq!(grapheme_display_width(""), 0);
    }

    #[test]
    fn grapheme_display_width_ascii() {
        assert_eq!(grapheme_display_width("hello"), 5);
    }

    #[test]
    fn grapheme_display_width_trailing_whitespace() {
        assert_eq!(grapheme_display_width("hi  "), 4);
    }

    #[test]
    fn grapheme_display_width_combining_mark() {
        // e + combining acute accent = single grapheme of display width 1
        assert_eq!(grapheme_display_width("e\u{0301}"), 1);
    }

    #[test]
    fn grapheme_display_width_zwj_emoji_family() {
        // 👨‍👩‍👧 — 5 codepoints, 1 grapheme cluster, display width 2
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        assert_eq!(family.graphemes(true).count(), 1);
        assert_eq!(grapheme_display_width(family), 2);
    }

    #[test]
    fn grapheme_display_width_cjk() {
        assert_eq!(grapheme_display_width("界"), 2);
        assert_eq!(grapheme_display_width("日本語"), 6);
    }

    #[test]
    fn gutter_and_screen_position_helpers_work() {
        assert_eq!(gutter_width(0), 4);
        assert_eq!(screen_to_buffer_pos(12, 8, 2, 4, 3, 4), (7, 6));
        assert_eq!(buffer_to_screen_pos(7, 6, 3, 4, 2, 4, 10), Some((12, 8)));
        assert_eq!(buffer_to_screen_pos(1, 0, 3, 4, 2, 4, 10), None);
    }

    #[test]
    fn display_col_to_char_col_grapheme_boundary_safety() {
        // Combining mark: e + combining acute accent = single grapheme of width 1
        let text = "e\u{0301}"; // "é" as two chars, one grapheme
        assert_eq!(display_col_to_char_col(text, 0), 0);
        // display_col=1 should land AFTER the grapheme (char index 2), not in the middle
        assert_eq!(display_col_to_char_col(text, 1), 2);

        // With surrounding text: "xe\u{0301}y" — chars: x(0), e(1), combining(2), y(3)
        let text2 = "xe\u{0301}y";
        // display_col=2 lands at the é/y boundary (char index 3), not the combining mark (2)
        assert_eq!(display_col_to_char_col(text2, 2), 3);

        // ZWJ emoji: 👨‍👩‍👧 — 5 chars, 1 grapheme cluster
        let zwj_family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        assert_eq!(zwj_family.graphemes(true).count(), 1);
        // Every display column must map to a grapheme boundary (0 or 5), never 1-4
        for dc in 0..=10 {
            let r = display_col_to_char_col(zwj_family, dc);
            assert!(
                r == 0 || r == 5,
                "dc={} returned {} which is inside a grapheme cluster",
                dc,
                r
            );
        }
    }

    #[test]
    fn mutation_apis_handle_out_of_range_positions() {
        // delete_range with pos beyond buffer
        let mut buffer = RopeBuffer::from_text("abc");
        let deleted = buffer.delete_range(10, 2);
        assert_eq!(deleted, "");
        assert_eq!(buffer.text(), "abc");

        // delete_range: pos within buffer but end extends beyond
        let mut buffer2 = RopeBuffer::from_text("abc");
        let deleted2 = buffer2.delete_range(1, 100);
        assert_eq!(deleted2, "bc");
        assert_eq!(buffer2.text(), "a");

        // insert_char with pos beyond buffer
        let mut buffer3 = RopeBuffer::from_text("abc");
        buffer3.insert_char(100, 'z');
        assert_eq!(buffer3.text(), "abcz");

        // insert_str with pos beyond buffer
        let mut buffer4 = RopeBuffer::from_text("abc");
        buffer4.insert_str(100, "xyz");
        assert_eq!(buffer4.text(), "abcxyz");

        // replace_range with pos beyond buffer and non-empty replacement (inserts at end)
        let mut buffer5 = RopeBuffer::from_text("abc");
        buffer5.replace_range(100, 5, "xyz");
        assert_eq!(buffer5.text(), "abcxyz");

        // replace_range with pos at end, len > 0, replacement = "" (no-op)
        let mut buffer6 = RopeBuffer::from_text("abc");
        buffer6.replace_range(3, 10, "");
        assert_eq!(buffer6.text(), "abc");

        // Empty buffer: all operations should be safe
        let mut empty = RopeBuffer::new();
        assert_eq!(empty.delete_range(10, 5), "");
        empty.insert_char(100, 'x');
        assert_eq!(empty.text(), "x");
        empty.insert_str(100, "yz");
        assert_eq!(empty.text(), "xyz");
        empty.replace_range(100, 10, "hello");
        assert_eq!(empty.text(), "xyzhello");
    }
}
