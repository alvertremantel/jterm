use color_eyre::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use jones_editor::{EditorAction, EditorContext};
use jones_event::{AppEvent, EventHandler};
use jones_outline::{self as outline, OutlineEntry};
use jones_render::{RenderedDocument, render_markdown_mapped};
use jones_text;
use jones_theme as theme;
use jones_workspace::{self as workspace, WorkspaceEntry, WorkspaceOptions, WorkspaceSortMode};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use writerm_config::Config;

use crate::metrics::{DocumentMetrics, compute as compute_metrics};

/// Layout for the heading-marker gutter inside the document area. The
/// gutter displays ATX `#` markers for heading rows in a subtle style,
/// with capacity adapting to the deepest heading in the document.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeadingGutterLayout {
    pub gutter_cells: u16,
    pub blank_after: u16,
    pub text_x_offset: u16,
    pub text_width: u16,
}

impl HeadingGutterLayout {
    pub fn for_area(area_width: u16, max_heading_depth: usize) -> Self {
        let right_margin = 1u16;
        let max_depth = max_heading_depth as u16;
        // When there are no headings, or when the area is too narrow to
        // fit the gutter, suppress it entirely.
        if max_depth == 0 || area_width < max_depth + 3 {
            Self {
                gutter_cells: 0,
                blank_after: 0,
                text_x_offset: 0,
                text_width: area_width.saturating_sub(right_margin).max(1),
            }
        } else {
            Self {
                gutter_cells: max_depth,
                blank_after: 1,
                text_x_offset: max_depth + 1,
                text_width: area_width - max_depth - 1 - right_margin,
            }
        }
    }
}

/// A single visual row inside the headings/section-browser panel, built
/// from one `OutlineEntry`. Long labels are wrapped across multiple rows.
#[derive(Debug, Clone)]
pub(crate) struct HeadingVisualLine {
    pub entry_idx: usize,
    pub content: String,
    pub style: ratatui::style::Style,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMode {
    NewFile,
}

pub struct WritermApp {
    pub config: Config,
    pub cwd: PathBuf,
    pub current_file_path: PathBuf,
    pub editor: EditorContext,
    pub rendered: RenderedDocument,
    rendered_version: u64,
    cached_visual: crate::visual::VisualDocument,
    cached_visual_version: u64,
    cached_visual_width: usize,
    cached_visual_source_peek: bool,
    cached_visual_paragraph_indent: bool,
    pub outline_entries: Vec<OutlineEntry>,
    pub workspace_entries: Vec<WorkspaceEntry>,
    pub workspace_summary: workspace::WorkspaceSummary,
    pub workspace_options: WorkspaceOptions,
    pub workspace_selection: usize,
    pub workspace_scroll: u16,
    pub workspace_viewport_rows: usize,
    pub show_headings: bool,
    pub show_files: bool,
    pub source_peek: bool,
    pub paragraph_indent: bool,
    pub document_scroll: usize,
    pub heading_scroll: u16,
    pub prompt_mode: Option<PromptMode>,
    pub prompt_buffer: String,
    pub notification: Option<(String, Instant, bool)>,
    pub running: bool,
    pub headings_area: Rect,
    pub document_area: Rect,
    pub files_area: Rect,
    pub metrics_area: Rect,
    pub headings_control_area: Rect,
    pub files_control_area: Rect,
    pub drag_selecting: bool,
    desired_display_col: Option<usize>,
    last_edit: Option<Instant>,
    needs_redraw: bool,
    /// Cache: buffer version when `word_byte_starts` was built.
    word_progress_version: u64,
    /// Sorted byte-start offsets of every whitespace-delimited word in
    /// the current buffer text.  Rebuilt once per buffer version.
    word_byte_starts: Vec<usize>,
}

impl WritermApp {
    pub fn new(maybe_path: Option<PathBuf>) -> Result<Self> {
        Self::with_config(maybe_path, Config::load()?)
    }

    pub fn with_config(maybe_path: Option<PathBuf>, config: Config) -> Result<Self> {
        let (cwd, open_file) = resolve_launch_target(maybe_path);
        let file = open_file
            .or_else(|| pick_default_markdown_file(&cwd))
            .unwrap_or_else(|| cwd.join("index.md"));
        ensure_file_exists(&file)?;
        let content = std::fs::read_to_string(&file)?;
        let editor = EditorContext::from_content(&content);
        let rendered = render_markdown_mapped(&content);
        let outline_entries = outline::extract_outline(Some(&file), &content);
        let source_peek = !is_markdown_path(&file);
        let paragraph_indent = config.layout.paragraph_indent;
        let mut workspace_options = WorkspaceOptions {
            show_hidden: config.workspace.show_hidden,
            sort_mode: WorkspaceSortMode::AlphaDirsFirst,
            ..WorkspaceOptions::default()
        };
        workspace_options.filter.clear();
        let (mut workspace_entries, workspace_summary) =
            workspace::list_workspace_entries(&cwd, &workspace_options, &[]);
        sort_writerm_entries(&mut workspace_entries, config.workspace.markdown_first);

        Ok(Self {
            config,
            cwd,
            current_file_path: file,
            editor,
            rendered,
            rendered_version: 0,
            cached_visual: crate::visual::VisualDocument { rows: Vec::new() },
            cached_visual_version: u64::MAX,
            cached_visual_width: 0,
            cached_visual_source_peek: !source_peek,
            cached_visual_paragraph_indent: paragraph_indent,
            outline_entries,
            workspace_entries,
            workspace_summary,
            workspace_options,
            workspace_selection: 0,
            workspace_scroll: 0,
            workspace_viewport_rows: 1,
            show_headings: true,
            show_files: true,
            source_peek,
            paragraph_indent,
            document_scroll: 0,
            heading_scroll: 0,
            prompt_mode: None,
            prompt_buffer: String::new(),
            notification: None,
            running: true,
            headings_area: Rect::default(),
            document_area: Rect::default(),
            files_area: Rect::default(),
            metrics_area: Rect::default(),
            headings_control_area: Rect::default(),
            files_control_area: Rect::default(),
            drag_selecting: false,
            desired_display_col: None,
            last_edit: None,
            needs_redraw: true,
            word_progress_version: u64::MAX,
            word_byte_starts: Vec::new(),
        })
    }

    pub async fn run<B>(&mut self, terminal: &mut Terminal<B>) -> Result<()>
    where
        B: Backend,
        B::Error: Send + Sync + 'static,
    {
        let (mut events, _tx) = EventHandler::<()>::new(Duration::from_millis(250));

        while self.running {
            self.refresh_render_cache();
            if self.needs_redraw {
                terminal.draw(|frame| crate::draw::draw(frame, self))?;
                self.needs_redraw = false;
            }
            if let Some(event) = events.next().await {
                self.handle_event(event);
            }
        }
        Ok(())
    }

    pub fn handle_event(&mut self, event: AppEvent<()>) {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Mouse(mouse) => {
                if !self.config.ui.mouse {
                    return;
                }
                self.needs_redraw = true;
                self.handle_mouse(mouse);
            }
            AppEvent::Resize(_, _) => self.needs_redraw = true,
            AppEvent::Tick => self.handle_tick(),
            AppEvent::Custom(()) => {}
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.needs_redraw = true;

        if self.prompt_mode.is_some() {
            self.handle_prompt_key(key);
            return;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('q') if ctrl => {
                self.quit();
            }
            KeyCode::Char('s') if ctrl => {
                self.save_now();
            }
            KeyCode::Char('m') if ctrl => {
                self.source_peek = !self.source_peek;
                self.desired_display_col = None;
                self.ensure_cursor_visible();
                self.notification = Some((
                    if self.source_peek {
                        "Source peek on".into()
                    } else {
                        "Rendered editing".into()
                    },
                    Instant::now(),
                    false,
                ));
            }
            KeyCode::F(2) => self.show_files = !self.show_files,
            KeyCode::F(3) => self.show_headings = !self.show_headings,
            KeyCode::F(4) => {
                self.paragraph_indent = !self.paragraph_indent;
                self.notification = Some((
                    if self.paragraph_indent {
                        "Paragraph indent on".into()
                    } else {
                        "Paragraph indent off".into()
                    },
                    Instant::now(),
                    false,
                ));
            }
            KeyCode::Char('n') if ctrl => {
                self.prompt_mode = Some(PromptMode::NewFile);
                self.prompt_buffer.clear();
            }
            KeyCode::PageUp => {
                self.move_visual_page(-1, shift);
            }
            KeyCode::PageDown => {
                self.move_visual_page(1, shift);
            }
            KeyCode::Left if !self.source_peek && ctrl && !alt => {
                self.move_visual_word(-1, shift);
            }
            KeyCode::Right if !self.source_peek && ctrl && !alt => {
                self.move_visual_word(1, shift);
            }
            KeyCode::Left if !self.source_peek && !ctrl && !alt => {
                self.move_visual_horizontal(-1, shift);
            }
            KeyCode::Right if !self.source_peek && !ctrl && !alt => {
                self.move_visual_horizontal(1, shift);
            }
            KeyCode::Home if !self.source_peek && !ctrl && !alt => {
                self.move_visual_line_boundary(false, shift);
            }
            KeyCode::End if !self.source_peek && !ctrl && !alt => {
                self.move_visual_line_boundary(true, shift);
            }
            KeyCode::Up if !ctrl && !alt => self.move_visual_vertical(-1, shift),
            KeyCode::Down if !ctrl && !alt => self.move_visual_vertical(1, shift),
            KeyCode::Up | KeyCode::Down if ctrl || alt => {}
            // When paragraph indent is enabled and we're not in source-peek
            // mode, Backspace at the visible text-start of an indented row
            // atomically deletes the hidden separator (newline + leading
            // whitespace) so the action produces an immediate visible change
            // instead of a no-op press.
            KeyCode::Backspace
                if !self.source_peek
                    && !ctrl
                    && !alt
                    && self.paragraph_indent
                    && self.handle_indented_backspace() => {}
            _ => self.handle_editor_key(key),
        }
    }

    /// When paragraph indent is on, the cursor sits at the visible
    /// text-start of an indented row, and the hidden separator between
    /// this row and the previous indented-prose row is a single newline
    /// followed only by spaces/tabs, one Backspace atomically deletes that
    /// separator so the user sees an immediate visible change (soft-break
    /// line merge).  Returns `true` if handled; `false` means the caller
    /// must forward the key to the normal editor path.
    ///
    /// Safety guards:
    /// - Never handles when a selection exists (falls through to editor).
    /// - The previous visual row must be indented prose (has a synthetic
    ///   prefix).  Structural rows, blank lines, and continuation rows
    ///   are not crossed.
    /// - The gap between the previous row's `source_end` and the cursor
    ///   must contain exactly one newline (`\n` or `\r\n`) followed only
    ///   by ASCII spaces and/or tabs.  Blank-line paragraph breaks
    ///   (`\n\n`) and non-whitespace characters are rejected.
    fn handle_indented_backspace(&mut self) -> bool {
        // Never interfere with selection deletion.
        if self.editor.state.selection.is_some() {
            return false;
        }

        self.refresh_render_cache();
        let visual = self.visual_document();
        let cursor = self.editor.cursor_char_pos();
        let Some((row, col)) = visual.source_to_display(cursor) else {
            return false;
        };

        // Must be exactly at the text-start of an indented row.
        let prefix = visual.row_prefix_width(row).unwrap_or(0);
        if prefix == 0 || col != prefix {
            return false;
        }

        // Previous visual row must be indented prose (has its own
        // synthetic prefix).  Structural rows, blank lines, and
        // wrapping continuation rows have prefix_width == 0 and are
        // rejected by `prev_prose_row_end`.
        let Some(prev_end) = visual.prev_prose_row_end(row) else {
            return false;
        };

        if cursor <= prev_end {
            return false;
        }

        // Validate the gap: one newline followed only by spaces/tabs.
        // prev_end and cursor are char offsets but String slicing uses
        // byte offsets, so convert to byte offsets first to avoid
        // panicking on multi-byte (non-ASCII) characters.
        let text = self.editor.text();
        let prev_end_byte = jones_text::nth_char_byte_offset(&text, prev_end);
        let cursor_byte = jones_text::nth_char_byte_offset(&text, cursor);
        let gap = &text[prev_end_byte..cursor_byte];
        if !is_soft_break_separator(gap) {
            return false;
        }

        // Atomic deletion — one editor transaction, one undo step.
        let delete_len = cursor - prev_end;
        self.editor.buffer.delete_range(prev_end, delete_len);
        self.editor.move_cursor_to_char_pos(prev_end);
        self.last_edit = Some(Instant::now());
        self.desired_display_col = None;
        self.refresh_document_metadata();
        self.ensure_cursor_visible();
        true
    }

    fn handle_editor_key(&mut self, key: KeyEvent) {
        self.desired_display_col = None;
        let version_before = self.editor.buffer.version();
        self.editor.viewport_height = self.document_area.height.max(1) as usize;
        let action = self.editor.handle_key(key);

        if self.editor.buffer.version() != version_before {
            self.last_edit = Some(Instant::now());
            self.refresh_document_metadata();
        }

        match action {
            EditorAction::SaveFile => {
                self.save_now();
            }
            EditorAction::Find => {}
            EditorAction::ExitEditor
            | EditorAction::ToggleSplitPreview
            | EditorAction::ReloadFile
            | EditorAction::None => {}
        }
        self.ensure_cursor_visible();
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.prompt_mode = None;
                self.prompt_buffer.clear();
            }
            KeyCode::Backspace => {
                self.prompt_buffer.pop();
            }
            KeyCode::Enter => {
                if matches!(self.prompt_mode, Some(PromptMode::NewFile)) {
                    let name = markdown_filename(&self.prompt_buffer);
                    self.prompt_mode = None;
                    self.prompt_buffer.clear();
                    match name {
                        Ok(name) => {
                            if !name.is_empty() {
                                let path = self.cwd.join(name);
                                self.open_or_create_file(&path);
                            }
                        }
                        Err(message) => {
                            self.notification = Some((message, Instant::now(), true));
                        }
                    }
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.prompt_buffer.push(c);
            }
            _ => {}
        }
    }

    fn handle_tick(&mut self) {
        let now = Instant::now();
        if let Some((_, at, _)) = &self.notification
            && now.duration_since(*at) > Duration::from_secs(3)
        {
            self.notification = None;
            self.needs_redraw = true;
        }
        if self.config.autosave.enabled
            && self.editor.is_dirty()
            && self.last_edit.is_some_and(|edit| {
                now.duration_since(edit).as_millis() >= self.config.autosave.delay_ms as u128
            })
            && !self.save_now()
        {
            self.last_edit = Some(now);
        }
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.prompt_mode.is_some() {
                    return;
                }
                if point_in(self.headings_control_area, mouse.column, mouse.row) {
                    self.show_headings = !self.show_headings;
                } else if point_in(self.files_control_area, mouse.column, mouse.row) {
                    self.show_files = !self.show_files;
                } else if point_in(self.headings_area, mouse.column, mouse.row) {
                    self.click_heading(mouse.row);
                } else if point_in(self.files_area, mouse.column, mouse.row) {
                    self.click_file(mouse.row);
                } else if point_in(self.document_area, mouse.column, mouse.row) {
                    self.click_document(mouse.column, mouse.row, false);
                    self.drag_selecting = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.drag_selecting => {
                self.click_document(mouse.column, mouse.row, true);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag_selecting = false;
            }
            MouseEventKind::ScrollUp => {
                self.scroll_document(-3);
            }
            MouseEventKind::ScrollDown => {
                self.scroll_document(3);
            }
            _ => {}
        }
    }

    fn click_heading(&mut self, row: u16) {
        let rel = row.saturating_sub(self.headings_area.y) as usize;
        let visual_idx = self.heading_scroll as usize + rel;
        let lines = self.build_heading_visual_lines(self.headings_area.width);
        if let Some(line) = lines.get(visual_idx) {
            let entry = &self.outline_entries[line.entry_idx];
            let pos = self.editor.buffer.rope().line_to_char(entry.line);
            self.editor.move_cursor_to_char_pos(pos);
            self.ensure_cursor_visible();
        }
    }

    fn click_file(&mut self, row: u16) {
        let rel = row.saturating_sub(self.files_area.y) as usize;
        let idx = self.workspace_scroll as usize + rel;
        if idx >= self.workspace_entries.len() {
            return;
        }
        self.workspace_selection = idx;
        let entry = self.workspace_entries[idx].clone();
        match entry.kind {
            workspace::WorkspaceEntryKind::Parent => {
                if let Some(parent) = self.cwd.parent().map(Path::to_path_buf) {
                    self.change_cwd(parent);
                }
            }
            workspace::WorkspaceEntryKind::Directory => self.change_cwd(self.cwd.join(entry.name)),
            workspace::WorkspaceEntryKind::File => {
                let path = self.cwd.join(entry.name);
                self.open_or_create_file(&path);
            }
        }
    }

    fn click_document(&mut self, col: u16, row: u16, extend_selection: bool) {
        self.refresh_render_cache();
        let rel_row = row.saturating_sub(self.document_area.y) as usize;
        let raw_rel_col = col.saturating_sub(self.document_area.x) as usize;
        // Adjust for the heading-marker gutter so the display column is
        // relative to the text area (not the full document area).
        let layout = self.heading_gutter_layout();
        let gutter_offset = (layout.gutter_cells + layout.blank_after) as usize;
        let rel_col = raw_rel_col.saturating_sub(gutter_offset);
        let display_row = self.document_scroll + rel_row;
        let char_pos = self
            .visual_document()
            .display_to_source(display_row, rel_col)
            .unwrap_or_else(|| self.editor.cursor_char_pos());

        if extend_selection {
            if self.editor.state.selection.is_none() {
                self.editor.state.start_selection();
            }
            self.editor.move_cursor_to_char_pos(char_pos);
            self.editor.state.extend_selection();
        } else {
            self.editor.state.clear_selection();
            self.editor.move_cursor_to_char_pos(char_pos);
        }
        self.desired_display_col = None;
        self.ensure_cursor_visible();
    }

    fn quit(&mut self) {
        if self.editor.is_dirty() && !self.save_now() {
            return;
        }
        self.running = false;
    }

    pub fn save_now(&mut self) -> bool {
        match self.editor.save(&self.current_file_path) {
            Ok(()) => {
                self.last_edit = None;
                self.notification = Some(("Saved".into(), Instant::now(), false));
                self.needs_redraw = true;
                true
            }
            Err(err) => {
                self.notification = Some((format!("Save failed: {err}"), Instant::now(), true));
                self.needs_redraw = true;
                false
            }
        }
    }

    pub fn open_or_create_file(&mut self, path: &Path) -> bool {
        if self.editor.is_dirty() && !self.save_now() {
            return false;
        }
        let path = absolute_path(path);
        if let Err(err) = ensure_file_exists(&path) {
            self.notification = Some((format!("Create failed: {err}"), Instant::now(), true));
            return false;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            self.notification = Some((
                format!("Cannot open {}", path.display()),
                Instant::now(),
                true,
            ));
            return false;
        };
        self.cwd = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.cwd.clone());
        self.current_file_path = path;
        self.editor = EditorContext::from_content(&content);
        // A newly opened buffer starts at version 0, which may equal the
        // version of the previous file's word-progress cache. Invalidate it
        // explicitly so counts cannot be reused across documents.
        self.word_progress_version = u64::MAX;
        self.word_byte_starts.clear();
        self.document_scroll = 0;
        self.heading_scroll = 0;
        self.desired_display_col = None;
        self.source_peek = !is_markdown_path(&self.current_file_path);
        self.refresh_workspace();
        self.refresh_document_metadata();
        self.refresh_render_cache_force();
        self.cached_visual_version = u64::MAX;
        self.notification = Some(("Opened".into(), Instant::now(), false));
        true
    }

    fn move_visual_page(&mut self, delta: isize, extend_selection: bool) {
        let jump = self.document_area.height.max(1) as isize;
        self.move_visual_rows(delta.saturating_mul(jump), extend_selection, true);
    }

    fn move_visual_vertical(&mut self, delta: isize, extend_selection: bool) {
        self.move_visual_rows(delta, extend_selection, false);
    }

    fn move_visual_horizontal(&mut self, delta: isize, extend_selection: bool) {
        self.refresh_render_cache();
        let visual = self.visual_document();
        let current = self.editor.cursor_char_pos();
        let Some((row, col)) = visual.source_to_display(current) else {
            return;
        };
        let prefix = visual.row_prefix_width(row).unwrap_or(0);
        let target = match delta.cmp(&0) {
            std::cmp::Ordering::Less => {
                if col > prefix {
                    visual.display_to_source(row, col - 1)
                } else {
                    row.checked_sub(1)
                        .and_then(|row| visual.display_to_source(row, usize::MAX))
                }
            }
            std::cmp::Ordering::Equal => Some(current),
            std::cmp::Ordering::Greater => {
                let visible_at_cursor = visual.display_to_source(row, col);
                if let Some(source) = visible_at_cursor
                    && current < source
                {
                    Some(source)
                } else {
                    next_visual_source_after(&visual, row, col, current)
                }
            }
        };
        let Some(char_pos) = target else {
            return;
        };
        self.move_visual_cursor_to(char_pos, extend_selection);
        self.desired_display_col = None;
        self.ensure_cursor_visible();
    }

    fn move_visual_word(&mut self, delta: isize, extend_selection: bool) {
        self.refresh_render_cache();
        let visual = self.visual_document();
        let current = self.editor.cursor_char_pos();
        let Some((row, col)) = visual.source_to_display(current) else {
            return;
        };
        let target = match delta.cmp(&0) {
            std::cmp::Ordering::Less => {
                visual_word_boundary_left(&visual, row, col).unwrap_or(current)
            }
            std::cmp::Ordering::Equal => current,
            std::cmp::Ordering::Greater => {
                visual_word_boundary_right(&visual, row, col).unwrap_or(current)
            }
        };
        if target == current {
            return;
        }

        self.move_visual_cursor_to(target, extend_selection);
        self.desired_display_col = None;
        self.ensure_cursor_visible();
    }

    fn move_visual_line_boundary(&mut self, end: bool, extend_selection: bool) {
        self.refresh_render_cache();
        let visual = self.visual_document();
        let current = self.editor.cursor_char_pos();
        let Some((row, _)) = visual.source_to_display(current) else {
            return;
        };
        let col = if end {
            visual.row_width(row).unwrap_or_default()
        } else {
            // Start-of-line lands at the first text character, skipping
            // any synthetic indent prefix so Home goes to the visible
            // text start rather than into the indent gutter.
            visual.row_prefix_width(row).unwrap_or(0)
        };
        let Some(char_pos) = visual.display_to_source(row, col) else {
            return;
        };
        self.move_visual_cursor_to(char_pos, extend_selection);
        self.desired_display_col = None;
        self.ensure_cursor_visible();
    }

    fn move_visual_rows(&mut self, delta: isize, extend_selection: bool, clamp: bool) {
        self.refresh_render_cache();
        let visual = self.visual_document();
        let Some((row, col)) = visual.source_to_display(self.editor.cursor_char_pos()) else {
            return;
        };
        if visual.rows.is_empty() {
            return;
        }
        let target_col = self.desired_display_col.unwrap_or(col);
        let clamped_to_boundary = match delta.cmp(&0) {
            std::cmp::Ordering::Less => clamp && row.checked_sub(delta.unsigned_abs()).is_none(),
            std::cmp::Ordering::Equal => false,
            std::cmp::Ordering::Greater => {
                clamp && row.saturating_add(delta as usize) >= visual.rows.len().saturating_sub(1)
            }
        };
        let target_row = match delta.cmp(&0) {
            std::cmp::Ordering::Less => {
                let target = row.checked_sub(delta.unsigned_abs());
                if clamp { target.or(Some(0)) } else { target }
            }
            std::cmp::Ordering::Equal => Some(row),
            std::cmp::Ordering::Greater => {
                let target = row.saturating_add(delta as usize);
                if clamp {
                    Some(target.min(visual.rows.len().saturating_sub(1)))
                } else {
                    Some(target)
                }
            }
        };
        let Some(target_row) = target_row else {
            return;
        };
        if target_row >= visual.rows.len() {
            return;
        }
        let Some(char_pos) = mapped_char_near_visual_row(
            &visual,
            target_row,
            target_col,
            delta,
            clamped_to_boundary,
        ) else {
            return;
        };

        self.move_visual_cursor_to(char_pos, extend_selection);
        self.desired_display_col = Some(target_col);
        self.ensure_cursor_visible();
    }

    fn move_visual_cursor_to(&mut self, char_pos: usize, extend_selection: bool) {
        if extend_selection {
            if self.editor.state.selection.is_none() {
                self.editor.state.start_selection();
            }
            self.editor.move_cursor_to_char_pos(char_pos);
            self.editor.state.extend_selection();
        } else {
            self.editor.state.clear_selection();
            self.editor.move_cursor_to_char_pos(char_pos);
        }
    }

    pub fn change_cwd(&mut self, path: PathBuf) {
        let cwd = path.canonicalize().unwrap_or(path);
        if cwd.is_dir() {
            self.cwd = cwd;
            self.refresh_workspace();
        }
    }

    pub fn refresh_workspace(&mut self) {
        let (mut entries, summary) =
            workspace::list_workspace_entries(&self.cwd, &self.workspace_options, &[]);
        sort_writerm_entries(&mut entries, self.config.workspace.markdown_first);
        self.workspace_entries = entries;
        self.workspace_summary = summary;
        self.workspace_selection = self
            .workspace_selection
            .min(self.workspace_entries.len().saturating_sub(1));
        self.workspace_scroll = 0;
    }

    pub fn refresh_document_metadata(&mut self) {
        let text = self.editor.text();
        self.outline_entries = outline::extract_outline(Some(&self.current_file_path), &text);
    }

    pub fn refresh_render_cache(&mut self) {
        let version = self.editor.buffer.version();
        if self.rendered_version != version {
            self.refresh_render_cache_force();
        }
    }

    fn refresh_render_cache_force(&mut self) {
        let text = self.editor.text();
        self.rendered = render_markdown_mapped(&text);
        self.rendered_version = self.editor.buffer.version();
    }

    fn refresh_visual_cache(&mut self) {
        let version = self.editor.buffer.version();
        let layout = self.heading_gutter_layout();
        let wrap_width = layout.text_width.max(1) as usize;
        let indent_gutter = if self.paragraph_indent && !self.source_peek && wrap_width >= 3 {
            2usize
        } else {
            0usize
        };
        // The visual document's wrap width is reduced so that indented
        // first rows stay within the text area; the indent itself is
        // injected later via apply_first_line_indent.
        let effective_wrap = wrap_width.saturating_sub(indent_gutter).max(1);
        if self.cached_visual_version != version
            || self.cached_visual_width != wrap_width
            || self.cached_visual_source_peek != self.source_peek
            || self.cached_visual_paragraph_indent != self.paragraph_indent
        {
            self.cached_visual = if self.source_peek {
                crate::visual::VisualDocument::from_source(
                    &self.editor.text(),
                    wrap_width,
                    ratatui::style::Style::default().fg(theme::text_primary()),
                )
            } else {
                crate::visual::VisualDocument::from_rendered(&self.rendered, effective_wrap)
            };
            if self.paragraph_indent && !self.source_peek && indent_gutter > 0 {
                self.cached_visual
                    .apply_first_line_indent(&self.editor.text());
            }
            self.cached_visual_version = version;
            self.cached_visual_width = wrap_width;
            self.cached_visual_source_peek = self.source_peek;
            self.cached_visual_paragraph_indent = self.paragraph_indent;
        }
    }

    pub fn word_count(&self) -> usize {
        self.editor.text().split_whitespace().count()
    }

    /// Build the sorted byte-start offsets of every whitespace-delimited
    /// word in `text`.  The list is ordered; binary search can find the
    /// word index for any byte position in O(log n).
    fn build_word_starts(text: &str) -> Vec<usize> {
        let mut starts = Vec::new();
        let mut in_word = false;
        for (byte_pos, ch) in text.char_indices() {
            if ch.is_whitespace() {
                in_word = false;
            } else if !in_word {
                starts.push(byte_pos);
                in_word = true;
            }
        }
        starts
    }

    /// Return `(cursor_word_index, total_words)` for the current cursor
    /// position.  Both counts are zero for empty / whitespace-only
    /// documents.  The backing sorted index is rebuilt once per buffer
    /// version and persisted across draws so cursor-only moves are cheap.
    pub fn cursor_word_progress(&mut self) -> (usize, usize) {
        let version = self.editor.buffer.version();
        if self.word_progress_version != version {
            // One full-text clone per buffer version is acceptable here.
            self.word_byte_starts = Self::build_word_starts(&self.editor.text());
            self.word_progress_version = version;
        }
        let total = self.word_byte_starts.len();
        if total == 0 {
            return (0, 0);
        }
        let rope = self.editor.buffer.rope();
        let char_pos = self.editor.cursor_char_pos().min(rope.len_chars());
        // O(log n) char → byte via the Rope, vs the old O(n) char_indices loop.
        let byte_pos = rope.char_to_byte(char_pos);
        // `partition_point` returns the index of the first word whose
        // start is NOT strictly before `byte_pos`, i.e. the count of
        // words that have begun before the cursor.
        let cursor = self
            .word_byte_starts
            .partition_point(|&start| start < byte_pos);
        (cursor.min(total), total)
    }

    /// Snapshot the current editor's document-length metrics for display in
    /// the bottom-eighth sidebar panel. Always reads from the raw editor
    /// text so the counts are stable across rendered/source-peek toggles.
    pub fn document_metrics(&self) -> DocumentMetrics {
        compute_metrics(&self.editor.text())
    }

    /// Deepest heading level present in the current outline (0 when there are
    /// no headings). Used to size the left marker gutter adaptively.
    pub fn max_heading_depth(&self) -> usize {
        self.outline_entries
            .iter()
            .filter(|e| matches!(e.kind, outline::OutlineKind::Heading))
            .map(|e| e.depth)
            .max()
            .unwrap_or(0)
    }

    /// Compute the gutter/margins layout for the current document area width.
    pub(crate) fn heading_gutter_layout(&self) -> HeadingGutterLayout {
        HeadingGutterLayout::for_area(self.document_area.width, self.max_heading_depth())
    }

    pub fn current_heading(&self) -> Option<String> {
        outline::breadcrumb(&self.outline_entries, self.editor.state.cursor_line)
    }

    pub fn ensure_cursor_visible(&mut self) {
        self.refresh_render_cache();
        let visual = self.visual_document();
        if let Some((row, _)) = visual.source_to_display(self.editor.cursor_char_pos()) {
            ensure_row_visible(
                &mut self.document_scroll,
                row,
                self.document_area.height as usize,
            );
        }
    }

    fn scroll_document(&mut self, delta: isize) {
        self.desired_display_col = None;
        self.document_scroll = if delta < 0 {
            self.document_scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.document_scroll.saturating_add(delta as usize)
        };
        self.clamp_document_scroll();
    }

    pub fn clamp_document_scroll(&mut self) {
        self.refresh_render_cache();
        let visual = self.visual_document();
        let max_scroll = visual
            .rows
            .len()
            .saturating_sub(self.document_area.height.max(1) as usize);
        self.document_scroll = self.document_scroll.min(max_scroll);
    }

    pub(crate) fn visual_document(&mut self) -> crate::visual::VisualDocument {
        self.refresh_render_cache();
        self.refresh_visual_cache();
        self.cached_visual.clone()
    }

    pub fn visual_rows_len(&self) -> usize {
        self.cached_visual.rows.len()
    }

    /// Build every visual row for the headings/section-browser panel.
    /// Long labels are wrapped at the panel width; continuation rows are
    /// indented to align under the label text. Each returned line carries
    /// the entry index, the text to render, and its style.
    pub(crate) fn build_heading_visual_lines(&self, max_width: u16) -> Vec<HeadingVisualLine> {
        let mut lines = Vec::new();
        let max_w = max_width.max(1) as usize;
        for (idx, entry) in self.outline_entries.iter().enumerate() {
            let indent = "  ".repeat(entry.depth.saturating_sub(1));
            // Bullet mirrors the current draw style: ▸ for depths 1-3, · for deeper.
            let bullet = if entry.depth <= 3 { "▸" } else { "·" };
            let active = entry.line <= self.editor.state.cursor_line;
            let level_color = match entry.depth {
                1 => theme::heading_h1(),
                2 => theme::heading_h2(),
                3 => theme::heading_h3(),
                4 => theme::heading_h4(),
                5 => theme::heading_h5(),
                _ => theme::heading_h6(),
            };
            let style = if active {
                ratatui::style::Style::default()
                    .fg(level_color)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                ratatui::style::Style::default().fg(theme::text_secondary())
            };

            let prefix = format!("{indent}{bullet} ");
            let prefix_dw = jones_text::grapheme_display_width(&prefix);
            let label = &entry.label;

            // How many display-width cells remain for the label after the prefix.
            let label_max = max_w.saturating_sub(prefix_dw);

            if label_max == 0 || prefix_dw >= max_w {
                // Not enough room for anything meaningful — show at least a
                // truncated version of the prefix so the user can see
                // *something* at ultra-narrow panel widths.
                let truncated = jones_text::truncate_to_display_width(&prefix, max_w);
                lines.push(HeadingVisualLine {
                    entry_idx: idx,
                    content: truncated.to_string(),
                    style,
                });
                continue;
            }

            // Grapheme-safe wrapping: never split combining/ZWJ clusters.
            // The helper prefers word boundaries but falls back to grapheme
            // boundaries for overlong words.
            let (first_chunk, rest_label) =
                jones_text::wrap_text_to_display_width(label, label_max);
            let first_line = format!("{prefix}{first_chunk}");
            lines.push(HeadingVisualLine {
                entry_idx: idx,
                content: first_line,
                style,
            });

            // Continuation lines: indent to align under the label text.
            let cont_indent = " ".repeat(prefix_dw);
            let mut rest: &str = rest_label;
            while !rest.is_empty() {
                let (chunk, remaining) = jones_text::wrap_text_to_display_width(rest, label_max);
                if chunk.is_empty() {
                    break;
                }
                lines.push(HeadingVisualLine {
                    entry_idx: idx,
                    content: format!("{cont_indent}{chunk}"),
                    style,
                });
                rest = remaining;
            }
        }
        lines
    }
}

fn mapped_char_near_visual_row(
    visual: &crate::visual::VisualDocument,
    target_row: usize,
    target_col: usize,
    delta: isize,
    clamped_to_boundary: bool,
) -> Option<usize> {
    if let Some(char_pos) = visual.display_to_source(target_row, target_col) {
        return Some(char_pos);
    }

    match delta.cmp(&0) {
        std::cmp::Ordering::Less => search_mapped_rows_backward(visual, target_row, target_col)
            .or_else(|| {
                clamped_to_boundary
                    .then(|| search_mapped_rows_forward(visual, target_row, target_col))
                    .flatten()
            }),
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Greater => search_mapped_rows_forward(visual, target_row, target_col)
            .or_else(|| {
                clamped_to_boundary
                    .then(|| search_mapped_rows_backward(visual, target_row, target_col))
                    .flatten()
            }),
    }
}

fn visual_word_boundary_right(
    visual: &crate::visual::VisualDocument,
    start_row: usize,
    start_col: usize,
) -> Option<usize> {
    let (mut row, mut col) = (start_row, start_col);
    while let Some((next_row, next_col)) = next_visual_cell(visual, row, col) {
        if !visual.is_word_at_display_col(row, col) {
            break;
        }
        row = next_row;
        col = next_col;
    }
    while let Some((next_row, next_col)) = next_visual_cell(visual, row, col) {
        if visual.is_word_at_display_col(row, col) {
            break;
        }
        row = next_row;
        col = next_col;
    }
    visual.display_to_source(row, col)
}

fn visual_word_boundary_left(
    visual: &crate::visual::VisualDocument,
    start_row: usize,
    start_col: usize,
) -> Option<usize> {
    let (mut row, mut col) = previous_visual_cell(visual, start_row, start_col)?;
    while !visual.is_word_at_display_col(row, col) {
        let Some((prev_row, prev_col)) = previous_visual_cell(visual, row, col) else {
            return visual.display_to_source(row, col);
        };
        row = prev_row;
        col = prev_col;
    }
    while let Some((prev_row, prev_col)) = previous_visual_cell(visual, row, col) {
        if visual.is_word_at_display_col(prev_row, prev_col) {
            row = prev_row;
            col = prev_col;
        } else {
            break;
        }
    }
    visual.display_to_source(row, col)
}

fn next_visual_cell(
    visual: &crate::visual::VisualDocument,
    row: usize,
    col: usize,
) -> Option<(usize, usize)> {
    let row_width = visual.row_width(row)?;
    if col < row_width {
        return Some((row, col + 1));
    }
    let mut next_row = row + 1;
    while next_row < visual.rows.len() {
        if visual.display_to_source(next_row, 0).is_some() {
            return Some((next_row, 0));
        }
        next_row += 1;
    }
    None
}

fn previous_visual_cell(
    visual: &crate::visual::VisualDocument,
    row: usize,
    col: usize,
) -> Option<(usize, usize)> {
    if col > 0 {
        return Some((row, col - 1));
    }
    let mut previous_row = row.checked_sub(1)?;
    loop {
        if let Some(width) = visual.row_width(previous_row)
            && visual.display_to_source(previous_row, width).is_some()
        {
            return Some((previous_row, width));
        }
        previous_row = previous_row.checked_sub(1)?;
    }
}

fn search_mapped_rows_forward(
    visual: &crate::visual::VisualDocument,
    start_row: usize,
    target_col: usize,
) -> Option<usize> {
    (start_row..visual.rows.len()).find_map(|row| visual.display_to_source(row, target_col))
}

fn search_mapped_rows_backward(
    visual: &crate::visual::VisualDocument,
    start_row: usize,
    target_col: usize,
) -> Option<usize> {
    (0..=start_row)
        .rev()
        .find_map(|row| visual.display_to_source(row, target_col))
}

fn next_visual_source_after(
    visual: &crate::visual::VisualDocument,
    start_row: usize,
    start_col: usize,
    current: usize,
) -> Option<usize> {
    let mut row = start_row;
    let mut col = start_col.saturating_add(1);
    while row < visual.rows.len() {
        let row_width = visual.row_width(row).unwrap_or_default();
        while col <= row_width {
            if let Some(source) = visual.display_to_source(row, col)
                && source > current
            {
                return Some(source);
            }
            col += 1;
        }
        row += 1;
        col = 0;
    }
    None
}

fn resolve_launch_target(maybe_path: Option<PathBuf>) -> (PathBuf, Option<PathBuf>) {
    match maybe_path {
        None => (
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            None,
        ),
        Some(path) if path.is_dir() => (path.canonicalize().unwrap_or(path), None),
        Some(path) => {
            let path = absolute_path(&path);
            let cwd = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            (cwd.canonicalize().unwrap_or(cwd), Some(path))
        }
    }
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn ensure_file_exists(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    if !path.exists() {
        std::fs::write(path, "")?;
    }
    Ok(())
}

fn pick_default_markdown_file(cwd: &Path) -> Option<PathBuf> {
    for name in ["index.md", "README.md", "readme.md"] {
        let candidate = cwd.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let mut markdown = std::fs::read_dir(cwd)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_markdown_path(path))
        .collect::<Vec<_>>();
    markdown.sort();
    markdown.into_iter().next()
}

fn sort_writerm_entries(entries: &mut [WorkspaceEntry], markdown_first: bool) {
    if !markdown_first {
        return;
    }
    entries.sort_by(|a, b| {
        entry_rank(a)
            .cmp(&entry_rank(b))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

fn entry_rank(entry: &WorkspaceEntry) -> u8 {
    match entry.kind {
        workspace::WorkspaceEntryKind::Parent => 0,
        workspace::WorkspaceEntryKind::Directory => 1,
        workspace::WorkspaceEntryKind::File if is_markdown_name(&entry.name) => 2,
        workspace::WorkspaceEntryKind::File if is_plain_text_name(&entry.name) => 3,
        workspace::WorkspaceEntryKind::File => 4,
    }
}

pub fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "md" | "markdown"))
}

fn is_markdown_name(name: &str) -> bool {
    is_markdown_path(Path::new(name))
}

fn is_plain_text_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "txt" | "text"))
}

fn markdown_filename(raw: &str) -> std::result::Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let path = Path::new(trimmed);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        return Err("Use a filename in the current folder".into());
    }
    if path.extension().is_some() {
        Ok(trimmed.to_string())
    } else {
        Ok(format!("{trimmed}.md"))
    }
}

fn point_in(area: Rect, col: u16, row: u16) -> bool {
    area.width > 0
        && area.height > 0
        && col >= area.x
        && col < area.x + area.width
        && row >= area.y
        && row < area.y + area.height
}

/// Returns `true` when `gap` is a valid soft-break separator: exactly
/// one newline (`\n` or `\r\n`) followed by zero or more ASCII spaces
/// and/or tabs.  Rejects empty gaps, multiple newlines (blank-line
/// paragraph breaks), bare `\r`, and any non-whitespace characters.
fn is_soft_break_separator(gap: &str) -> bool {
    if gap.is_empty() {
        return false;
    }
    let bytes = gap.as_bytes();
    let mut pos = 0;
    // Optional CR before LF.
    if bytes.get(pos) == Some(&b'\r') {
        pos += 1;
    }
    // Must have exactly one LF.
    if bytes.get(pos) != Some(&b'\n') {
        return false;
    }
    pos += 1;
    // Everything after the newline must be ASCII spaces or tabs.
    while pos < bytes.len() {
        match bytes[pos] {
            b' ' | b'\t' => pos += 1,
            _ => return false,
        }
    }
    true
}

fn ensure_row_visible(scroll: &mut usize, row: usize, viewport: usize) {
    let viewport = viewport.max(1);
    if row < *scroll {
        *scroll = row;
    } else if row >= *scroll + viewport {
        *scroll = row.saturating_sub(viewport.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::TempDir;

    fn app_at(path: PathBuf) -> WritermApp {
        let mut config = Config::default();
        config.layout.paragraph_indent = false;
        WritermApp::with_config(Some(path), config).unwrap()
    }

    #[test]
    fn directory_launch_prefers_index_then_readme_then_markdown() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("z.md"), "z").unwrap();
        std::fs::write(dir.path().join("README.md"), "readme").unwrap();
        std::fs::write(dir.path().join("index.md"), "index").unwrap();

        let app = app_at(dir.path().to_path_buf());

        assert_eq!(app.current_file_path.file_name().unwrap(), "index.md");
        assert_eq!(app.editor.text(), "index");
    }

    #[test]
    fn new_file_prompt_defaults_markdown_extension() {
        let dir = TempDir::new().unwrap();
        let mut app = app_at(dir.path().to_path_buf());

        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        for ch in "chapter-one".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.current_file_path.file_name().unwrap(), "chapter-one.md");
        assert!(app.current_file_path.exists());
    }

    #[test]
    fn plain_n_and_q_insert_text_instead_of_commands() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "").unwrap();
        let mut app = app_at(path);

        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));

        assert_eq!(app.editor.text(), "nq");
        assert!(app.running);
        assert!(app.prompt_mode.is_none());
    }

    #[test]
    fn new_file_prompt_rejects_paths_outside_current_folder() {
        let dir = TempDir::new().unwrap();
        let mut app = app_at(dir.path().to_path_buf());

        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        for ch in "../escape".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(
            app.notification
                .as_ref()
                .is_some_and(|(_, _, is_error)| *is_error)
        );
        assert!(!dir.path().join("../escape.md").exists());
    }

    #[test]
    fn invalid_utf8_launch_returns_error_instead_of_empty_buffer() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.md");
        std::fs::write(&path, [0xff, 0xfe]).unwrap();

        assert!(WritermApp::with_config(Some(path), Config::default()).is_err());
    }

    #[test]
    fn plain_text_launch_starts_in_source_peek() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "plain").unwrap();

        let app = app_at(path);

        assert!(app.source_peek);
    }

    #[test]
    fn sidebar_keys_toggle_each_sidebar_independently() {
        let dir = TempDir::new().unwrap();
        let mut app = app_at(dir.path().to_path_buf());

        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));

        assert!(app.show_headings);
        assert!(!app.show_files);

        app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));

        assert!(!app.show_headings);
        assert!(!app.show_files);
    }

    #[test]
    fn sidebar_control_clicks_toggle_each_sidebar_independently() {
        let dir = TempDir::new().unwrap();
        let mut app = app_at(dir.path().to_path_buf());
        app.headings_control_area = Rect::new(4, 9, 18, 1);
        app.files_control_area = Rect::new(24, 9, 14, 1);

        app.handle_event(AppEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 9,
            modifiers: KeyModifiers::NONE,
        }));

        assert!(!app.show_headings);
        assert!(app.show_files);

        app.handle_event(AppEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 25,
            row: 9,
            modifiers: KeyModifiers::NONE,
        }));

        assert!(!app.show_headings);
        assert!(!app.show_files);
    }

    #[test]
    fn file_switch_saves_dirty_content() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let mut app = app_at(a.clone());

        app.editor.buffer.insert_str(1, " changed");
        assert!(app.editor.is_dirty());
        assert!(app.open_or_create_file(&b));

        assert_eq!(std::fs::read_to_string(a).unwrap(), "a changed");
        assert_eq!(app.editor.text(), "b");
    }

    #[test]
    fn failed_save_keeps_dirty_state() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "note").unwrap();
        let mut app = app_at(path);
        app.editor.buffer.insert_str(4, "!");
        app.current_file_path = dir.path().join("missing").join("note.md");
        std::fs::write(dir.path().join("missing"), "not a dir").unwrap();

        assert!(!app.save_now());
        assert!(app.editor.is_dirty());
        assert!(
            app.notification
                .as_ref()
                .is_some_and(|(_, _, is_error)| *is_error)
        );
    }

    #[test]
    fn failed_dirty_save_blocks_open_and_preserves_document_state() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "alpha beta gamma").unwrap();
        std::fs::write(&b, "other").unwrap();
        let mut app = app_at(a.clone());
        app.document_area = Rect::new(0, 0, 10, 1);
        app.editor.move_cursor_to_char_pos(5);
        app.editor.state.start_selection();
        app.editor.move_cursor_to_char_pos(11);
        app.editor.state.extend_selection();
        app.document_scroll = 1;
        app.editor.buffer.insert_str(16, "!");
        app.current_file_path = dir.path().join("missing").join("a.md");
        std::fs::write(dir.path().join("missing"), "not a dir").unwrap();

        assert!(!app.open_or_create_file(&b));

        assert_eq!(
            app.current_file_path,
            dir.path().join("missing").join("a.md")
        );
        assert_eq!(app.editor.text(), "alpha beta gamma!");
        assert!(app.editor.is_dirty());
        assert_eq!(app.editor.cursor_char_pos(), 11);
        assert_eq!(
            app.editor
                .state
                .selected_char_range(app.editor.buffer.rope()),
            Some((5, 11))
        );
        assert_eq!(app.document_scroll, 1);
        assert_eq!(app.heading_scroll, 0);
        assert!(!app.source_peek);
        assert!(
            app.notification
                .as_ref()
                .is_some_and(|(_, _, is_error)| *is_error)
        );
    }

    #[test]
    fn autosave_failure_sets_redraw_and_backs_off_retry() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "note").unwrap();
        let mut app = app_at(path);
        app.editor.buffer.insert_str(4, "!");
        app.current_file_path = dir.path().join("missing").join("note.md");
        std::fs::write(dir.path().join("missing"), "not a dir").unwrap();
        app.last_edit = Some(Instant::now() - Duration::from_secs(10));
        app.needs_redraw = false;

        app.handle_tick();

        assert!(app.needs_redraw);
        assert!(
            app.last_edit
                .is_some_and(|edit| edit.elapsed() < Duration::from_secs(2))
        );
    }

    #[test]
    fn document_click_maps_rendered_cursor_to_source() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Hello").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 40, 10);

        app.click_document(0, 0, false);

        assert_eq!(app.editor.cursor_char_pos(), 2);
    }

    #[test]
    fn source_peek_click_past_line_end_clamps_to_that_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "abc\ndef").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 40, 10);
        app.source_peek = true;

        app.click_document(30, 0, false);

        assert_eq!(app.editor.cursor_char_pos(), 3);
    }

    #[test]
    fn cursor_after_space_stays_on_current_visual_row_in_rendered_mode() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "hello").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 3);
        app.editor.move_cursor_to_char_pos(5);

        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 6);
        assert_eq!(app.visual_document().source_to_display(6), Some((0, 6)));
        assert_eq!(app.document_scroll, 0);
    }

    #[test]
    fn cursor_after_newline_moves_to_real_next_visual_row() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "hello").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 3);
        app.editor.move_cursor_to_char_pos(5);

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 6);
        assert_eq!(app.visual_document().source_to_display(6), Some((1, 0)));
        assert_eq!(app.document_scroll, 0);
    }

    #[test]
    fn cursor_after_incomplete_markdown_marker_does_not_jump_to_bottom() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "\n\nnext").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 4);
        app.editor.move_cursor_to_char_pos(0);

        app.handle_key(KeyEvent::new(KeyCode::Char('#'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('#'), KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 2);
        assert_eq!(app.visual_document().source_to_display(2), Some((0, 0)));
        assert_eq!(app.document_scroll, 0);
    }

    #[test]
    fn down_arrow_moves_by_wrapped_visual_rows_inside_one_paragraph() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma delta").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 11, 4);
        app.editor.move_cursor_to_char_pos(2);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(app.editor.state.cursor_line, 0);
        assert_eq!(app.editor.cursor_char_pos(), 13);

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 2);
    }

    #[test]
    fn repeated_down_preserves_visual_column_across_short_wrapped_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "abcdefgh ij klmnopqr").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 8, 4);
        app.editor.move_cursor_to_char_pos(6);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 11);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 18);
    }

    #[test]
    fn typing_on_blank_line_after_down_does_not_render_on_previous_paragraph() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma\n\n# Heading").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 40, 8);
        app.editor.move_cursor_to_char_pos(0);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        let insert_pos = app.editor.cursor_char_pos();
        assert_eq!(insert_pos, 17);
        let visible_cursor = app.visual_document().source_to_display(insert_pos);
        assert_eq!(visible_cursor, Some((1, 0)));

        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        assert_eq!(
            app.visual_document().source_to_display(insert_pos),
            visible_cursor
        );
    }

    #[test]
    fn typing_after_down_inside_inline_code_stays_at_visible_cursor() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "**alpha beta gamma**\n`delta epsilon zeta`").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 11, 8);
        app.editor.move_cursor_to_char_pos(21);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        let insert_pos = app.editor.cursor_char_pos();
        assert_eq!(insert_pos, 27);
        let visible_cursor = app.visual_document().source_to_display(insert_pos);
        assert_eq!(visible_cursor, Some((2, 5)));

        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        assert_eq!(
            app.visual_document().source_to_display(insert_pos),
            visible_cursor
        );
    }

    #[test]
    fn typing_on_blank_line_after_down_from_wrapped_heading_stays_put() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Heading\n\nalpha beta gamma delta").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 7, 8);
        app.editor.move_cursor_to_char_pos(7);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        let insert_pos = app.editor.cursor_char_pos();
        assert_eq!(insert_pos, 10);
        let visible_cursor = app.visual_document().source_to_display(insert_pos);
        assert_eq!(visible_cursor, Some((2, 0)));
        assert_eq!(app.visual_document().row_width(2), Some(0));

        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        assert_eq!(
            app.visual_document().source_to_display(insert_pos),
            visible_cursor
        );
    }

    #[test]
    fn down_from_heading_reaches_intervening_text_before_later_blank_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Heading\npara\n\nnext").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 40, 8);
        app.editor.move_cursor_to_char_pos(2);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 10);
        assert_eq!(app.visual_document().source_to_display(10), Some((1, 0)));

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        let insert_pos = app.editor.cursor_char_pos();
        assert_eq!(insert_pos, 15);
        let visible_cursor = app.visual_document().source_to_display(insert_pos);
        assert_eq!(visible_cursor, Some((2, 0)));
        assert_eq!(app.visual_document().row_width(2), Some(0));

        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        assert_eq!(
            app.visual_document().source_to_display(insert_pos),
            visible_cursor
        );
    }

    #[test]
    fn modified_vertical_keys_do_not_fall_back_to_raw_source_movement() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "one two three four\nnext").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 8, 4);
        app.editor.move_cursor_to_char_pos(2);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL));

        assert_eq!(app.editor.cursor_char_pos(), 2);
        assert!(app.editor.state.selection.is_none());
    }

    #[test]
    fn shifted_visual_down_extends_selection_on_wrapped_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 11, 4);
        app.editor.move_cursor_to_char_pos(1);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));

        let rope = app.editor.buffer.rope();
        assert_eq!(app.editor.state.selected_char_range(rope), Some((1, 12)));
    }

    #[test]
    fn rendered_right_arrow_skips_hidden_heading_markers_without_looking_stuck() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Heading").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 4);
        app.editor.move_cursor_to_char_pos(0);

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 2);
        assert_eq!(app.visual_document().source_to_display(2), Some((0, 0)));

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 3);
        assert_eq!(app.visual_document().source_to_display(3), Some((0, 1)));
    }

    #[test]
    fn rendered_right_arrow_moves_from_horizontal_rule_to_next_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "---\nnext").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 40, 8);
        app.editor.move_cursor_to_char_pos(3);

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 4);
        assert_eq!(app.visual_document().source_to_display(4), Some((1, 0)));
    }

    #[test]
    fn rendered_ctrl_right_moves_by_visible_word_not_hidden_heading_marker() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Heading").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 4);

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));

        assert_eq!(app.editor.cursor_char_pos(), 9);
        assert!(app.editor.state.selection.is_none());
    }

    #[test]
    fn rendered_ctrl_right_skips_hidden_link_url() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        let text = "[link](https://x.test) next";
        std::fs::write(&path, text).unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 40, 4);
        let next_start = text.find("next").unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));

        assert_eq!(app.editor.cursor_char_pos(), next_start);
        assert_eq!(
            app.visual_document().source_to_display(next_start),
            Some((0, 5))
        );

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));

        assert_eq!(app.editor.cursor_char_pos(), text.chars().count());
    }

    #[test]
    fn rendered_ctrl_right_skips_hidden_bold_markers() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        let text = "**bold** next";
        std::fs::write(&path, text).unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 40, 4);
        let next_start = text.find("next").unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));

        assert_eq!(app.editor.cursor_char_pos(), next_start);
        assert_eq!(
            app.visual_document().source_to_display(next_start),
            Some((0, 5))
        );
    }

    #[test]
    fn rendered_ctrl_right_skips_hidden_inline_code_markers() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        let text = "`code` next";
        std::fs::write(&path, text).unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 40, 4);
        let next_start = text.find("next").unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));

        assert_eq!(app.editor.cursor_char_pos(), next_start);
        assert_eq!(
            app.visual_document().source_to_display(next_start),
            Some((0, 5))
        );
    }

    #[test]
    fn rendered_ctrl_shift_right_selects_visible_word_not_hidden_heading_marker() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Heading").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 4);

        app.handle_key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));

        assert_eq!(app.editor.cursor_char_pos(), 9);
        assert_eq!(
            app.editor
                .state
                .selected_char_range(app.editor.buffer.rope()),
            Some((0, 9))
        );
        assert_eq!(app.visual_document().source_to_display(9), Some((0, 7)));
    }

    #[test]
    fn rendered_ctrl_shift_left_selects_visible_word_from_heading_end() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Heading").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 4);
        app.editor.move_cursor_to_char_pos(9);

        app.handle_key(KeyEvent::new(
            KeyCode::Left,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));

        assert_eq!(app.editor.cursor_char_pos(), 2);
        assert_eq!(
            app.editor
                .state
                .selected_char_range(app.editor.buffer.rope()),
            Some((2, 9))
        );
    }

    #[test]
    fn source_peek_right_arrow_uses_raw_source_positions() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Heading").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 4);
        app.source_peek = true;
        app.editor.move_cursor_to_char_pos(0);

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 1);
    }

    #[test]
    fn source_peek_ctrl_shift_right_extends_word_selection() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "hello world").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 4);
        app.source_peek = true;

        app.handle_key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));

        assert_eq!(app.editor.cursor_char_pos(), 6);
        assert_eq!(
            app.editor
                .state
                .selected_char_range(app.editor.buffer.rope()),
            Some((0, 6))
        );
        assert_eq!(app.document_scroll, 0);
    }

    #[test]
    fn source_peek_ctrl_shift_left_extends_word_selection() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "hello world").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 4);
        app.source_peek = true;
        app.editor.move_cursor_to_char_pos(11);

        app.handle_key(KeyEvent::new(
            KeyCode::Left,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));

        assert_eq!(app.editor.cursor_char_pos(), 6);
        assert_eq!(
            app.editor
                .state
                .selected_char_range(app.editor.buffer.rope()),
            Some((6, 11))
        );
    }

    #[test]
    fn source_peek_shift_home_end_extend_source_line_selection() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "hello world").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 4);
        app.source_peek = true;
        app.editor.move_cursor_to_char_pos(6);

        app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::SHIFT));

        assert_eq!(
            app.editor
                .state
                .selected_char_range(app.editor.buffer.rope()),
            Some((0, 6))
        );

        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        app.editor.move_cursor_to_char_pos(6);

        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::SHIFT));

        assert_eq!(
            app.editor
                .state
                .selected_char_range(app.editor.buffer.rope()),
            Some((6, 11))
        );
    }

    #[test]
    fn rendered_home_end_move_to_wrapped_visual_row_boundaries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 10, 4);
        app.editor.move_cursor_to_char_pos(13);

        app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 11);

        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 16);
    }

    #[test]
    fn shifted_rendered_home_extends_selection_to_visual_row_start() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 10, 4);
        app.editor.move_cursor_to_char_pos(13);

        app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::SHIFT));

        assert_eq!(
            app.editor
                .state
                .selected_char_range(app.editor.buffer.rope()),
            Some((11, 13))
        );
    }

    #[test]
    fn wrapped_document_click_maps_visible_row_to_source_position() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 11, 4);

        app.click_document(1, 1, false);

        assert_eq!(app.editor.cursor_char_pos(), 12);
    }

    #[test]
    fn rendered_click_with_document_scroll_maps_offset_row() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 11, 2);
        app.document_scroll = 1;

        app.click_document(1, 0, false);

        assert_eq!(app.editor.cursor_char_pos(), 12);
    }

    #[test]
    fn cursor_visibility_uses_wrapped_visual_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 11, 1);
        app.editor.move_cursor_to_char_pos(13);

        app.ensure_cursor_visible();

        assert_eq!(app.document_scroll, 1);
    }

    #[test]
    fn cursor_on_table_delimiter_stays_visible_at_body_transition() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "| A |\n|---|\n| B |").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 1);
        app.editor.move_cursor_to_char_pos(6);

        app.ensure_cursor_visible();

        assert_eq!(app.visual_document().source_to_display(6), Some((1, 0)));
        assert_eq!(app.document_scroll, 1);

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 2);
        assert_eq!(app.document_scroll, 0);
    }

    #[test]
    fn mouse_scroll_down_clamps_to_available_visual_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 10, 1);

        for _ in 0..5 {
            app.handle_event(AppEvent::Mouse(crossterm::event::MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            }));
        }

        let max_scroll = app
            .visual_document()
            .rows
            .len()
            .saturating_sub(app.document_area.height as usize);
        assert_eq!(app.document_scroll, max_scroll);
    }

    #[test]
    fn page_down_moves_cursor_by_visual_rows_and_scrolls_to_it() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma delta epsilon zeta eta").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 11, 2);
        app.editor.move_cursor_to_char_pos(2);

        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));

        assert_eq!(app.visual_document().source_to_display(19), Some((2, 2)));
        assert_eq!(app.editor.cursor_char_pos(), 19);
        assert_eq!(app.document_scroll, 1);
    }

    #[test]
    fn page_up_down_clamp_to_visual_document_bounds() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 10, 20);
        app.editor.move_cursor_to_char_pos(2);

        app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 2);

        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 13);
        assert_eq!(app.document_scroll, 0);
    }

    #[test]
    fn page_down_skips_unmapped_rendered_spacer_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Heading\n\nbody").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 1);
        app.editor.move_cursor_to_char_pos(2);

        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 10);
        assert_eq!(app.visual_document().source_to_display(10), Some((1, 0)));
        assert_eq!(app.document_scroll, 1);
    }

    #[test]
    fn page_up_moves_cursor_by_visual_rows_from_scrolled_position() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma delta epsilon zeta eta").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 11, 2);
        app.editor.move_cursor_to_char_pos(31);
        app.ensure_cursor_visible();
        assert_eq!(app.visual_document().source_to_display(31), Some((4, 0)));
        assert_eq!(app.document_scroll, 3);

        app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 17);
        assert_eq!(app.document_scroll, 2);
    }

    #[test]
    fn shifted_page_down_extends_selection_by_visual_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma delta epsilon zeta eta").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 11, 2);
        app.editor.move_cursor_to_char_pos(2);

        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::SHIFT));

        assert_eq!(
            app.editor
                .state
                .selected_char_range(app.editor.buffer.rope()),
            Some((2, 19))
        );
    }

    #[test]
    fn shifted_page_up_extends_selection_by_visual_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma delta epsilon zeta eta").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 10, 2);
        app.editor.move_cursor_to_char_pos(31);

        app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT));

        assert_eq!(
            app.editor
                .state
                .selected_char_range(app.editor.buffer.rope()),
            Some((17, 31))
        );
    }

    #[test]
    fn ctrl_m_remaps_current_cursor_and_scroll_without_losing_position() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Heading\n\nalpha beta gamma delta").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 13, 1);
        app.editor.move_cursor_to_char_pos(12);
        app.document_scroll = 3;

        app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::CONTROL));

        assert!(app.source_peek);
        assert_eq!(app.editor.cursor_char_pos(), 12);
        assert_eq!(app.visual_document().source_to_display(12), Some((2, 1)));
        assert_eq!(app.document_scroll, 2);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 23);
    }

    #[test]
    fn switching_documents_resets_preserved_visual_column() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "abcdefgh ij klmnopqr").unwrap();
        std::fs::write(&b, "one two three four").unwrap();
        let mut app = app_at(a);
        app.document_area = Rect::new(0, 0, 8, 4);
        app.editor.move_cursor_to_char_pos(6);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.editor.cursor_char_pos(), 11);

        assert!(app.open_or_create_file(&b));
        app.document_area = Rect::new(0, 0, 8, 4);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(app.editor.cursor_char_pos(), 8);
    }

    #[test]
    fn opening_document_resets_scroll_cursor_selection_heading_scroll_and_render_mode() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "source text").unwrap();
        std::fs::write(&b, "# Heading\n\nbody").unwrap();
        let mut app = app_at(a);
        app.document_area = Rect::new(0, 0, 10, 1);
        app.document_scroll = 4;
        app.heading_scroll = 2;
        app.editor.move_cursor_to_char_pos(1);
        app.editor.state.start_selection();
        app.editor.move_cursor_to_char_pos(6);
        app.editor.state.extend_selection();
        assert!(app.source_peek);

        assert!(app.open_or_create_file(&b));

        assert_eq!(app.editor.cursor_char_pos(), 0);
        assert!(app.editor.state.selection.is_none());
        assert!(!app.editor.is_dirty());
        assert_eq!(app.document_scroll, 0);
        assert_eq!(app.heading_scroll, 0);
        assert!(!app.source_peek);
        assert_eq!(app.outline_entries.len(), 1);
        assert_eq!(app.outline_entries[0].label, "Heading");
    }

    #[test]
    fn heading_click_jumps_to_heading_source_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# One\nbody\n## Two\nmore").unwrap();
        let mut app = app_at(path);
        app.headings_area = Rect::new(0, 0, 20, 10);

        app.click_heading(1);

        assert_eq!(app.editor.state.cursor_line, 2);
    }

    #[test]
    fn file_click_opens_document() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let mut app = app_at(a);
        app.files_area = Rect::new(0, 0, 30, 10);
        let b_index = app
            .workspace_entries
            .iter()
            .position(|entry| entry.name == "b.md")
            .unwrap();

        app.click_file(b_index as u16);

        assert_eq!(app.current_file_path.file_name().unwrap(), "b.md");
        assert_eq!(app.editor.text(), "b");
    }

    // ── Heading gutter width and marker tests ────────────────────────

    #[test]
    fn max_heading_depth_returns_zero_when_no_heading_entries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "no headings here").unwrap();
        let app = app_at(path);

        assert_eq!(app.max_heading_depth(), 0);
    }

    #[test]
    fn max_heading_depth_adapts_to_deepest_heading() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# H1\n## H2\n### H3\n#### H4").unwrap();
        let app = app_at(path);

        assert_eq!(app.max_heading_depth(), 4);
    }

    #[test]
    fn max_heading_depth_ignores_section_fallback_entries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "Just text\nAnother line:").unwrap();
        let app = app_at(path);

        // Fallback outlines create Section entries but no Heading entries.
        assert_eq!(app.max_heading_depth(), 0);
    }

    #[test]
    fn gutter_layout_suppresses_gutter_when_no_headings() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "plain text").unwrap();
        let mut app = app_at(path);
        // Use a realistic area width.
        app.document_area = Rect::new(0, 0, 80, 20);

        let layout = app.heading_gutter_layout();
        assert_eq!(layout.gutter_cells, 0);
        assert_eq!(layout.blank_after, 0);
        assert_eq!(layout.text_x_offset, 0);
        // Text width = area_width - right_margin(1)
        assert_eq!(layout.text_width, 79);
    }

    #[test]
    fn gutter_layout_reserves_cells_for_heading_markers() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "### Deep heading").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 80, 20);

        let layout = app.heading_gutter_layout();
        // max depth = 3 → 3 gutter cells + 1 blank separator.
        assert_eq!(layout.gutter_cells, 3);
        assert_eq!(layout.blank_after, 1);
        assert_eq!(layout.text_x_offset, 4);
        assert_eq!(layout.text_width, 80 - 3 - 1 - 1); // 75
    }

    #[test]
    fn gutter_layout_suppresses_at_narrow_widths() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "## Two").unwrap();
        let mut app = app_at(path);
        // 2 + 3 = 5 → area_width 4 is below threshold.
        app.document_area = Rect::new(0, 0, 4, 20);

        let layout = app.heading_gutter_layout();
        assert_eq!(layout.gutter_cells, 0);
        assert_eq!(layout.text_width, 3); // 4 - 1 margin
    }

    // ── Right margin ─────────────────────────────────────────────────

    #[test]
    fn visual_cache_accounts_for_right_margin() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "hello world").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 10);

        let visual = app.visual_document();
        // With no headings, wrap width = 20 - 1 = 19.
        // "hello world" is 11 cells, well under 19, so 1 row.
        assert_eq!(visual.rows.len(), 1);
        // The row width should respect the margin-aware wrap width.
        assert!(visual.row_width(0).unwrap() <= 19);
    }

    // ── Wrapped heading visual lines ─────────────────────────────────

    #[test]
    fn heading_visual_lines_wrap_long_labels() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# A very long heading title that must wrap").unwrap();
        let mut app = app_at(path);
        app.headings_area = Rect::new(0, 0, 15, 10);

        let lines = app.build_heading_visual_lines(15);
        // With prefix "▸ " (2 cells display width), label max = 13.
        // "A very long heading title that must wrap" = 40 chars.
        // First line: "▸ A very long " (15 cells), then continuation.
        assert!(lines.len() > 1, "long label should wrap");
        // Continuation rows should be indented under the label (2 spaces).
        assert!(lines[1].content.starts_with("  "));
    }

    #[test]
    fn heading_visual_lines_single_entry_no_wrap() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Short").unwrap();
        let mut app = app_at(path);
        app.headings_area = Rect::new(0, 0, 30, 10);

        let lines = app.build_heading_visual_lines(30);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].content.contains("Short"));
    }

    #[test]
    fn heading_visual_lines_active_entry_has_bold_style() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Active\n\nin between\n\n## Inactive").unwrap();
        let mut app = app_at(path);
        app.headings_area = Rect::new(0, 0, 30, 10);
        // Cursor is at line 0, so the first heading is active.
        app.editor.state.cursor_line = 0;

        let lines = app.build_heading_visual_lines(30);
        // First entry (depth 1, active) should have BOLD modifier.
        assert!(
            lines[0]
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
        // Second entry (depth 2, inactive)
        let second_lines: Vec<_> = lines.iter().filter(|l| l.entry_idx == 1).collect();
        assert!(!second_lines.is_empty());
        assert!(
            !second_lines[0]
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
    }

    #[test]
    fn heading_visual_lines_never_split_combining_graphemes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        // "abc" + 2 combining marks per letter = 3 graphemes, display width 3.
        // Wrapping at 3 should emit the full heading without splitting graphemes.
        std::fs::write(
            &path,
            "# a\u{0301}b\u{0301}c\u{0301} d e f g h i j k l m n o p q r s t u v w x y z",
        )
        .unwrap();
        let mut app = app_at(path);
        app.headings_area = Rect::new(0, 0, 10, 20);

        let lines = app.build_heading_visual_lines(10);
        for line in &lines {
            // Every line must end at a grapheme boundary. We verify by
            // checking that a combining mark is never orphaned.
            let content = &line.content;
            // Walk through char by char: if we see a combining mark
            // (Unicode category Mn/Mc/Me), its predecessor char must have
            // been a base character.
            let mut prev_was_base = false;
            for ch in content.chars() {
                let is_zero_width = jones_text::grapheme_display_width(&ch.to_string()) == 0;
                if is_zero_width && prev_was_base {
                    // zero-width combining mark after a base char — OK
                } else if is_zero_width {
                    panic!("orphan zero-width char {ch:?} in line {content:?}");
                }
                prev_was_base = !is_zero_width;
            }
        }
    }

    #[test]
    fn heading_visual_lines_never_split_zwj_emoji() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        // Family emoji: width 2, single grapheme.  Place it near a wrap
        // boundary so we can assert it stays in one piece.
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let content = format!("# aa bb cc dd ee ff gg hh ii {family} jj kk ll mm");
        std::fs::write(&path, &content).unwrap();
        let mut app = app_at(path);
        // Narrow panel: ~10 cells wide. The ZWJ family is 2 cells wide and
        // must never be split across two visual lines.
        app.headings_area = Rect::new(0, 0, 10, 30);

        let lines = app.build_heading_visual_lines(10);
        // Find any line that contains a partial ZWJ sequence.
        for line in &lines {
            let text = &line.content;
            // If a ZWJ (U+200D) appears, the entire grapheme cluster must
            // be present (it's a multi-codepoint emoji sequence). We check
            // that any ZWJ is flanked by non-trivial chars, i.e. it's part
            // of a complete cluster.
            for (i, ch) in text.char_indices() {
                if ch == '\u{200D}' {
                    assert!(
                        i > 0 && i + ch.len_utf8() < text.len(),
                        "ZWJ at byte {i} in {text:?} looks truncated"
                    );
                }
            }
        }
    }

    // ── Heading scroll clamping ──────────────────────────────────────

    #[test]
    fn heading_scroll_is_clamped_by_real_draw_path() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# A\n# B\n# C\n# D").unwrap();
        // Terminal wide enough that the headings panel is visible
        // (MIN_DOCUMENT_WIDTH 40 + headings_block_w 30 + sep 1 = 71).
        let backend = ratatui::backend::TestBackend::new(80, 16);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut app = app_at(path);
        app.show_headings = true;
        app.heading_scroll = 50u16; // stale — well beyond everything

        // The draw path calls draw_headings_content which clamps
        // heading_scroll as a side-effect.
        terminal
            .draw(|frame| crate::draw::draw(frame, &mut app))
            .unwrap();

        // After the draw, heading_scroll must have been clamped.
        assert!(
            app.heading_scroll < 50,
            "draw path must clamp stale heading_scroll; still {}",
            app.heading_scroll,
        );
        // Build visual lines to compute the real maximum.
        let all_lines = app.build_heading_visual_lines(app.headings_area.width);
        let max_scroll = all_lines
            .len()
            .saturating_sub(app.headings_area.height.max(1) as usize);
        let max_scroll = u16::try_from(max_scroll).unwrap_or(u16::MAX);
        assert!(
            app.heading_scroll <= max_scroll,
            "heading_scroll {} must be <= max {max_scroll}",
            app.heading_scroll,
        );
    }

    #[test]
    fn heading_click_maps_to_correct_entry_with_wrapping() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        // A heading whose label is long enough to wrap inside a narrow
        // headings panel.
        std::fs::write(&path, "# First short\n## A Very Long Heading Title That Will Wrap To Multiple Visual Rows Inside The Section Browser Panel").unwrap();
        let mut app = app_at(path);
        app.headings_area = Rect::new(0, 0, 20, 10);

        let lines = app.build_heading_visual_lines(20);
        // The second heading should wrap to more than 1 visual row.
        let second_lines: Vec<_> = lines.iter().filter(|l| l.entry_idx == 1).collect();
        assert!(
            second_lines.len() > 1,
            "long heading must wrap to multiple visual rows, got {}",
            second_lines.len()
        );

        // Find the first continuation row of the second heading and click it.
        // All rows for entry_idx=1, after the first, are continuations.
        let cont_row = lines
            .iter()
            .position(|l| l.entry_idx == 1)
            .map(|first| first + 1)
            .unwrap();
        let abs_row = app.headings_area.y + cont_row as u16;
        app.click_heading(abs_row);
        // Cursor should land on the "## A Very Long..." heading line.
        assert_eq!(
            app.editor.state.cursor_line, 1,
            "continuation row click must map to the heading entry (line 1)"
        );

        // Now click the first entry after the wrapped heading to verify
        // row-to-entry mapping past the wrapped rows.
        if lines.len() > second_lines.len() + 1 {
            let after_wrapped = lines
                .iter()
                .rposition(|l| l.entry_idx == 1)
                .map(|last| last + 1)
                .unwrap();
            let abs_after = app.headings_area.y + after_wrapped as u16;
            app.click_heading(abs_after);
            // After the wrapped heading, the next entry might be a
            // section/symbol entry or empty. If it maps, it's correct.
            // The cursor should still be somewhere valid.
            assert!(
                app.editor.state.cursor_line < 1000,
                "click after wrapped rows should produce a valid cursor position"
            );
        }
    }

    // ── Display-width truncation ─────────────────────────────────────

    #[test]
    fn gutter_click_maps_to_correct_source() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Heading\n\nBody paragraph").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(5, 3, 40, 10);

        // Click inside the heading-marker gutter: doc_area.x is 5, row is
        // doc_area.y = 3. The gutter column maps through display_to_source
        // via the gutter offset correction in click_document.
        app.click_document(5, 3, false);
        // `# Heading` source positions: #(0), ' '(1), H(2), e(3)...
        // At minimum the cursor should land somewhere in the heading text
        // (position 2 or later), not at position 0.
        assert!(
            app.editor.cursor_char_pos() >= 2,
            "gutter click should map into the heading text, got {}",
            app.editor.cursor_char_pos(),
        );
    }

    #[test]
    fn effective_text_width_reduces_wrap_for_gutter_and_margin() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        // Heading depth 1 → gutter 1 + blank 1 + margin 1 = 3 cells less.
        std::fs::write(&path, "# H\n\nalpha beta gamma delta").unwrap();
        let mut app = app_at(path);
        app.document_area = Rect::new(0, 0, 20, 10);

        let visual = app.visual_document();
        // The heading marker row wraps at 20 - 3 = 17 cells (wide enough).
        // The body text should also wrap at 17.
        let layout = app.heading_gutter_layout();
        assert_eq!(layout.text_width, 17);
        assert!(visual.row_width(0).unwrap() <= layout.text_width as usize);
    }

    // ── cursor_word_progress tests ────────────────────────────────────

    #[test]
    fn word_progress_empty_doc_returns_zero() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "").unwrap();
        let mut app = app_at(path);

        let (cursor, total) = app.cursor_word_progress();
        assert_eq!((cursor, total), (0, 0));
    }

    #[test]
    fn word_progress_whitespace_only_returns_zero() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "   \n  \n  ").unwrap();
        let mut app = app_at(path);

        let (cursor, total) = app.cursor_word_progress();
        assert_eq!((cursor, total), (0, 0));
    }

    #[test]
    fn word_progress_at_start() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "one two three").unwrap();
        let mut app = app_at(path);
        app.editor.move_cursor_to_char_pos(0);

        let (cursor, total) = app.cursor_word_progress();
        assert_eq!(cursor, 0);
        assert_eq!(total, 3);
    }

    #[test]
    fn word_progress_at_end() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "one two three").unwrap();
        let mut app = app_at(path);
        let len = app.editor.text().chars().count();
        app.editor.move_cursor_to_char_pos(len);

        let (cursor, total) = app.cursor_word_progress();
        assert_eq!(cursor, 3);
        assert_eq!(total, 3);
    }

    #[test]
    fn word_progress_mid_word() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma").unwrap();
        let mut app = app_at(path);
        // "alpha beta gamma"
        //  a(0) l(1) p(2) h(3) a(4) ' '(5) b(6) e(7) t(8) a(9)
        // Cursor at pos 7 = 'e'. Text before cursor byte: "alpha b"
        // split_whitespace → ["alpha", "b"] = 2 words.
        app.editor.move_cursor_to_char_pos(7);

        let (cursor, total) = app.cursor_word_progress();
        assert_eq!(cursor, 2, "2 words before cursor inside 'beta'");
        assert_eq!(total, 3);
    }

    #[test]
    fn word_progress_with_unicode_characters() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        // "café résumé naïveté" — 3 words
        std::fs::write(&path, "café résumé naïveté").unwrap();
        let mut app = app_at(path);
        // Cursor at the start of "résumé": "café " = 5 chars.
        app.editor.move_cursor_to_char_pos(5);

        let (cursor, total) = app.cursor_word_progress();
        assert_eq!(cursor, 1);
        assert_eq!(total, 3);

        // Cursor inside "café" at position 3 (the 'é').
        // Byte slice before the cursor: "caf". This is a fragment of one
        // word (no whitespace), so split_whitespace counts it as 1 token.
        app.editor.move_cursor_to_char_pos(3);
        let (cursor2, total2) = app.cursor_word_progress();
        assert_eq!(cursor2, 1, "cursor inside first word gives 1 word");
        assert_eq!(total2, 3);
    }

    #[test]
    fn word_progress_updates_after_typing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "hello").unwrap();
        let mut app = app_at(path);
        app.editor.move_cursor_to_char_pos(5);

        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

        let (cursor, total) = app.cursor_word_progress();
        assert_eq!(total, 2, "should now have 2 words");
        assert_eq!(cursor, 2, "cursor should be past both words");
    }

    #[test]
    fn word_progress_cache_reuses_across_cursor_moves() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "one two three four five six").unwrap();
        let mut app = app_at(path);

        // Prime the cache.
        let v1 = app.editor.buffer.version();
        let (_c1, t1) = app.cursor_word_progress();
        assert_eq!(t1, 6);

        // Move cursor twice; results must be consistent with the cache
        // (the index is NOT rebuilt because the buffer version hasn't
        // changed).
        app.editor.move_cursor_to_char_pos(0);
        let (c2, t2) = app.cursor_word_progress();
        assert_eq!(c2, 0, "cursor at start");
        assert_eq!(t2, 6);

        // Cursor at the 't' of "three": pos 8 → 2 words before.
        app.editor.move_cursor_to_char_pos(8);
        let (c3, t3) = app.cursor_word_progress();
        assert_eq!(c3, 2, "2 words before 'three'");
        assert_eq!(t3, 6);
        assert_eq!(v1, app.editor.buffer.version(), "version unchanged");
    }

    #[test]
    fn word_progress_after_edit_rebuilds_cache() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "hello world").unwrap();
        let mut app = app_at(path);

        let v1 = app.editor.buffer.version();
        let (_c1, t1) = app.cursor_word_progress();
        assert_eq!(t1, 2);

        // Move to end, then type a new word.
        let end = app.editor.text().chars().count();
        app.editor.move_cursor_to_char_pos(end);
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));

        assert_ne!(v1, app.editor.buffer.version(), "version should change");
        let (_c2, t2) = app.cursor_word_progress();
        assert_eq!(t2, 3, "should now have 3 words");
    }

    #[test]
    fn word_progress_rebuilds_after_opening_another_file() {
        let dir = TempDir::new().unwrap();
        let first = dir.path().join("first.md");
        let second = dir.path().join("second.md");
        std::fs::write(&first, vec!["word"; 816].join(" ")).unwrap();
        std::fs::write(&second, vec!["word"; 900].join(" ")).unwrap();
        let mut app = app_at(first);

        let (_, first_total) = app.cursor_word_progress();
        assert_eq!(first_total, 816);
        assert_eq!(app.editor.buffer.version(), 0);

        assert!(app.open_or_create_file(&second));
        assert_eq!(app.editor.buffer.version(), 0);
        let end = app.editor.buffer.rope().len_chars();
        app.editor.move_cursor_to_char_pos(end);

        assert_eq!(app.cursor_word_progress(), (900, 900));
    }

    // ── Narrow-width indent regression test ─────────────────────────

    #[test]
    fn indent_at_width_one_does_not_panic_or_overflow() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "Abc").unwrap();
        let mut app = app_at(path);
        app.source_peek = false;
        app.paragraph_indent = true;
        // Force document area so text_width = 1 (heading depth 0 → no
        // gutter, margin = 1, wrap = 1).
        app.document_area = Rect::new(0, 0, 2, 10);
        let visual = app.visual_document();
        // Indented row width must NOT exceed the wrap width + indent
        // (which saturates).  Check that every row has width ≤ 2.
        for idx in 0..visual.rows.len() {
            let w = visual.row_width(idx).unwrap_or(0);
            assert!(
                w <= 2,
                "row {idx} width {w} must not overflow at wrap_width=1"
            );
        }
    }

    #[test]
    fn indent_at_width_two_fits_exactly() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "Hi").unwrap();
        let mut app = app_at(path);
        app.source_peek = false;
        app.paragraph_indent = true;
        // text area = 3 cells gives wrap_width = 3, minus indent = 1.
        app.document_area = Rect::new(0, 0, 4, 10);
        let visual = app.visual_document();
        for idx in 0..visual.rows.len() {
            let w = visual.row_width(idx).unwrap_or(0);
            assert!(
                w <= 3,
                "row {idx} width {w} must not overflow at effective_wrap=1"
            );
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Paragraph-indent Backspace regression
    // ═══════════════════════════════════════════════════════════════════

    fn app_with_indent(path: PathBuf) -> WritermApp {
        let mut config = Config::default();
        config.layout.paragraph_indent = true;
        WritermApp::with_config(Some(path), config).unwrap()
    }

    /// Backspace at the visible text-start of a tab-indented prose
    /// continuation line atomically deletes the hidden newline+tab
    /// separator, merging the line with the previous one in a single
    /// user-visible action.  The cursor remains at the merged position
    /// and the document text changes immediately.
    #[test]
    fn backspace_at_tab_indented_text_start_merges_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        // Model: normal line followed by tab-prefixed continuation in the
        // same Markdown paragraph (soft break — barrens-e2.md pattern).
        std::fs::write(&path, "First line.\n\tSecond line.").unwrap();
        let mut app = app_with_indent(path);
        app.document_area = Rect::new(0, 0, 60, 10);

        // Verify initial visual state: two rows, both indented.
        let visual = app.visual_document();
        assert_eq!(visual.rows.len(), 2);
        assert!(visual.rows[0].to_line().to_string().starts_with("  First"));
        assert!(visual.rows[1].to_line().to_string().starts_with("  Second"));
        assert_eq!(visual.row_prefix_width(0), Some(2));
        assert_eq!(visual.row_prefix_width(1), Some(2));

        // Move cursor to the text-start of the second row.
        // "Second" starts at source position 13: 0-10="First line.",
        // 11='\n', 12='\t', 13='S'.
        // source_to_display(13) should return (1, 2) — text-start.
        let display = visual.source_to_display(13).unwrap();
        assert_eq!(
            display,
            (1, 2),
            "cursor should land at text-start after indent"
        );
        app.editor.move_cursor_to_char_pos(13);

        // One Backspace.
        let text_before = app.editor.text();
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        let text_after = app.editor.text();

        // Source text must have changed: the newline and tab are gone,
        // lines are merged.
        assert_ne!(text_before, text_after, "Backspace must change source text");
        assert!(
            !text_after.contains('\t'),
            "tab should be deleted, got: {text_after:?}"
        );
        assert_eq!(
            text_after.lines().count(),
            1,
            "lines should be merged into one: {text_after:?}"
        );
        assert!(
            text_after.starts_with("First line.Second line."),
            "merged text should be 'First line.Second line.', got: {text_after:?}"
        );

        // Visual state must have changed — only one visual row now.
        let visual_after = app.visual_document();
        let non_blank: Vec<_> = visual_after
            .rows
            .iter()
            .filter(|r| !r.to_line().to_string().trim().is_empty())
            .collect();
        assert_eq!(non_blank.len(), 1, "should have one merged visual row");

        // Cursor must be at a valid position, not trapped.
        let cursor = app.editor.cursor_char_pos();
        let display_after = visual_after.source_to_display(cursor);
        assert!(
            display_after.is_some(),
            "cursor must have a valid display position"
        );
        // The cursor should be on the merged line, at or after the "Second" text start.
        let (row, col) = display_after.unwrap();
        assert_eq!(row, 0, "cursor should be on the single merged visual row");
        assert!(col >= 2, "cursor should be past the indent prefix");
    }

    /// Same as above but with space-prefixed continuation (4 spaces
    /// instead of a tab).  Backspace atomically consumes the hidden
    /// whitespace boundary so the user sees one clear action.
    #[test]
    fn backspace_at_space_indented_text_start_merges_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "First para.\n    Second line.").unwrap();
        let mut app = app_with_indent(path);
        app.document_area = Rect::new(0, 0, 60, 10);

        // Source positions: "First para." = 11 chars (0-10), '\n' = 11,
        // "    " = 12-15, 'S' = 16.
        // After rendering, leading spaces trimmed, indent added.
        let visual = app.visual_document();
        assert!(visual.rows[1].to_line().to_string().starts_with("  Second"));

        // Text-start of "Second" is at source position 16.
        let display = visual.source_to_display(16).unwrap();
        assert_eq!(display.0, 1, "should be on second visual row");
        app.editor.move_cursor_to_char_pos(16);

        let text_before = app.editor.text();
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        let text_after = app.editor.text();

        // Source text must have changed — no silent no-op.
        assert_ne!(text_before, text_after, "Backspace must change source text");

        // Visual state: lines merged into one visual row.
        let visual_after = app.visual_document();
        let non_blank: Vec<_> = visual_after
            .rows
            .iter()
            .filter(|r| !r.to_line().to_string().trim().is_empty())
            .collect();
        assert_eq!(non_blank.len(), 1, "should merge visual rows");

        // Cursor must have a valid display position, not trapped.
        let cursor = app.editor.cursor_char_pos();
        let display_after = visual_after.source_to_display(cursor);
        assert!(
            display_after.is_some(),
            "cursor must have valid display position"
        );
    }

    /// Backspace at the text-start of the FIRST paragraph (no previous
    /// visual row) falls through to normal editor behavior — the cursor
    /// is at position 0, Backspace does nothing.
    #[test]
    fn backspace_at_first_paragraph_text_start_is_noop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "Hello world").unwrap();
        let mut app = app_with_indent(path);
        app.document_area = Rect::new(0, 0, 60, 10);

        // Cursor at text-start (source pos 0, col 2 after indent).
        app.editor.move_cursor_to_char_pos(0);
        let text_before = app.editor.text();

        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

        // Text should be unchanged (no previous row to merge with).
        assert_eq!(app.editor.text(), text_before);
    }

    /// Normal Backspace behavior is preserved when NOT at text-start:
    /// Backspace deletes the character before the cursor as usual.
    #[test]
    fn backspace_mid_text_uses_normal_editor_behavior() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "Hello world").unwrap();
        let mut app = app_with_indent(path);
        app.document_area = Rect::new(0, 0, 60, 10);

        // Cursor at source pos 3 ("l" in "Hello").
        app.editor.move_cursor_to_char_pos(3);

        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

        // Normal backspace: delete the character before the cursor.
        assert_eq!(app.editor.text(), "Helo world");
    }

    /// Backspace at the text-start of an indented prose row does NOT
    /// merge across a structural boundary (heading).  The hidden-
    /// separator handler rejects the previous row because headings have
    /// `prefix_width == 0`, so Backspace falls through to normal editor
    /// behavior (deleting the `\n` before the heading).
    #[test]
    fn backspace_does_not_merge_across_heading() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        // Prose paragraph followed by a heading, then blank-line, then prose.
        std::fs::write(&path, "prose line.\n# Heading\n\nAfter heading.").unwrap();
        let mut app = app_with_indent(path);
        app.document_area = Rect::new(0, 0, 60, 10);

        // Find the visual row for "After heading." — it should be indented.
        let visual = app.visual_document();
        let after_row = visual
            .rows
            .iter()
            .position(|r| r.to_line().to_string().contains("After heading"))
            .expect("should have After heading row");
        assert_eq!(
            visual.row_prefix_width(after_row),
            Some(2),
            "prose after heading should be indented"
        );

        // Move cursor to text-start of "After heading."
        // Source: "prose line.\n# Heading\n\nAfter heading."
        // Positions: 0-10="prose line.", 11='\n', 12-20="# Heading", 21='\n',
        //   22='\n', 23-36="After heading."
        // Text-start of "After heading": source pos 23.
        let display = visual.source_to_display(23).unwrap();
        assert_eq!(display.0, after_row);
        app.editor.move_cursor_to_char_pos(23);

        let text_before = app.editor.text();
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        let text_after = app.editor.text();

        // The # Heading row is structural (no prefix), so the atomic
        // deletion must NOT fire.  Normal Backspace deletes one char
        // (the '\n' at pos 22), merging the blank line with the prose.
        // The heading stays on its own line.
        assert!(
            text_after.contains("# Heading"),
            "heading must survive Backspace: {text_after:?}"
        );
        // The blank line between heading and prose should be gone
        // (one \n was deleted), but the heading line-break should remain.
        assert_ne!(text_before, text_after, "text should have changed");
        let lines: Vec<&str> = text_after.lines().collect();
        // After deleting one \n: "prose line.", "# Heading", "After heading."
        // Wait — the blank line becomes empty, so we have:
        // "prose line.\n# Heading\n\nAfter heading." → delete \n at 22 →
        // "prose line.\n# Heading\nAfter heading." (3 lines)
        assert_eq!(lines.len(), 3, "should have 3 lines, got: {lines:?}");
    }

    /// Backspace at text-start of a blank-line-separated paragraph does
    /// NOT atomically delete the blank-line separator.  The blank row
    /// has `prefix_width == 0`, so the handler falls through to normal
    /// Backspace (one character deletion).
    #[test]
    fn backspace_does_not_cross_blank_line_separator() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "First para.\n\nSecond para.").unwrap();
        let mut app = app_with_indent(path);
        app.document_area = Rect::new(0, 0, 60, 10);

        let visual = app.visual_document();
        // "Second para." starts at source position 13 (after "First para.\n\n").
        // source_to_display(13) should return text-start (col 2 after indent).
        let _display = visual.source_to_display(13).unwrap();
        assert_eq!(_display.1, 2, "cursor should be at text-start");
        app.editor.move_cursor_to_char_pos(13);

        let text_before = app.editor.text();
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        let text_after = app.editor.text();

        // One Backspace deletes one character (the '\n' at pos 12).
        // The paragraphs should NOT merge — there's still a \n remaining.
        assert_ne!(text_before, text_after);
        assert!(
            text_after.contains("First para."),
            "first para must survive"
        );
        assert!(
            text_after.contains("Second para."),
            "second para must survive"
        );
        // After deleting one \n: "First para.\nSecond para." (2 lines)
        let lines: Vec<&str> = text_after.lines().collect();
        assert_eq!(lines.len(), 2, "should have 2 lines, got: {lines:?}");
    }

    /// When a selection exists at the text-start of an indented row,
    /// the handler falls through to normal editor Backspace (which
    /// deletes the selection).
    #[test]
    fn backspace_with_selection_falls_through_to_editor() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "First line.\n\tSecond line.").unwrap();
        let mut app = app_with_indent(path);
        app.document_area = Rect::new(0, 0, 60, 10);

        // Source: 0-10="First line.", 11='\n', 12='\t', 13-24="Second line."
        // Create a selection covering "Second" (positions 13-18 inclusive, 6 chars).
        app.editor.move_cursor_to_char_pos(13);
        app.editor.state.start_selection();
        app.editor.move_cursor_to_char_pos(19);
        app.editor.state.extend_selection();
        // Selection covers [13, 19).

        let text_before = app.editor.text();
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        let text_after = app.editor.text();

        // Selection was deleted. "Second" (6 chars) removed.
        assert_ne!(
            text_before, text_after,
            "text should change when selection deleted"
        );
        assert!(
            !text_after.contains("Second"),
            "selection should be deleted: {text_after:?}"
        );
        // The surrounding structure should survive.
        assert!(
            text_after.contains("First line."),
            "first line must survive"
        );
        assert!(
            text_after.contains('\t'),
            "tab should survive: {text_after:?}"
        );
        assert!(
            text_after.contains(" line."),
            "rest of line should survive: {text_after:?}"
        );
    }

    /// Regression test: non-ASCII characters before a `\n\t` continuation
    /// cause char-offset vs byte-offset mismatch when slicing the gap
    /// string.  The fix uses `nth_char_byte_offset` to convert to byte
    /// offsets before slicing.  Without the fix, this test panics.
    ///
    /// "Hëllõ." = 6 chars but 8 bytes (ë=2 bytes, õ=2 bytes).  Char 6
    /// is '\n' at byte 8, char 7 is '\t' at byte 9, char 8 is 'S' at
    /// byte 10.  The gap between prev_end (char 6) and cursor (char 8)
    /// in char space is "\n\t", but a naive `text[6..8]` byte-slice
    /// lands inside 'õ' (bytes 5-6) and panics.
    #[test]
    fn backspace_with_non_ascii_before_continuation_does_not_panic() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "Hëllõ.\n\tSecond").unwrap();
        let mut app = app_with_indent(path);
        app.document_area = Rect::new(0, 0, 60, 10);

        let visual = app.visual_document();
        // "Hëllõ." (chars 0-5), '\n' (6), '\t' (7), "Second" (8-13).
        // S of "Second" is at char offset 8.
        let display = visual.source_to_display(8).unwrap();
        assert_eq!(
            display.1, 2,
            "cursor should be at col 2 (text-start) on second row"
        );
        app.editor.move_cursor_to_char_pos(8);

        let text_before = app.editor.text();
        // This call must NOT panic (regression).
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        let text_after = app.editor.text();

        // Lines merged (the \n\t separator was consumed atomically).
        assert_ne!(text_before, text_after, "Backspace must change source text");
        assert_eq!(
            text_after.lines().count(),
            1,
            "lines should be merged into one: {text_after:?}"
        );
        // Non-ASCII content at the start must be preserved.
        assert!(
            text_after.starts_with("Hëllõ."),
            "non-ASCII text must survive: {text_after:?}"
        );
        assert!(
            text_after.contains("Second"),
            "second-line text must be merged: {text_after:?}"
        );
    }

    /// After the atomic backspace deletion, document metadata (word
    /// count etc.) is refreshed, and the buffer version changes.
    #[test]
    fn backspace_at_indented_start_refreshes_metadata() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "First line.\n\tSecond line.").unwrap();
        let mut app = app_with_indent(path);
        app.document_area = Rect::new(0, 0, 60, 10);

        let version_before = app.editor.buffer.version();
        let wc_before = app.word_count();

        // Cursor at text-start of second row.
        let visual = app.visual_document();
        let _display = visual.source_to_display(13).unwrap();
        app.editor.move_cursor_to_char_pos(13);

        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

        // Buffer version must have incremented.
        assert_ne!(
            app.editor.buffer.version(),
            version_before,
            "buffer version must change after edit"
        );
        // Word count must be updated (cache was rebuilt).
        let wc_after = app.word_count();
        // "First line." + "Second line." = "First line.Second line."
        // = "First" "line" "Second" "line" = 4 words.  Original had same.
        // Actually, word count depends on whitespace splitting.
        // Before: "First line.\n\tSecond line." — words: "First", "line.", "Second", "line."
        // After:  "First line.Second line." — words: "First", "line.Second", "line."
        // So it should change.
        assert_eq!(wc_before, 4);
        assert_eq!(wc_after, 3); // "line.Second" is one word now
    }
}
