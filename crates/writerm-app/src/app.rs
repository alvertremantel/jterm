use color_eyre::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use jones_editor::{EditorAction, EditorContext};
use jones_event::{AppEvent, EventHandler};
use jones_outline::{self as outline, OutlineEntry};
use jones_render::{RenderedDocument, render_markdown_mapped};
use jones_workspace::{self as workspace, WorkspaceEntry, WorkspaceOptions, WorkspaceSortMode};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use writerm_config::Config;

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
    pub document_scroll: usize,
    pub heading_scroll: u16,
    pub prompt_mode: Option<PromptMode>,
    pub prompt_buffer: String,
    pub notification: Option<(String, Instant, bool)>,
    pub running: bool,
    pub headings_area: Rect,
    pub document_area: Rect,
    pub files_area: Rect,
    pub drag_selecting: bool,
    last_edit: Option<Instant>,
    needs_redraw: bool,
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
            document_scroll: 0,
            heading_scroll: 0,
            prompt_mode: None,
            prompt_buffer: String::new(),
            notification: None,
            running: true,
            headings_area: Rect::default(),
            document_area: Rect::default(),
            files_area: Rect::default(),
            drag_selecting: false,
            last_edit: None,
            needs_redraw: true,
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
        match key.code {
            KeyCode::Char('q') if ctrl => {
                self.quit();
            }
            KeyCode::Char('s') if ctrl => {
                self.save_now();
            }
            KeyCode::Char('m') if ctrl => {
                self.source_peek = !self.source_peek;
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
            KeyCode::Char('n') if ctrl => {
                self.prompt_mode = Some(PromptMode::NewFile);
                self.prompt_buffer.clear();
            }
            KeyCode::PageUp => {
                self.document_scroll = self
                    .document_scroll
                    .saturating_sub(self.document_area.height.max(1) as usize);
            }
            KeyCode::PageDown => {
                self.document_scroll += self.document_area.height.max(1) as usize;
            }
            _ => self.handle_editor_key(key),
        }
    }

    fn handle_editor_key(&mut self, key: KeyEvent) {
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
                if point_in(self.headings_area, mouse.column, mouse.row) {
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
                self.document_scroll = self.document_scroll.saturating_sub(3);
            }
            MouseEventKind::ScrollDown => {
                self.document_scroll = self.document_scroll.saturating_add(3);
            }
            _ => {}
        }
    }

    fn click_heading(&mut self, row: u16) {
        let rel = row.saturating_sub(self.headings_area.y) as usize;
        let idx = self.heading_scroll as usize + rel;
        if let Some(entry) = self.outline_entries.get(idx) {
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
        let rel_col = col.saturating_sub(self.document_area.x) as usize;
        let display_row = self.document_scroll + rel_row;
        let char_pos = if self.source_peek {
            let line = self.document_scroll + rel_row;
            let line = line.min(self.editor.buffer.line_count().saturating_sub(1));
            let rope = self.editor.buffer.rope();
            let line_start = rope.line_to_char(line);
            let line_slice = rope.line(line);
            let mut line_len = line_slice.len_chars();
            if line_len > 0 && line_slice.char(line_len - 1) == '\n' {
                line_len -= 1;
                if line_len > 0 && line_slice.char(line_len - 1) == '\r' {
                    line_len -= 1;
                }
            }
            line_start + rel_col.min(line_len)
        } else {
            self.rendered
                .display_to_source(display_row, rel_col)
                .unwrap_or_else(|| self.editor.cursor_char_pos())
        };

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
        self.document_scroll = 0;
        self.source_peek = !is_markdown_path(&self.current_file_path);
        self.refresh_workspace();
        self.refresh_document_metadata();
        self.refresh_render_cache_force();
        self.notification = Some(("Opened".into(), Instant::now(), false));
        true
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

    pub fn word_count(&self) -> usize {
        self.editor.text().split_whitespace().count()
    }

    pub fn current_heading(&self) -> Option<String> {
        outline::breadcrumb(&self.outline_entries, self.editor.state.cursor_line)
    }

    pub fn ensure_cursor_visible(&mut self) {
        if self.source_peek {
            let row = self.editor.state.cursor_line;
            ensure_row_visible(
                &mut self.document_scroll,
                row,
                self.document_area.height as usize,
            );
            return;
        }
        self.refresh_render_cache();
        if let Some((row, _)) = self
            .rendered
            .source_to_display(self.editor.cursor_char_pos())
        {
            ensure_row_visible(
                &mut self.document_scroll,
                row,
                self.document_area.height as usize,
            );
        }
    }
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
        WritermApp::with_config(Some(path), Config::default()).unwrap()
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
}
