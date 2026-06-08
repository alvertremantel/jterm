use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use jones_text::{Direction, EditorState, RopeBuffer};
use std::path::Path;

/// Which mode the content pane is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentMode {
    /// Read-only markdown preview (default).
    Read,
    /// Full editor with syntax highlighting and cursor.
    Edit,
    /// Side-by-side editor + live markdown preview.
    Split,
}

impl ContentMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContentMode::Read => "READ",
            ContentMode::Edit => "EDIT",
            ContentMode::Split => "SPLIT",
        }
    }
}

/// The result of handling a key event in the editor.
pub enum EditorAction {
    /// Nothing special happened.
    None,
    /// User wants to leave edit mode (Esc).
    ExitEditor,
    /// User wants to save the file (Ctrl+S).
    SaveFile,
    /// User wants to toggle split preview (Ctrl+P).
    ToggleSplitPreview,
    /// User wants to open the find bar (Ctrl+F).
    Find,
    /// User wants to reload the file from disk (Ctrl+R).
    ReloadFile,
}

/// Wraps shared text buffer and editor state.
pub struct EditorContext {
    pub buffer: RopeBuffer,
    pub state: EditorState,
    /// Fallback clipboard for when the system clipboard is unavailable.
    internal_clipboard: String,
    /// Whether the find bar is currently active.
    pub search_active: bool,
    /// The current search query text.
    pub search_query: String,
    /// Whether the replace input row is visible.
    pub replace_active: bool,
    /// The current replace query text.
    pub replace_query: String,
    /// When `true`, keyboard input targets `replace_query`; otherwise `search_query`.
    pub replace_focused: bool,
    /// Viewport height in lines (set by app before each `handle_key`).
    pub viewport_height: usize,
}

impl EditorContext {
    /// Create an editor context from raw markdown text.
    pub fn from_content(content: &str) -> Self {
        Self {
            buffer: RopeBuffer::from_text(content),
            state: EditorState::new(),
            internal_clipboard: String::new(),
            search_active: false,
            search_query: String::new(),
            replace_active: false,
            replace_query: String::new(),
            replace_focused: false,
            viewport_height: 30,
        }
    }

    /// Get the current buffer text.
    pub fn text(&self) -> String {
        self.buffer.text()
    }

    /// Whether the buffer has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.buffer.is_dirty()
    }

    /// Save the buffer to a file path.
    pub fn save(&mut self, path: &Path) -> color_eyre::Result<()> {
        self.buffer.save_file(path)
    }

    // ── Word boundary helpers ──────────────────────────────────────

    /// Find the char offset of the previous word boundary from `pos`.
    /// A word char is alphanumeric or underscore.
    fn word_boundary_left(&self, pos: usize) -> usize {
        if pos == 0 {
            return 0;
        }
        let rope = self.buffer.rope();
        let mut i = pos;

        // Skip non-word chars backwards.
        while i > 0 {
            let ch = rope.char(i - 1);
            if ch.is_alphanumeric() || ch == '_' {
                break;
            }
            i -= 1;
        }
        // Skip word chars backwards.
        while i > 0 {
            let ch = rope.char(i - 1);
            if !(ch.is_alphanumeric() || ch == '_') {
                break;
            }
            i -= 1;
        }
        i
    }

    /// Find the char offset of the next word boundary from `pos`.
    /// A word char is alphanumeric or underscore.
    fn word_boundary_right(&self, pos: usize) -> usize {
        let rope = self.buffer.rope();
        let len = rope.len_chars();
        if pos >= len {
            return len;
        }
        let mut i = pos;

        // Skip word chars forwards.
        while i < len {
            let ch = rope.char(i);
            if !(ch.is_alphanumeric() || ch == '_') {
                break;
            }
            i += 1;
        }
        // Skip non-word chars forwards.
        while i < len {
            let ch = rope.char(i);
            if ch.is_alphanumeric() || ch == '_' {
                break;
            }
            i += 1;
        }
        i
    }

    /// Move cursor to a given char offset, updating line/col/desired_col.
    pub fn move_cursor_to_char_pos(&mut self, char_pos: usize) {
        let rope = self.buffer.rope();
        let clamped = char_pos.min(rope.len_chars());
        let line = rope.char_to_line(clamped);
        let line_start = rope.line_to_char(line);
        self.state.cursor_line = line;
        self.state.cursor_col = clamped - line_start;
        self.state.desired_col = self.state.cursor_col;
    }

    /// Get the current cursor position as a char offset in the rope.
    pub fn cursor_char_pos(&self) -> usize {
        let rope = self.buffer.rope();
        rope.line_to_char(self.state.cursor_line) + self.state.cursor_col
    }

    pub fn move_cursor_to_line_col(&mut self, line: usize, col: usize) {
        let rope = self.buffer.rope();
        self.state.move_cursor_to(line, col, rope);
    }

    // ── Clipboard helpers ──────────────────────────────────────────

    /// Copy the given text to both system clipboard and internal fallback.
    fn clipboard_set(&mut self, text: &str) {
        self.internal_clipboard = text.to_string();
        if let Ok(mut cb) = arboard::Clipboard::new() {
            let _ = cb.set_text(text);
        }
    }

    /// Read text from clipboard: try system first, fall back to internal.
    fn clipboard_get(&self) -> String {
        if let Ok(mut cb) = arboard::Clipboard::new()
            && let Ok(text) = cb.get_text()
        {
            return text;
        }
        self.internal_clipboard.clone()
    }

    /// Handle a key event, returning what happened.
    pub fn handle_key(&mut self, key: KeyEvent) -> EditorAction {
        // Intercept keys when the find bar is active.
        if self.search_active {
            return self.handle_search_key(key);
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        let rope = self.buffer.rope();
        let total_lines = rope.len_lines();

        match key.code {
            // ── Exit editor ─────────────────────────────────────────
            KeyCode::Esc => return EditorAction::ExitEditor,

            // ── Save ────────────────────────────────────────────────
            KeyCode::Char('s') if ctrl => {
                return EditorAction::SaveFile;
            }

            // ── Toggle split preview ─────────────────────────────────
            KeyCode::Char('p') if ctrl => {
                return EditorAction::ToggleSplitPreview;
            }

            // ── Find ────────────────────────────────────────────────
            KeyCode::Char('f') if ctrl => {
                self.search_active = true;
                self.search_query.clear();
                self.replace_active = false;
                self.replace_query.clear();
                self.replace_focused = false;
                return EditorAction::Find;
            }

            // ── Reload file from disk ──────────────────────────────
            KeyCode::Char('r') if ctrl => {
                return EditorAction::ReloadFile;
            }

            // ── Markdown: Bold toggle (Ctrl+B) ─────────────────────
            KeyCode::Char('b') if ctrl => {
                self.toggle_inline_marker("**");
            }

            // ── Markdown: Italic toggle (Ctrl+I) ───────────────────
            KeyCode::Char('i') if ctrl => {
                self.toggle_inline_marker("*");
            }

            // ── Markdown: Link insertion (Ctrl+K) ──────────────────
            KeyCode::Char('k') if ctrl => {
                self.insert_link();
            }

            // ── Markdown: Heading level toggle (Ctrl+1..6) ─────────
            KeyCode::Char(c @ '1'..='6') if ctrl => {
                let level = (c as u8 - b'0') as usize;
                self.toggle_heading(level);
            }

            // ── Undo / Redo ─────────────────────────────────────────
            KeyCode::Char('z') if ctrl => {
                self.buffer.undo();
                let rope = self.buffer.rope();
                self.state
                    .move_cursor_to(self.state.cursor_line, self.state.cursor_col, rope);
            }
            KeyCode::Char('y') if ctrl => {
                self.buffer.redo();
                let rope = self.buffer.rope();
                self.state
                    .move_cursor_to(self.state.cursor_line, self.state.cursor_col, rope);
            }

            // ── Select all ──────────────────────────────────────────
            KeyCode::Char('a') if ctrl => {
                let rope = self.buffer.rope();
                self.state.cursor_line = 0;
                self.state.cursor_col = 0;
                self.state.start_selection();
                let last_line = total_lines.saturating_sub(1);
                let last_col = EditorState::line_len_for(rope, last_line);
                self.state.cursor_line = last_line;
                self.state.cursor_col = last_col;
                self.state.extend_selection();
            }

            // ── Clipboard: Copy ─────────────────────────────────────
            KeyCode::Char('c') if ctrl => {
                if let Some((start, end)) = self.state.selected_char_range(self.buffer.rope()) {
                    let rope = self.buffer.rope();
                    let text: String = rope.slice(start..end).to_string();
                    self.clipboard_set(&text);
                }
            }

            // ── Clipboard: Cut ──────────────────────────────────────
            KeyCode::Char('x') if ctrl => {
                if let Some((start, end)) = self.state.selected_char_range(self.buffer.rope()) {
                    let rope = self.buffer.rope();
                    let text: String = rope.slice(start..end).to_string();
                    self.clipboard_set(&text);
                    self.delete_selection_if_any();
                }
            }

            // ── Clipboard: Paste ────────────────────────────────────
            KeyCode::Char('v') if ctrl => {
                let text = self.clipboard_get();
                if !text.is_empty() {
                    self.delete_selection_if_any();
                    let pos = self.cursor_char_pos();
                    let char_count = text.chars().count();
                    self.buffer.insert_str(pos, &text);
                    self.move_cursor_to_char_pos(pos + char_count);
                }
            }

            // ── Word-jump: Ctrl+Left ────────────────────────────────
            KeyCode::Left if ctrl => {
                self.state.clear_selection();
                let pos = self.cursor_char_pos();
                let target = self.word_boundary_left(pos);
                self.move_cursor_to_char_pos(target);
            }

            // ── Word-jump: Ctrl+Right ───────────────────────────────
            KeyCode::Right if ctrl => {
                self.state.clear_selection();
                let pos = self.cursor_char_pos();
                let target = self.word_boundary_right(pos);
                self.move_cursor_to_char_pos(target);
            }

            // ── Word-delete: Ctrl+Backspace ─────────────────────────
            KeyCode::Backspace if ctrl => {
                if self.state.selection.is_some() {
                    self.delete_selection_if_any();
                } else {
                    let pos = self.cursor_char_pos();
                    let target = self.word_boundary_left(pos);
                    if target < pos {
                        self.buffer.delete_range(target, pos - target);
                        self.move_cursor_to_char_pos(target);
                    }
                }
            }

            // ── Word-delete: Ctrl+Delete ────────────────────────────
            KeyCode::Delete if ctrl => {
                if self.state.selection.is_some() {
                    self.delete_selection_if_any();
                } else {
                    let pos = self.cursor_char_pos();
                    let target = self.word_boundary_right(pos);
                    if target > pos {
                        self.buffer.delete_range(pos, target - pos);
                        // Cursor stays at pos; just clamp it.
                        self.move_cursor_to_char_pos(pos);
                    }
                }
            }

            // ── Shift+Arrow: selection ──────────────────────────────
            KeyCode::Up if shift => self.move_with_selection(Direction::Up),
            KeyCode::Down if shift => self.move_with_selection(Direction::Down),
            KeyCode::Left if shift => self.move_with_selection(Direction::Left),
            KeyCode::Right if shift => self.move_with_selection(Direction::Right),

            // ── Cursor movement ─────────────────────────────────────
            KeyCode::Up => self.move_plain(Direction::Up),
            KeyCode::Down => self.move_plain(Direction::Down),
            KeyCode::Left => self.move_plain(Direction::Left),
            KeyCode::Right => self.move_plain(Direction::Right),
            KeyCode::Home => {
                self.state.clear_selection();
                self.state.home();
            }
            KeyCode::End => {
                self.state.clear_selection();
                let rope = self.buffer.rope();
                self.state.end(rope);
            }
            KeyCode::PageUp => {
                self.state.clear_selection();
                let rope = self.buffer.rope();
                self.state.page_up(self.viewport_height, rope);
            }
            KeyCode::PageDown => {
                self.state.clear_selection();
                let rope = self.buffer.rope();
                self.state.page_down(self.viewport_height, rope);
            }

            // ── Text insertion ──────────────────────────────────────
            KeyCode::Char(c) => {
                self.handle_char_insert(c);
            }
            KeyCode::Enter => {
                self.handle_enter();
            }
            KeyCode::Tab if shift => {
                self.handle_outdent();
            }
            KeyCode::Tab => {
                self.handle_tab();
            }
            KeyCode::BackTab => {
                self.handle_outdent();
            }
            KeyCode::Backspace => {
                if self.state.selection.is_some() {
                    self.delete_selection_if_any();
                } else {
                    let rope = self.buffer.rope();
                    let pos = rope.line_to_char(self.state.cursor_line) + self.state.cursor_col;
                    if pos > 0 {
                        self.buffer.delete_range(pos - 1, 1);
                        let rope = self.buffer.rope();
                        self.state.move_cursor(Direction::Left, rope);
                    }
                }
            }
            KeyCode::Delete => {
                if self.state.selection.is_some() {
                    self.delete_selection_if_any();
                } else {
                    let rope = self.buffer.rope();
                    let pos = rope.line_to_char(self.state.cursor_line) + self.state.cursor_col;
                    if pos < rope.len_chars() {
                        self.buffer.delete_range(pos, 1);
                    }
                }
            }

            _ => {}
        }

        EditorAction::None
    }

    // ── Smart editing helpers ────────────────────────────────────

    /// Detect a list marker at the start of `line_text` (after leading whitespace).
    /// Returns `(indent, marker)` where `indent` is the leading whitespace
    /// and `marker` is the list prefix (e.g. `"- "`, `"* "`, `"1. "`).
    /// Returns `None` if no list marker is found.
    fn detect_list_marker(line_text: &str) -> Option<(String, String)> {
        let indent_len = line_text.len() - line_text.trim_start().len();
        let indent: String = line_text[..indent_len].to_string();
        let rest = &line_text[indent_len..];

        // Unordered: `- ` or `* `
        if rest.starts_with("- ") {
            return Some((indent, "- ".to_string()));
        }
        if rest.starts_with("* ") {
            return Some((indent, "* ".to_string()));
        }

        // Ordered: `123. `
        let mut num_end = 0;
        for ch in rest.chars() {
            if ch.is_ascii_digit() {
                num_end += 1;
            } else {
                break;
            }
        }
        if num_end > 0 && rest[num_end..].starts_with(". ") {
            let num: u64 = rest[..num_end].parse().unwrap_or(0);
            return Some((indent, format!("{}. ", num)));
        }

        None
    }

    /// Handle the Enter key with auto-indent and list continuation.
    fn handle_enter(&mut self) {
        self.delete_selection_if_any();

        // Extract info from the current line before mutating.
        let (pos, indent_str, list_info) = {
            let rope = self.buffer.rope();
            let line_idx = self.state.cursor_line;
            let line_start = rope.line_to_char(line_idx);
            let pos = line_start + self.state.cursor_col;
            let line_slice = rope.line(line_idx);
            let line_text: String = line_slice.chars().collect();
            // Strip trailing newline for analysis.
            let content = line_text.trim_end_matches(&['\n', '\r'][..]);

            let indent_len = content.len() - content.trim_start().len();
            let indent_str: String = content[..indent_len].to_string();

            let list_info = Self::detect_list_marker(content);

            (pos, indent_str, list_info)
        };

        if let Some((list_indent, marker)) = list_info {
            let full_prefix = format!("{}{}", list_indent, marker);
            // Check if the current line is ONLY the marker (no content after it).
            let line_text = {
                let rope = self.buffer.rope();
                let line_slice = rope.line(self.state.cursor_line);
                let lt: String = line_slice.chars().collect();
                lt.trim_end_matches(&['\n', '\r'][..]).to_string()
            };

            if line_text.trim_end() == full_prefix.trim_end() {
                // Empty list item: remove the marker and unindent.
                let rope = self.buffer.rope();
                let line_start = rope.line_to_char(self.state.cursor_line);
                let remove_len = full_prefix.chars().count();
                self.buffer.delete_range(line_start, remove_len);
                self.state.cursor_col = 0;
                self.state.desired_col = 0;
            } else {
                // Insert newline + indent + next marker.
                let next_marker = Self::next_list_marker(&marker);
                let insert_text = format!("\n{}{}", list_indent, next_marker);
                let new_col = list_indent.chars().count() + next_marker.chars().count();
                self.buffer.insert_str(pos, &insert_text);
                self.state.cursor_line += 1;
                self.state.cursor_col = new_col;
                self.state.desired_col = new_col;
            }
        } else {
            // No list marker: insert newline + same indent.
            let insert_text = format!("\n{}", indent_str);
            let new_col = indent_str.chars().count();
            self.buffer.insert_str(pos, &insert_text);
            self.state.cursor_line += 1;
            self.state.cursor_col = new_col;
            self.state.desired_col = new_col;
        }
    }

    /// Given a list marker like `"- "`, `"* "`, or `"1. "`, return the next
    /// appropriate marker. Unordered markers repeat; ordered markers increment.
    fn next_list_marker(marker: &str) -> String {
        // Check for ordered: digits followed by ". "
        let trimmed = marker.trim_end();
        if trimmed.ends_with('.') {
            let num_str = trimmed.trim_end_matches('.');
            if let Ok(n) = num_str.parse::<u64>() {
                return format!("{}. ", n + 1);
            }
        }
        // Unordered or unrecognized: return as-is.
        marker.to_string()
    }

    // ── Markdown formatting helpers ────────────────────────────

    /// Toggle an inline marker (e.g. `**` for bold, `*` for italic) around
    /// the current selection. If no selection, insert the marker pair and
    /// position the cursor between them.
    fn toggle_inline_marker(&mut self, marker: &str) {
        let marker_len = marker.chars().count();

        if let Some((start, end)) = self.state.selected_char_range(self.buffer.rope()) {
            // Extract selected text into an owned String before mutating.
            let selected: String = self.buffer.rope().slice(start..end).to_string();

            if selected.starts_with(marker)
                && selected.ends_with(marker)
                && selected.chars().count() >= marker_len * 2
            {
                // Already wrapped — remove the markers.
                let inner = &selected[marker.len()..selected.len() - marker.len()];
                let inner_owned = inner.to_string();
                let inner_char_len = inner_owned.chars().count();
                self.state.clear_selection();
                self.buffer.replace_range(start, end - start, &inner_owned);
                // Select the unwrapped text.
                self.move_cursor_to_char_pos(start);
                self.state.start_selection();
                self.move_cursor_to_char_pos(start + inner_char_len);
                self.state.extend_selection();
            } else {
                // Wrap selection in markers.
                let wrapped = format!("{marker}{selected}{marker}");
                let wrapped_char_len = wrapped.chars().count();
                self.state.clear_selection();
                self.buffer.replace_range(start, end - start, &wrapped);
                // Select the entire wrapped text.
                self.move_cursor_to_char_pos(start);
                self.state.start_selection();
                self.move_cursor_to_char_pos(start + wrapped_char_len);
                self.state.extend_selection();
            }
        } else {
            // No selection — insert empty marker pair and place cursor inside.
            let pos = self.cursor_char_pos();
            let pair = format!("{marker}{marker}");
            self.buffer.insert_str(pos, &pair);
            self.move_cursor_to_char_pos(pos + marker_len);
        }
    }

    /// Insert a markdown link. If text is selected, wrap it as `[text](url)`
    /// with the cursor positioned inside the `url` placeholder. If no
    /// selection, insert `[](url)` with the cursor inside the brackets.
    fn insert_link(&mut self) {
        if let Some((start, end)) = self.state.selected_char_range(self.buffer.rope()) {
            let selected: String = self.buffer.rope().slice(start..end).to_string();
            let link = format!("[{selected}](url)");
            self.state.clear_selection();
            self.buffer.replace_range(start, end - start, &link);
            // Place cursor inside the (url) part — on the 'u' of "url".
            // "[selected](url)" — the 'u' is at start + 1 + selected_len + 2
            let selected_char_len = selected.chars().count();
            let cursor_pos = start + 1 + selected_char_len + 2; // past "[text]("
            self.move_cursor_to_char_pos(cursor_pos);
            // Select the "url" placeholder so the user can type over it.
            self.state.start_selection();
            self.move_cursor_to_char_pos(cursor_pos + 3); // "url" is 3 chars
            self.state.extend_selection();
        } else {
            let pos = self.cursor_char_pos();
            let link = "[](url)";
            self.buffer.insert_str(pos, link);
            // Place cursor inside the brackets: after '['.
            self.move_cursor_to_char_pos(pos + 1);
        }
    }

    /// Toggle heading level on the current line. If the line already has the
    /// requested level, remove the heading prefix. Otherwise, set it.
    fn toggle_heading(&mut self, level: usize) {
        let prefix = format!("{} ", "#".repeat(level));
        let prefix_char_len = prefix.chars().count();

        // Extract current line info before mutating.
        let (line_start, current_heading_len, has_same_level) = {
            let rope = self.buffer.rope();
            let line_idx = self.state.cursor_line;
            let line_start = rope.line_to_char(line_idx);
            let line_slice = rope.line(line_idx);
            let line_text: String = line_slice.chars().collect();
            let content = line_text.trim_end_matches(&['\n', '\r'][..]);

            // Detect existing heading prefix: count leading '#' chars.
            let hash_count = content.chars().take_while(|&c| c == '#').count();
            let current_heading_len =
                if hash_count > 0 && content.chars().nth(hash_count) == Some(' ') {
                    hash_count + 1 // include the space
                } else {
                    0
                };
            let has_same_level = hash_count == level && current_heading_len > 0;

            (line_start, current_heading_len, has_same_level)
        };

        if has_same_level {
            // Remove heading prefix.
            self.buffer.delete_range(line_start, current_heading_len);
            // Adjust cursor column.
            self.state.cursor_col = self.state.cursor_col.saturating_sub(current_heading_len);
            self.state.desired_col = self.state.cursor_col;
        } else {
            // Replace existing heading prefix (if any) with new one.
            self.buffer
                .replace_range(line_start, current_heading_len, &prefix);
            // Adjust cursor column: subtract old prefix, add new prefix.
            let new_col = if self.state.cursor_col >= current_heading_len {
                self.state.cursor_col - current_heading_len + prefix_char_len
            } else {
                prefix_char_len
            };
            self.state.cursor_col = new_col;
            self.state.desired_col = new_col;
        }
    }

    /// Handle Tab key: indent selected lines or insert 4 spaces.
    fn handle_tab(&mut self) {
        if let Some(sel) = &self.state.selection {
            let ((start_line, _), (end_line, _)) = sel.normalized();
            // Indent all lines in the selection by 4 spaces.
            // Work from last line to first to preserve char offsets.
            let first = start_line;
            let last = end_line;
            self.state.clear_selection();
            for line_idx in (first..=last).rev() {
                let line_start = self.buffer.rope().line_to_char(line_idx);
                self.buffer.insert_str(line_start, "    ");
            }
            // Adjust cursor column.
            self.state.cursor_col += 4;
            self.state.desired_col = self.state.cursor_col;
        } else {
            // No selection: insert 4 spaces at cursor.
            self.delete_selection_if_any();
            let rope = self.buffer.rope();
            let pos = rope.line_to_char(self.state.cursor_line) + self.state.cursor_col;
            self.buffer.insert_str(pos, "    ");
            self.state.cursor_col += 4;
            self.state.desired_col = self.state.cursor_col;
        }
    }

    /// Handle Shift+Tab / BackTab: outdent selected lines or current line.
    fn handle_outdent(&mut self) {
        if let Some(sel) = &self.state.selection {
            let ((start_line, _), (end_line, _)) = sel.normalized();
            let first = start_line;
            let last = end_line;
            self.state.clear_selection();
            for line_idx in (first..=last).rev() {
                self.outdent_line(line_idx);
            }
        } else {
            self.outdent_line(self.state.cursor_line);
        }
        // Re-clamp cursor column after outdent.
        let rope = self.buffer.rope();
        let max_col = EditorState::line_len_for(rope, self.state.cursor_line);
        self.state.cursor_col = self.state.cursor_col.min(max_col);
        self.state.desired_col = self.state.cursor_col;
    }

    /// Remove up to 4 leading spaces from the given line.
    fn outdent_line(&mut self, line_idx: usize) {
        let (line_start, remove_count) = {
            let rope = self.buffer.rope();
            if line_idx >= rope.len_lines() {
                return;
            }
            let line_start = rope.line_to_char(line_idx);
            let line_slice = rope.line(line_idx);
            let mut remove = 0usize;
            for ch in line_slice.chars().take(4) {
                if ch == ' ' {
                    remove += 1;
                } else {
                    break;
                }
            }
            (line_start, remove)
        };

        if remove_count > 0 {
            self.buffer.delete_range(line_start, remove_count);
            // Adjust cursor if on this line.
            if self.state.cursor_line == line_idx {
                self.state.cursor_col = self.state.cursor_col.saturating_sub(remove_count);
            }
        }
    }

    /// Handle character insertion with auto-close pairs.
    fn handle_char_insert(&mut self, c: char) {
        self.delete_selection_if_any();

        let closing = match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            '"' => Some('"'),
            '`' => Some('`'),
            _ => None,
        };

        if let Some(close_ch) = closing {
            // Special case for `*`: don't auto-close if previous char is also `*`
            // (allows natural `**bold**` typing).
            if c == '`' {
                let suppress = {
                    let rope = self.buffer.rope();
                    let pos = rope.line_to_char(self.state.cursor_line) + self.state.cursor_col;
                    pos > 0 && rope.len_chars() > 0 && rope.char(pos - 1) == '`'
                };
                if suppress {
                    // Just insert the character normally.
                    let rope = self.buffer.rope();
                    let pos = rope.line_to_char(self.state.cursor_line) + self.state.cursor_col;
                    self.buffer.insert_char(pos, c);
                    let rope = self.buffer.rope();
                    self.state.move_cursor(Direction::Right, rope);
                    return;
                }
            }

            // For `"`: don't auto-close if previous char is also `"` or alphanumeric
            // (to avoid issues with closing quotes).
            if c == '"' {
                let suppress = {
                    let rope = self.buffer.rope();
                    let pos = rope.line_to_char(self.state.cursor_line) + self.state.cursor_col;
                    if pos > 0 {
                        let prev = rope.char(pos - 1);
                        prev == '"' || prev.is_alphanumeric()
                    } else {
                        false
                    }
                };
                if suppress {
                    let rope = self.buffer.rope();
                    let pos = rope.line_to_char(self.state.cursor_line) + self.state.cursor_col;
                    self.buffer.insert_char(pos, c);
                    let rope = self.buffer.rope();
                    self.state.move_cursor(Direction::Right, rope);
                    return;
                }
            }

            // Check if the next character is whitespace or end of line.
            let should_auto_close = {
                let rope = self.buffer.rope();
                let pos = rope.line_to_char(self.state.cursor_line) + self.state.cursor_col;
                if pos >= rope.len_chars() {
                    true // End of document.
                } else {
                    let next_ch = rope.char(pos);
                    next_ch.is_whitespace()
                        || next_ch == '\n'
                        || next_ch == '\r'
                        || next_ch == ')'
                        || next_ch == ']'
                        || next_ch == '}'
                }
            };

            if should_auto_close {
                let rope = self.buffer.rope();
                let pos = rope.line_to_char(self.state.cursor_line) + self.state.cursor_col;
                // Insert both open and close characters.
                let pair = format!("{}{}", c, close_ch);
                self.buffer.insert_str(pos, &pair);
                // Move cursor between the pair (one position right).
                let rope = self.buffer.rope();
                self.state.move_cursor(Direction::Right, rope);
            } else {
                // Just insert the opening character.
                let rope = self.buffer.rope();
                let pos = rope.line_to_char(self.state.cursor_line) + self.state.cursor_col;
                self.buffer.insert_char(pos, c);
                let rope = self.buffer.rope();
                self.state.move_cursor(Direction::Right, rope);
            }
        } else {
            // Not a pair character: insert normally.
            let rope = self.buffer.rope();
            let pos = rope.line_to_char(self.state.cursor_line) + self.state.cursor_col;
            self.buffer.insert_char(pos, c);
            let rope = self.buffer.rope();
            self.state.move_cursor(Direction::Right, rope);
        }
    }

    /// Handle a key event while the find bar is active.
    fn handle_search_key(&mut self, key: KeyEvent) -> EditorAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Esc => {
                self.search_active = false;
                self.replace_active = false;
                self.replace_focused = false;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                if self.replace_active {
                    // Toggle focus between find and replace fields.
                    self.replace_focused = !self.replace_focused;
                } else {
                    // Activate the replace row, focus on it.
                    self.replace_active = true;
                    self.replace_focused = true;
                }
            }
            KeyCode::Enter => {
                if self.replace_focused && self.replace_active {
                    self.replace_current_match();
                } else {
                    self.jump_to_next_match();
                }
            }
            // Ctrl+A while replace is active: replace all matches.
            KeyCode::Char('a') if ctrl && self.replace_active => {
                self.replace_all_matches();
            }
            KeyCode::Backspace => {
                if self.replace_focused {
                    self.replace_query.pop();
                } else {
                    self.search_query.pop();
                }
            }
            KeyCode::Char(c) => {
                if self.replace_focused {
                    self.replace_query.push(c);
                } else {
                    self.search_query.push(c);
                }
            }
            _ => {}
        }
        EditorAction::None
    }

    /// Jump to the next occurrence of `search_query` in the buffer,
    /// starting after the current cursor position. Wraps around to the
    /// beginning of the document if no match is found after the cursor.
    fn jump_to_next_match(&mut self) {
        if self.search_query.is_empty() {
            return;
        }
        let text = self.buffer.text();
        let query = &self.search_query;
        let cursor_char = self.cursor_char_pos();

        // Convert char offset to byte offset for string slicing.
        let cursor_byte = text
            .char_indices()
            .nth(cursor_char)
            .map(|(b, _)| b)
            .unwrap_or(text.len());
        let search_start = text
            .char_indices()
            .nth(cursor_char + 1)
            .map(|(b, _)| b)
            .unwrap_or(text.len());

        // Search forward from just after the cursor.
        if let Some(rel) = text[search_start..].find(query) {
            let byte_offset = search_start + rel;
            let char_pos = self.byte_offset_to_char_pos(&text, byte_offset);
            self.move_cursor_to_char_pos(char_pos);
            return;
        }
        // Wrap around: search from the beginning up to the cursor.
        let wrap_end = cursor_byte.min(text.len());
        if let Some(rel) = text[..wrap_end].find(query) {
            let char_pos = self.byte_offset_to_char_pos(&text, rel);
            self.move_cursor_to_char_pos(char_pos);
        }
    }

    /// Convert a byte offset in a string to a char offset.
    fn byte_offset_to_char_pos(&self, text: &str, byte_offset: usize) -> usize {
        text[..byte_offset].chars().count()
    }

    /// Replace the match at or nearest after the cursor, then jump to the next
    /// match. If no match is found after the cursor, wraps to the beginning.
    fn replace_current_match(&mut self) {
        if self.search_query.is_empty() {
            return;
        }
        let text = self.buffer.text();
        let query = &self.search_query;
        let cursor_char = self.cursor_char_pos();

        // Find the byte offset of the cursor position.
        let cursor_byte = text
            .char_indices()
            .nth(cursor_char)
            .map(|(b, _)| b)
            .unwrap_or(text.len());

        // Try to find a match starting at or after the cursor.
        let match_byte = if let Some(rel) = text[cursor_byte..].find(query) {
            Some(cursor_byte + rel)
        } else {
            // Wrap: search from start up to cursor.
            text[..cursor_byte].find(query)
        };

        if let Some(byte_offset) = match_byte {
            let char_pos = self.byte_offset_to_char_pos(&text, byte_offset);
            let query_char_len = query.chars().count();
            self.buffer
                .replace_range(char_pos, query_char_len, &self.replace_query.clone());
            // Position cursor at end of replacement so next jump skips past it.
            let replacement_char_len = self.replace_query.chars().count();
            self.move_cursor_to_char_pos(char_pos + replacement_char_len);
            // Jump to next match after the replacement.
            self.jump_to_next_match();
        }
    }

    /// Replace every occurrence of `search_query` with `replace_query`.
    /// Iterates from end to start to preserve byte/char offsets.
    fn replace_all_matches(&mut self) {
        if self.search_query.is_empty() {
            return;
        }
        let text = self.buffer.text();
        let query = &self.search_query;

        // Collect all byte-offset match positions.
        let mut byte_positions: Vec<usize> = Vec::new();
        let mut start = 0;
        while let Some(rel) = text[start..].find(query) {
            byte_positions.push(start + rel);
            start += rel + query.len();
        }

        if byte_positions.is_empty() {
            return;
        }

        // Convert byte positions to char positions.
        let query_char_len = query.chars().count();
        let replacement = self.replace_query.clone();

        // Replace from end to start so earlier offsets remain valid.
        let char_positions: Vec<usize> = byte_positions
            .iter()
            .map(|&b| self.byte_offset_to_char_pos(&text, b))
            .collect();

        for &char_pos in char_positions.iter().rev() {
            self.buffer
                .replace_range(char_pos, query_char_len, &replacement);
        }

        // Place cursor at the end of the last (by document order) replacement.
        if let Some(&first_char_pos) = char_positions.first() {
            let replacement_char_len = replacement.chars().count();
            self.move_cursor_to_char_pos(first_char_pos + replacement_char_len);
        }
    }

    /// Delete the selected text if there is a selection, leaving cursor at
    /// the start of the deleted region.
    fn delete_selection_if_any(&mut self) {
        if let Some((start, end)) = self.state.selected_char_range(self.buffer.rope()) {
            let len = end - start;
            if len > 0 {
                self.buffer.delete_range(start, len);
                // Move cursor to start of deleted region.
                let rope = self.buffer.rope();
                let line = rope.char_to_line(start);
                let line_start = rope.line_to_char(line);
                self.state.cursor_line = line;
                self.state.cursor_col = start - line_start;
                self.state.desired_col = self.state.cursor_col;
            }
            self.state.clear_selection();
        }
    }

    // ── Movement helpers ────────────────────────────────────────

    /// Move the cursor in `dir`, clearing any active selection first.
    fn move_plain(&mut self, dir: Direction) {
        self.state.clear_selection();
        let rope = self.buffer.rope();
        self.state.move_cursor(dir, rope);
    }

    /// Move the cursor in `dir` while extending (or starting) a selection.
    fn move_with_selection(&mut self, dir: Direction) {
        if self.state.selection.is_none() {
            self.state.start_selection();
        }
        let rope = self.buffer.rope();
        self.state.move_cursor(dir, rope);
        self.state.extend_selection();
    }

    /// Ensure cursor is visible within the viewport.
    pub fn ensure_visible(&mut self, viewport_height: usize) {
        self.state.ensure_cursor_visible(viewport_height);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    // ── Test helpers ─────────────────────────────────────────────────

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn shift_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    /// Place cursor at the given (line, col) position.
    fn set_cursor(ed: &mut EditorContext, line: usize, col: usize) {
        ed.state.cursor_line = line;
        ed.state.cursor_col = col;
        ed.state.desired_col = col;
    }

    /// Select a range by setting cursor and using shift-arrow keys.
    /// Places cursor at `start_col` on `line`, then shift-selects `count` chars right.
    fn select_range(ed: &mut EditorContext, line: usize, start_col: usize, count: usize) {
        set_cursor(ed, line, start_col);
        ed.state.start_selection();
        ed.state.cursor_col = start_col + count;
        ed.state.desired_col = ed.state.cursor_col;
        ed.state.extend_selection();
    }

    // ── 1. Basic editing operations ─────────────────────────────────

    #[test]
    fn from_content_creates_buffer_with_correct_text() {
        let ed = EditorContext::from_content("hello world");
        assert_eq!(ed.text(), "hello world");
    }

    #[test]
    fn from_content_multiline() {
        let ed = EditorContext::from_content("line1\nline2\nline3");
        assert_eq!(ed.text(), "line1\nline2\nline3");
    }

    #[test]
    fn from_content_empty() {
        let ed = EditorContext::from_content("");
        assert_eq!(ed.text(), "");
    }

    #[test]
    fn text_returns_content() {
        let ed = EditorContext::from_content("abc");
        assert_eq!(ed.text(), "abc");
    }

    #[test]
    fn is_dirty_false_initially() {
        let ed = EditorContext::from_content("test");
        assert!(!ed.is_dirty());
    }

    #[test]
    fn is_dirty_true_after_edit() {
        let mut ed = EditorContext::from_content("test");
        ed.handle_key(key(KeyCode::Char('x')));
        assert!(ed.is_dirty());
    }

    // ── 2. Key handling — character insertion ───────────────────────

    #[test]
    fn typing_inserts_at_cursor() {
        let mut ed = EditorContext::from_content("");
        ed.handle_key(key(KeyCode::Char('a')));
        ed.handle_key(key(KeyCode::Char('b')));
        ed.handle_key(key(KeyCode::Char('c')));
        assert_eq!(ed.text(), "abc");
        assert_eq!(ed.state.cursor_col, 3);
    }

    #[test]
    fn typing_inserts_at_middle_of_text() {
        let mut ed = EditorContext::from_content("ac");
        set_cursor(&mut ed, 0, 1); // between 'a' and 'c'
        ed.handle_key(key(KeyCode::Char('b')));
        assert_eq!(ed.text(), "abc");
    }

    // ── 2. Key handling — Enter ─────────────────────────────────────

    #[test]
    fn enter_creates_new_line() {
        let mut ed = EditorContext::from_content("hello");
        set_cursor(&mut ed, 0, 5);
        ed.handle_key(key(KeyCode::Enter));
        assert_eq!(ed.text(), "hello\n");
        assert_eq!(ed.state.cursor_line, 1);
        assert_eq!(ed.state.cursor_col, 0);
    }

    #[test]
    fn enter_splits_line() {
        let mut ed = EditorContext::from_content("helloworld");
        set_cursor(&mut ed, 0, 5);
        ed.handle_key(key(KeyCode::Enter));
        assert_eq!(ed.text(), "hello\nworld");
        assert_eq!(ed.state.cursor_line, 1);
        assert_eq!(ed.state.cursor_col, 0);
    }

    // ── 2. Key handling — Backspace ─────────────────────────────────

    #[test]
    fn backspace_deletes_char_before_cursor() {
        let mut ed = EditorContext::from_content("abc");
        set_cursor(&mut ed, 0, 3);
        ed.handle_key(key(KeyCode::Backspace));
        assert_eq!(ed.text(), "ab");
    }

    #[test]
    fn backspace_at_start_of_line_joins_lines() {
        let mut ed = EditorContext::from_content("ab\ncd");
        set_cursor(&mut ed, 1, 0);
        ed.handle_key(key(KeyCode::Backspace));
        assert_eq!(ed.text(), "abcd");
    }

    #[test]
    fn backspace_at_position_zero_does_nothing() {
        let mut ed = EditorContext::from_content("abc");
        set_cursor(&mut ed, 0, 0);
        ed.handle_key(key(KeyCode::Backspace));
        assert_eq!(ed.text(), "abc");
    }

    // ── 2. Key handling — Delete ────────────────────────────────────

    #[test]
    fn delete_removes_char_after_cursor() {
        let mut ed = EditorContext::from_content("abc");
        set_cursor(&mut ed, 0, 0);
        ed.handle_key(key(KeyCode::Delete));
        assert_eq!(ed.text(), "bc");
    }

    #[test]
    fn delete_at_end_of_line_joins_lines() {
        let mut ed = EditorContext::from_content("ab\ncd");
        set_cursor(&mut ed, 0, 2);
        ed.handle_key(key(KeyCode::Delete));
        assert_eq!(ed.text(), "abcd");
    }

    #[test]
    fn delete_at_end_of_document_does_nothing() {
        let mut ed = EditorContext::from_content("abc");
        set_cursor(&mut ed, 0, 3);
        ed.handle_key(key(KeyCode::Delete));
        assert_eq!(ed.text(), "abc");
    }

    // ── 2. Key handling — Arrow keys ────────────────────────────────

    #[test]
    fn arrow_right_moves_cursor() {
        let mut ed = EditorContext::from_content("abc");
        set_cursor(&mut ed, 0, 0);
        ed.handle_key(key(KeyCode::Right));
        assert_eq!(ed.state.cursor_col, 1);
    }

    #[test]
    fn arrow_left_moves_cursor() {
        let mut ed = EditorContext::from_content("abc");
        set_cursor(&mut ed, 0, 2);
        ed.handle_key(key(KeyCode::Left));
        assert_eq!(ed.state.cursor_col, 1);
    }

    #[test]
    fn arrow_down_moves_to_next_line() {
        let mut ed = EditorContext::from_content("abc\ndef");
        set_cursor(&mut ed, 0, 1);
        ed.handle_key(key(KeyCode::Down));
        assert_eq!(ed.state.cursor_line, 1);
        assert_eq!(ed.state.cursor_col, 1);
    }

    #[test]
    fn arrow_up_moves_to_previous_line() {
        let mut ed = EditorContext::from_content("abc\ndef");
        set_cursor(&mut ed, 1, 1);
        ed.handle_key(key(KeyCode::Up));
        assert_eq!(ed.state.cursor_line, 0);
        assert_eq!(ed.state.cursor_col, 1);
    }

    #[test]
    fn arrow_down_clamps_col_to_shorter_line() {
        let mut ed = EditorContext::from_content("abcdef\nxy");
        set_cursor(&mut ed, 0, 5);
        ed.handle_key(key(KeyCode::Down));
        assert_eq!(ed.state.cursor_line, 1);
        // Column should be clamped to line length (2)
        assert!(ed.state.cursor_col <= 2);
    }

    // ── 2. Key handling — Home / End ────────────────────────────────

    #[test]
    fn home_moves_to_line_start() {
        let mut ed = EditorContext::from_content("hello world");
        set_cursor(&mut ed, 0, 5);
        ed.handle_key(key(KeyCode::Home));
        assert_eq!(ed.state.cursor_col, 0);
    }

    #[test]
    fn end_moves_to_line_end() {
        let mut ed = EditorContext::from_content("hello world");
        set_cursor(&mut ed, 0, 0);
        ed.handle_key(key(KeyCode::End));
        assert_eq!(ed.state.cursor_col, 11); // "hello world" = 11 chars
    }

    // ── 3. Auto-indent ─────────────────────────────────────────────

    #[test]
    fn enter_preserves_indent() {
        let mut ed = EditorContext::from_content("    indented");
        set_cursor(&mut ed, 0, 12); // end of "    indented"
        ed.handle_key(key(KeyCode::Enter));
        assert_eq!(ed.state.cursor_line, 1);
        assert_eq!(ed.state.cursor_col, 4); // 4 spaces of indent
        // New line starts with 4 spaces
        let text = ed.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[1].starts_with("    "));
    }

    #[test]
    fn enter_continues_unordered_list() {
        let mut ed = EditorContext::from_content("- item one");
        set_cursor(&mut ed, 0, 10); // end of "- item one"
        ed.handle_key(key(KeyCode::Enter));
        assert_eq!(ed.state.cursor_line, 1);
        assert_eq!(ed.state.cursor_col, 2); // "- " is 2 chars
        let text = ed.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], "- ");
    }

    #[test]
    fn enter_on_empty_list_item_removes_marker() {
        let mut ed = EditorContext::from_content("- first\n- ");
        set_cursor(&mut ed, 1, 2); // end of "- " on second line
        ed.handle_key(key(KeyCode::Enter));
        // The "- " marker should be removed, leaving the line empty.
        // The implementation removes the marker without inserting a newline,
        // so the second line becomes empty (just a trailing newline from line 1).
        let text = ed.text();
        assert!(text.starts_with("- first\n"), "first line preserved");
        // After the marker is removed, the second line should be empty
        let second_line = text.strip_prefix("- first\n").unwrap();
        assert_eq!(second_line, "", "marker removed, line now empty");
        assert_eq!(ed.state.cursor_col, 0);
    }

    #[test]
    fn enter_continues_ordered_list() {
        let mut ed = EditorContext::from_content("1. first item");
        set_cursor(&mut ed, 0, 13); // end of line
        ed.handle_key(key(KeyCode::Enter));
        let text = ed.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], "2. ");
    }

    #[test]
    fn enter_preserves_indent_with_list() {
        let mut ed = EditorContext::from_content("  - nested item");
        set_cursor(&mut ed, 0, 15); // end of line
        ed.handle_key(key(KeyCode::Enter));
        let text = ed.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], "  - ");
        assert_eq!(ed.state.cursor_col, 4); // "  - " = 4 chars
    }

    // ── 4. Auto-close pairs ────────────────────────────────────────

    #[test]
    fn typing_open_paren_inserts_pair() {
        let mut ed = EditorContext::from_content("");
        ed.handle_key(key(KeyCode::Char('(')));
        assert_eq!(ed.text(), "()");
        assert_eq!(ed.state.cursor_col, 1); // cursor between parens
    }

    #[test]
    fn typing_open_bracket_inserts_pair() {
        let mut ed = EditorContext::from_content("");
        ed.handle_key(key(KeyCode::Char('[')));
        assert_eq!(ed.text(), "[]");
        assert_eq!(ed.state.cursor_col, 1);
    }

    #[test]
    fn typing_open_brace_inserts_pair() {
        let mut ed = EditorContext::from_content("");
        ed.handle_key(key(KeyCode::Char('{')));
        assert_eq!(ed.text(), "{}");
        assert_eq!(ed.state.cursor_col, 1);
    }

    #[test]
    fn auto_close_does_not_trigger_mid_word() {
        // When next char is alphanumeric, auto-close should NOT trigger
        let mut ed = EditorContext::from_content("abc");
        set_cursor(&mut ed, 0, 0);
        ed.handle_key(key(KeyCode::Char('(')));
        // Should insert only '(' since next char 'a' is not whitespace/closing
        assert_eq!(ed.text(), "(abc");
    }

    // ── 5. Inline marker toggling ──────────────────────────────────

    #[test]
    fn toggle_bold_wraps_selected_text() {
        let mut ed = EditorContext::from_content("hello world");
        select_range(&mut ed, 0, 0, 5); // select "hello"
        ed.toggle_inline_marker("**");
        assert_eq!(ed.text(), "**hello** world");
    }

    #[test]
    fn toggle_bold_unwraps_already_wrapped_text() {
        let mut ed = EditorContext::from_content("**hello** world");
        select_range(&mut ed, 0, 0, 9); // select "**hello**"
        ed.toggle_inline_marker("**");
        assert_eq!(ed.text(), "hello world");
    }

    #[test]
    fn toggle_bold_no_selection_inserts_empty_markers() {
        let mut ed = EditorContext::from_content("text");
        set_cursor(&mut ed, 0, 4); // end of "text"
        ed.toggle_inline_marker("**");
        assert_eq!(ed.text(), "text****");
        assert_eq!(ed.state.cursor_col, 6); // cursor between the ** pairs
    }

    #[test]
    fn toggle_italic_wraps_selected_text() {
        let mut ed = EditorContext::from_content("hello");
        select_range(&mut ed, 0, 0, 5);
        ed.toggle_inline_marker("*");
        assert_eq!(ed.text(), "*hello*");
    }

    #[test]
    fn toggle_italic_unwraps() {
        let mut ed = EditorContext::from_content("*hello*");
        select_range(&mut ed, 0, 0, 7); // select "*hello*"
        ed.toggle_inline_marker("*");
        assert_eq!(ed.text(), "hello");
    }

    // ── 6. Heading toggling ────────────────────────────────────────

    #[test]
    fn toggle_heading_adds_prefix() {
        let mut ed = EditorContext::from_content("Hello");
        set_cursor(&mut ed, 0, 0);
        ed.toggle_heading(1);
        assert_eq!(ed.text(), "# Hello");
    }

    #[test]
    fn toggle_heading_removes_same_level() {
        let mut ed = EditorContext::from_content("# Hello");
        set_cursor(&mut ed, 0, 2); // cursor after "# "
        ed.toggle_heading(1);
        assert_eq!(ed.text(), "Hello");
    }

    #[test]
    fn toggle_heading_changes_level() {
        let mut ed = EditorContext::from_content("# Hello");
        set_cursor(&mut ed, 0, 2);
        ed.toggle_heading(2);
        assert_eq!(ed.text(), "## Hello");
    }

    #[test]
    fn toggle_heading_level_3() {
        let mut ed = EditorContext::from_content("plain text");
        set_cursor(&mut ed, 0, 0);
        ed.toggle_heading(3);
        assert_eq!(ed.text(), "### plain text");
    }

    #[test]
    fn toggle_heading_on_multiline_only_affects_cursor_line() {
        let mut ed = EditorContext::from_content("first\nsecond");
        set_cursor(&mut ed, 1, 0);
        ed.toggle_heading(1);
        assert_eq!(ed.text(), "first\n# second");
    }

    #[test]
    fn toggle_heading_adjusts_cursor_col() {
        let mut ed = EditorContext::from_content("Hello");
        set_cursor(&mut ed, 0, 3);
        ed.toggle_heading(1);
        // "# " added, so cursor_col should shift right by 2
        assert_eq!(ed.state.cursor_col, 5);
        assert_eq!(ed.text(), "# Hello");
    }

    // ── 7. Word boundary navigation ────────────────────────────────

    #[test]
    fn word_boundary_left_from_end_of_word() {
        let ed = EditorContext::from_content("hello world");
        // pos 11 is end of "world", boundary should be at 6
        assert_eq!(ed.word_boundary_left(11), 6);
    }

    #[test]
    fn word_boundary_left_from_space() {
        let ed = EditorContext::from_content("hello world");
        // pos 5 is space, boundary should go back to 0 (start of "hello")
        assert_eq!(ed.word_boundary_left(5), 0);
    }

    #[test]
    fn word_boundary_left_from_zero() {
        let ed = EditorContext::from_content("hello");
        assert_eq!(ed.word_boundary_left(0), 0);
    }

    #[test]
    fn word_boundary_right_from_start() {
        let ed = EditorContext::from_content("hello world");
        // From 0, should skip "hello" and the space, landing at 6
        assert_eq!(ed.word_boundary_right(0), 6);
    }

    #[test]
    fn word_boundary_right_from_middle_of_word() {
        let ed = EditorContext::from_content("hello world");
        // From 2 (in "hello"), skip rest of word + space, land at 6
        assert_eq!(ed.word_boundary_right(2), 6);
    }

    #[test]
    fn word_boundary_right_at_end() {
        let ed = EditorContext::from_content("hello");
        assert_eq!(ed.word_boundary_right(5), 5);
    }

    #[test]
    fn word_boundary_with_underscores() {
        let ed = EditorContext::from_content("foo_bar baz");
        // "foo_bar" is a single word, boundary_right from 0 => 8
        assert_eq!(ed.word_boundary_right(0), 8);
        // boundary_left from 8 => 0
        assert_eq!(ed.word_boundary_left(8), 0);
    }

    #[test]
    fn word_boundary_with_punctuation() {
        let ed = EditorContext::from_content("hello, world");
        // From 0, skip "hello" then skip ", " => land at 7
        assert_eq!(ed.word_boundary_right(0), 7);
    }

    // ── 8. Clipboard (internal) ────────────────────────────────────

    #[test]
    fn copy_populates_internal_clipboard() {
        let mut ed = EditorContext::from_content("hello world");
        select_range(&mut ed, 0, 0, 5); // select "hello"
        ed.handle_key(ctrl_key(KeyCode::Char('c')));
        assert_eq!(ed.internal_clipboard, "hello");
    }

    #[test]
    fn paste_inserts_from_internal_clipboard() {
        let mut ed = EditorContext::from_content("hello world");
        // Manually set internal clipboard (avoids system clipboard issues)
        ed.internal_clipboard = "PASTED".to_string();
        set_cursor(&mut ed, 0, 5);
        // We call paste logic directly via the internal clipboard to avoid
        // system clipboard interference.
        let text = ed.internal_clipboard.clone();
        let pos = ed.cursor_char_pos();
        let char_count = text.chars().count();
        ed.buffer.insert_str(pos, &text);
        ed.move_cursor_to_char_pos(pos + char_count);
        assert_eq!(ed.text(), "helloPASTED world");
    }

    #[test]
    fn cut_removes_selection_and_populates_clipboard() {
        let mut ed = EditorContext::from_content("hello world");
        select_range(&mut ed, 0, 6, 5); // select "world"
        ed.handle_key(ctrl_key(KeyCode::Char('x')));
        assert_eq!(ed.internal_clipboard, "world");
        assert_eq!(ed.text(), "hello ");
    }

    // ── Additional edge cases ──────────────────────────────────────

    #[test]
    fn esc_returns_exit_editor() {
        let mut ed = EditorContext::from_content("text");
        let action = ed.handle_key(key(KeyCode::Esc));
        assert!(matches!(action, EditorAction::ExitEditor));
    }

    #[test]
    fn ctrl_s_returns_save_file() {
        let mut ed = EditorContext::from_content("text");
        let action = ed.handle_key(ctrl_key(KeyCode::Char('s')));
        assert!(matches!(action, EditorAction::SaveFile));
    }

    #[test]
    fn ctrl_p_returns_toggle_split() {
        let mut ed = EditorContext::from_content("text");
        let action = ed.handle_key(ctrl_key(KeyCode::Char('p')));
        assert!(matches!(action, EditorAction::ToggleSplitPreview));
    }

    #[test]
    fn ctrl_f_activates_search() {
        let mut ed = EditorContext::from_content("text");
        let action = ed.handle_key(ctrl_key(KeyCode::Char('f')));
        assert!(matches!(action, EditorAction::Find));
        assert!(ed.search_active);
    }

    #[test]
    fn shift_arrows_create_selection() {
        let mut ed = EditorContext::from_content("hello");
        set_cursor(&mut ed, 0, 0);
        ed.handle_key(shift_key(KeyCode::Right));
        ed.handle_key(shift_key(KeyCode::Right));
        assert!(ed.state.selection.is_some());
        let (start, end) = ed.state.selected_char_range(ed.buffer.rope()).unwrap();
        assert_eq!(end - start, 2);
    }

    #[test]
    fn tab_inserts_four_spaces() {
        let mut ed = EditorContext::from_content("text");
        set_cursor(&mut ed, 0, 0);
        ed.handle_key(key(KeyCode::Tab));
        assert_eq!(ed.text(), "    text");
        assert_eq!(ed.state.cursor_col, 4);
    }

    #[test]
    fn ctrl_left_jumps_word_boundary() {
        let mut ed = EditorContext::from_content("hello world");
        set_cursor(&mut ed, 0, 11); // end
        ed.handle_key(ctrl_key(KeyCode::Left));
        assert_eq!(ed.state.cursor_col, 6); // start of "world"
    }

    #[test]
    fn ctrl_right_jumps_word_boundary() {
        let mut ed = EditorContext::from_content("hello world");
        set_cursor(&mut ed, 0, 0);
        ed.handle_key(ctrl_key(KeyCode::Right));
        assert_eq!(ed.state.cursor_col, 6); // start of "world"
    }

    #[test]
    fn detect_list_marker_unordered_dash() {
        let result = EditorContext::detect_list_marker("- item");
        assert!(result.is_some());
        let (indent, marker) = result.unwrap();
        assert_eq!(indent, "");
        assert_eq!(marker, "- ");
    }

    #[test]
    fn detect_list_marker_unordered_star() {
        let result = EditorContext::detect_list_marker("* item");
        assert!(result.is_some());
        let (_, marker) = result.unwrap();
        assert_eq!(marker, "* ");
    }

    #[test]
    fn detect_list_marker_ordered() {
        let result = EditorContext::detect_list_marker("1. item");
        assert!(result.is_some());
        let (_, marker) = result.unwrap();
        assert_eq!(marker, "1. ");
    }

    #[test]
    fn detect_list_marker_indented() {
        let result = EditorContext::detect_list_marker("    - nested");
        assert!(result.is_some());
        let (indent, marker) = result.unwrap();
        assert_eq!(indent, "    ");
        assert_eq!(marker, "- ");
    }

    #[test]
    fn detect_list_marker_no_marker() {
        let result = EditorContext::detect_list_marker("plain text");
        assert!(result.is_none());
    }

    #[test]
    fn next_list_marker_increments_ordered() {
        assert_eq!(EditorContext::next_list_marker("1. "), "2. ");
        assert_eq!(EditorContext::next_list_marker("9. "), "10. ");
    }

    #[test]
    fn next_list_marker_preserves_unordered() {
        assert_eq!(EditorContext::next_list_marker("- "), "- ");
        assert_eq!(EditorContext::next_list_marker("* "), "* ");
    }

    #[test]
    fn backspace_deletes_selection() {
        let mut ed = EditorContext::from_content("hello world");
        select_range(&mut ed, 0, 5, 6); // select " world"
        ed.handle_key(key(KeyCode::Backspace));
        assert_eq!(ed.text(), "hello");
    }

    #[test]
    fn delete_deletes_selection() {
        let mut ed = EditorContext::from_content("hello world");
        select_range(&mut ed, 0, 0, 5); // select "hello"
        ed.handle_key(key(KeyCode::Delete));
        assert_eq!(ed.text(), " world");
    }

    #[test]
    fn typing_replaces_selection() {
        let mut ed = EditorContext::from_content("hello world");
        select_range(&mut ed, 0, 0, 5); // select "hello"
        ed.handle_key(key(KeyCode::Char('H')));
        assert_eq!(ed.text(), "H world");
    }

    #[test]
    fn cursor_position_after_multiple_enters() {
        let mut ed = EditorContext::from_content("");
        ed.handle_key(key(KeyCode::Char('a')));
        ed.handle_key(key(KeyCode::Enter));
        ed.handle_key(key(KeyCode::Char('b')));
        ed.handle_key(key(KeyCode::Enter));
        ed.handle_key(key(KeyCode::Char('c')));
        assert_eq!(ed.text(), "a\nb\nc");
        assert_eq!(ed.state.cursor_line, 2);
        assert_eq!(ed.state.cursor_col, 1);
    }

    #[test]
    fn search_key_handling() {
        let mut ed = EditorContext::from_content("hello world");
        ed.search_active = true;
        ed.handle_key(key(KeyCode::Char('h')));
        ed.handle_key(key(KeyCode::Char('e')));
        assert_eq!(ed.search_query, "he");
        ed.handle_key(key(KeyCode::Backspace));
        assert_eq!(ed.search_query, "h");
        ed.handle_key(key(KeyCode::Esc));
        assert!(!ed.search_active);
    }

    #[test]
    fn heading_toggle_round_trip() {
        let mut ed = EditorContext::from_content("Hello");
        set_cursor(&mut ed, 0, 0);
        ed.toggle_heading(2);
        assert_eq!(ed.text(), "## Hello");
        ed.toggle_heading(2);
        assert_eq!(ed.text(), "Hello");
    }

    #[test]
    fn auto_close_paren_cursor_between() {
        let mut ed = EditorContext::from_content("");
        ed.handle_key(key(KeyCode::Char('(')));
        // Cursor should be between ( and )
        assert_eq!(ed.state.cursor_col, 1);
        // Typing inside the pair
        ed.handle_key(key(KeyCode::Char('x')));
        assert_eq!(ed.text(), "(x)");
    }

    #[test]
    fn inline_marker_on_multiline_selection() {
        let mut ed = EditorContext::from_content("hello\nworld");
        // Select from (0,0) to (0,5) — just "hello"
        select_range(&mut ed, 0, 0, 5);
        ed.toggle_inline_marker("**");
        assert_eq!(ed.text(), "**hello**\nworld");
    }

    // ── 10. Find-and-replace ─────────────────────────────────────────

    #[test]
    fn tab_activates_replace_in_search_mode() {
        let mut ed = EditorContext::from_content("text");
        ed.search_active = true;
        assert!(!ed.replace_active);
        ed.handle_key(key(KeyCode::Tab));
        assert!(ed.replace_active);
        assert!(ed.replace_focused);
    }

    #[test]
    fn tab_toggles_focus_between_find_and_replace() {
        let mut ed = EditorContext::from_content("text");
        ed.search_active = true;
        ed.replace_active = true;
        ed.replace_focused = false;
        ed.handle_key(key(KeyCode::Tab));
        assert!(ed.replace_focused);
        ed.handle_key(key(KeyCode::Tab));
        assert!(!ed.replace_focused);
    }

    #[test]
    fn replace_focused_routes_chars_to_replace_query() {
        let mut ed = EditorContext::from_content("text");
        ed.search_active = true;
        ed.replace_active = true;
        ed.replace_focused = true;
        ed.handle_key(key(KeyCode::Char('a')));
        ed.handle_key(key(KeyCode::Char('b')));
        assert_eq!(ed.replace_query, "ab");
        assert!(ed.search_query.is_empty());
    }

    #[test]
    fn replace_current_match_replaces_and_advances() {
        let mut ed = EditorContext::from_content("foo bar foo baz");
        ed.search_query = "foo".to_string();
        ed.replace_query = "qux".to_string();
        set_cursor(&mut ed, 0, 0);
        ed.replace_current_match();
        // First "foo" replaced; cursor should be past replacement.
        let text = ed.text();
        assert!(text.starts_with("qux bar "), "got: {text}");
    }

    #[test]
    fn replace_all_matches_replaces_every_occurrence() {
        let mut ed = EditorContext::from_content("foo bar foo baz foo");
        ed.search_query = "foo".to_string();
        ed.replace_query = "x".to_string();
        ed.replace_all_matches();
        assert_eq!(ed.text(), "x bar x baz x");
    }

    #[test]
    fn replace_all_with_no_matches_is_noop() {
        let mut ed = EditorContext::from_content("hello world");
        ed.search_query = "xyz".to_string();
        ed.replace_query = "abc".to_string();
        ed.replace_all_matches();
        assert_eq!(ed.text(), "hello world");
    }

    #[test]
    fn replace_all_with_longer_replacement() {
        let mut ed = EditorContext::from_content("a b a");
        ed.search_query = "a".to_string();
        ed.replace_query = "longer".to_string();
        ed.replace_all_matches();
        assert_eq!(ed.text(), "longer b longer");
    }

    #[test]
    fn esc_closes_replace_bar() {
        let mut ed = EditorContext::from_content("text");
        ed.search_active = true;
        ed.replace_active = true;
        ed.replace_focused = true;
        ed.handle_key(key(KeyCode::Esc));
        assert!(!ed.search_active);
        assert!(!ed.replace_active);
        assert!(!ed.replace_focused);
    }

    #[test]
    fn ctrl_f_resets_replace_state() {
        let mut ed = EditorContext::from_content("text");
        ed.handle_key(ctrl_key(KeyCode::Char('f')));
        assert!(ed.search_active);
        assert!(!ed.replace_active);
        assert!(ed.replace_query.is_empty());
        assert!(!ed.replace_focused);
    }

    // ── Workflow / integration tests ─────────────────────────────────

    /// Type a string into the editor character by character, translating
    /// `\n` into Enter key presses.
    fn type_str(editor: &mut EditorContext, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                editor.handle_key(key(KeyCode::Enter));
            } else {
                editor.handle_key(key(KeyCode::Char(c)));
            }
        }
    }

    #[test]
    fn workflow_write_markdown_document() {
        let mut ed = EditorContext::from_content("");

        // Type a heading and blank line.
        type_str(&mut ed, "# My Document\n\n");
        // Type a paragraph with bold text.
        type_str(&mut ed, "This is a paragraph with **bold** text.\n\n");
        // Type a sub-heading and blank line.
        type_str(&mut ed, "## Section One\n\n");
        // Start an unordered list.
        type_str(&mut ed, "- First item\n");
        // Enter at end of "- First item" auto-continues the list with "- ".
        // Now type the second item's content (cursor is after "- ").
        type_str(&mut ed, "Second item\n");
        // Enter at end of "- Second item" auto-continues with "- " again.
        // Now press Enter on the empty list item to remove the marker.
        type_str(&mut ed, "\n");
        // Type trailing text after the cleared list marker.
        type_str(&mut ed, "Back to normal.");

        let text = ed.text();
        let lines: Vec<&str> = text.lines().collect();

        assert_eq!(lines[0], "# My Document");
        assert_eq!(lines[1], "");
        assert_eq!(lines[2], "This is a paragraph with **bold** text.");
        assert_eq!(lines[3], "");
        assert_eq!(lines[4], "## Section One");
        assert_eq!(lines[5], "");
        assert_eq!(lines[6], "- First item");
        assert_eq!(lines[7], "- Second item");
        // Line 8: the empty list marker was removed (no newline inserted),
        // so "Back to normal." lands directly on this line.
        assert_eq!(lines[8], "Back to normal.");
    }

    #[test]
    fn workflow_edit_existing_text() {
        let mut ed = EditorContext::from_content("Hello World\nThis is a test.");

        // Move cursor to line 1.
        ed.handle_key(key(KeyCode::Down));
        assert_eq!(ed.state.cursor_line, 1);
        assert_eq!(ed.state.cursor_col, 0);

        // Move right 5 times to land on 'i' of "is" (col 5: "This " = 5 chars).
        for _ in 0..5 {
            ed.handle_key(key(KeyCode::Right));
        }
        assert_eq!(ed.state.cursor_col, 5);

        // Shift+Right twice to select "is".
        ed.handle_key(shift_key(KeyCode::Right));
        ed.handle_key(shift_key(KeyCode::Right));
        assert!(ed.state.selection.is_some());

        // Type "was" to replace the selection.
        type_str(&mut ed, "was");
        assert_eq!(ed.text(), "Hello World\nThis was a test.");

        // Undo: each typed char is a separate undo entry, plus the
        // selection deletion. Undo 's', 'a', 'w', then restore "is".
        ed.handle_key(ctrl_key(KeyCode::Char('z'))); // undo 's'
        ed.handle_key(ctrl_key(KeyCode::Char('z'))); // undo 'a'
        ed.handle_key(ctrl_key(KeyCode::Char('z'))); // undo 'w'
        ed.handle_key(ctrl_key(KeyCode::Char('z'))); // undo delete of "is"
        assert_eq!(ed.text(), "Hello World\nThis is a test.");

        // Redo: replay the 4 operations.
        ed.handle_key(ctrl_key(KeyCode::Char('y'))); // redo delete of "is"
        ed.handle_key(ctrl_key(KeyCode::Char('y'))); // redo 'w'
        ed.handle_key(ctrl_key(KeyCode::Char('y'))); // redo 'a'
        ed.handle_key(ctrl_key(KeyCode::Char('y'))); // redo 's'
        assert_eq!(ed.text(), "Hello World\nThis was a test.");
    }

    #[test]
    fn workflow_find_and_replace() {
        let mut ed = EditorContext::from_content(
            "The quick brown fox\nThe quick red fox\nThe slow brown fox",
        );

        // Activate search.
        ed.handle_key(ctrl_key(KeyCode::Char('f')));
        assert!(ed.search_active);

        // Type the search query.
        type_str(&mut ed, "quick");
        assert_eq!(ed.search_query, "quick");

        // Verify matches exist in the buffer.
        assert!(ed.text().contains("quick"));
        assert_eq!(ed.text().matches("quick").count(), 2);

        // Activate replace (Tab while search is active).
        ed.handle_key(key(KeyCode::Tab));
        assert!(ed.replace_active);
        assert!(ed.replace_focused);

        // Type the replacement text.
        type_str(&mut ed, "fast");
        assert_eq!(ed.replace_query, "fast");

        // Replace current match (Enter while replace is focused).
        ed.handle_key(key(KeyCode::Enter));
        let text = ed.text();
        // First "quick" should be replaced.
        assert!(
            text.starts_with("The fast brown fox"),
            "first match replaced: got {text}",
        );
        // Second "quick" should still be there.
        assert_eq!(text.matches("quick").count(), 1);

        // Replace all remaining matches.
        ed.handle_key(ctrl_key(KeyCode::Char('a')));
        let text = ed.text();
        assert_eq!(text.matches("quick").count(), 0, "all matches replaced");
        assert_eq!(text.matches("fast").count(), 2, "both now say fast");
        assert_eq!(
            text,
            "The fast brown fox\nThe fast red fox\nThe slow brown fox",
        );
    }

    #[test]
    fn workflow_markdown_formatting() {
        let mut ed = EditorContext::from_content("Some plain text here.");

        // Select "plain" (starts at col 5, length 5).
        select_range(&mut ed, 0, 5, 5);

        // Ctrl+B to wrap in bold.
        ed.handle_key(ctrl_key(KeyCode::Char('b')));
        assert_eq!(ed.text(), "Some **plain** text here.");

        // After wrapping, toggle_inline_marker selects the wrapped text
        // "**plain**". Pressing Ctrl+B again should unwrap it.
        ed.handle_key(ctrl_key(KeyCode::Char('b')));
        assert_eq!(ed.text(), "Some plain text here.");

        // After unwrapping, the selection covers "plain".
        // Now apply italic with Ctrl+I.
        ed.handle_key(ctrl_key(KeyCode::Char('i')));
        assert_eq!(ed.text(), "Some *plain* text here.");
    }

    #[test]
    fn workflow_heading_management() {
        let mut ed = EditorContext::from_content("My Title\nSome content");

        // Position cursor on first line.
        set_cursor(&mut ed, 0, 0);

        // Ctrl+1 adds h1 prefix.
        ed.handle_key(ctrl_key(KeyCode::Char('1')));
        assert_eq!(ed.text(), "# My Title\nSome content",);

        // Ctrl+2 changes heading from h1 to h2.
        ed.handle_key(ctrl_key(KeyCode::Char('2')));
        assert_eq!(ed.text(), "## My Title\nSome content",);

        // Ctrl+2 again on same level removes the heading.
        ed.handle_key(ctrl_key(KeyCode::Char('2')));
        assert_eq!(ed.text(), "My Title\nSome content",);

        // Verify second line was never affected.
        let final_text = ed.text();
        let lines: Vec<&str> = final_text.lines().collect();
        assert_eq!(lines[1], "Some content");
    }
}
