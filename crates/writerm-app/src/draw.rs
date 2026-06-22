use crate::app::{WritermApp, is_markdown_path};
use jones_theme as theme;
use jones_workspace::WorkspaceEntryKind;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::metrics::format_reading_time;

const MIN_DOCUMENT_WIDTH: u16 = 40;
const SIDEBAR_GAP: u16 = 2;
/// How the right-hand sidebar is split between the filesystem browser and
/// the document-length metrics panel. The user-visible model is "split the
/// sidebar in half, then shrink the bottom half down to roughly 1/8", which
/// leaves 7/8 for the filesystem browser and 1/8 for the metrics readout.
const FILES_PARTS: u32 = 7;
const METRICS_PARTS: u32 = 1;
const SIDEBAR_PARTS: u16 = (FILES_PARTS + METRICS_PARTS) as u16;

pub fn draw(frame: &mut Frame, app: &mut WritermApp) {
    frame.render_widget(
        ratatui::widgets::Block::default().style(theme::base_style()),
        frame.area(),
    );

    let outer = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(frame.area());
    draw_top_ribbon(frame, app, outer[0]);
    draw_body(frame, app, outer[1]);
    draw_bottom_bar(frame, app, outer[2]);

    if app.prompt_mode.is_some() {
        draw_prompt(frame, app, outer[2]);
    }
}

fn draw_body(frame: &mut Frame, app: &mut WritermApp, area: Rect) {
    let headings_w = if app.show_headings
        && area.width >= MIN_DOCUMENT_WIDTH + app.config.layout.headings_width + SIDEBAR_GAP
    {
        app.config.layout.headings_width
    } else {
        0
    };
    let headings_gap_w = if headings_w > 0 { SIDEBAR_GAP } else { 0 };
    let files_w = if app.show_files
        && area.width
            >= MIN_DOCUMENT_WIDTH
                + headings_w
                + headings_gap_w
                + app.config.layout.files_width
                + SIDEBAR_GAP
    {
        app.config.layout.files_width
    } else {
        0
    };
    let files_gap_w = if files_w > 0 { SIDEBAR_GAP } else { 0 };

    let chunks = Layout::horizontal([
        Constraint::Length(headings_w),
        Constraint::Length(headings_gap_w),
        Constraint::Min(MIN_DOCUMENT_WIDTH.min(area.width)),
        Constraint::Length(files_gap_w),
        Constraint::Length(files_w),
    ])
    .split(area);

    app.headings_area = if headings_w > 0 {
        chunks[0]
    } else {
        Rect::default()
    };
    app.document_area = chunks[2];
    if files_w > 0 && chunks[4].height > 0 {
        // Split the right sidebar vertically: top 7/8 stays the filesystem
        // browser, bottom 1/8 becomes the document-length readout. We fall
        // back to the whole sidebar as the files area when the height is too
        // small for the ratio to produce a non-zero metrics slice.
        let (files_only, metrics) = if chunks[4].height >= SIDEBAR_PARTS {
            let vchunks = Layout::vertical([
                Constraint::Ratio(FILES_PARTS, METRICS_PARTS),
                Constraint::Ratio(METRICS_PARTS, FILES_PARTS),
            ])
            .split(chunks[4]);
            (vchunks[0], vchunks[1])
        } else {
            (chunks[4], Rect::default())
        };
        app.files_area = files_only;
        app.metrics_area = metrics;
        draw_files(frame, app, files_only);
        if metrics.height > 0 {
            draw_metrics(frame, app, metrics);
        }
    } else {
        app.files_area = Rect::default();
        app.metrics_area = Rect::default();
    }

    if headings_w > 0 {
        draw_headings(frame, app, chunks[0]);
    }
    draw_document(frame, app, chunks[2]);
}

fn draw_top_ribbon(frame: &mut Frame, app: &WritermApp, area: Rect) {
    let name = app
        .current_file_path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let dirty = if app.editor.is_dirty() {
        "dirty"
    } else {
        "saved"
    };
    let heading = app.current_heading().unwrap_or_default();
    let message = app
        .notification
        .as_ref()
        .map(|(text, _, _)| format!(" | {text}"))
        .unwrap_or_default();
    let text = format!(
        " {name} | {dirty} | {} words | {} | {}{}",
        app.word_count(),
        truncate(&heading, 28),
        truncate(
            &app.current_file_path.display().to_string(),
            area.width.saturating_sub(48) as usize
        ),
        message
    );
    let style = app
        .notification
        .as_ref()
        .map(|(_, _, is_error)| {
            if *is_error {
                Style::default()
                    .fg(theme::notify_error_fg())
                    .bg(theme::notify_error_bg())
            } else {
                Style::default()
                    .fg(theme::status_fg())
                    .bg(theme::status_bg())
            }
        })
        .unwrap_or_else(|| {
            Style::default()
                .fg(theme::status_fg())
                .bg(theme::status_bg())
        });
    frame.render_widget(
        Paragraph::new(truncate(&text, area.width as usize)).style(style),
        area,
    );
}

fn draw_bottom_bar(frame: &mut Frame, app: &mut WritermApp, area: Rect) {
    let render = if app.source_peek { "off" } else { "on" };
    let headings = if app.show_headings { "on" } else { "off" };
    let files = if app.show_files { "on" } else { "off" };
    let text = format!(
        " WRITERM  |  C-S: save  C-B/I/K: format  C-N: new  C-Q: quit  |  [C-M: render {render}]  [F3: headings {headings}]  [F2: files {files}] "
    );
    set_control_areas(app, area, &text, headings, files);
    frame.render_widget(
        Paragraph::new(truncate(&text, area.width as usize)).style(
            Style::default()
                .fg(theme::status_fg())
                .bg(theme::status_bg()),
        ),
        area,
    );
}

fn draw_headings(frame: &mut Frame, app: &WritermApp, area: Rect) {
    let mut lines = Vec::new();
    let max_rows = area.height as usize;
    for entry in app
        .outline_entries
        .iter()
        .skip(app.heading_scroll as usize)
        .take(max_rows)
    {
        let indent = "  ".repeat(entry.depth.saturating_sub(1));
        let active = entry.line <= app.editor.state.cursor_line;
        let style = if active {
            Style::default()
                .fg(theme::heading_h2())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::text_secondary())
        };
        lines.push(Line::from(Span::styled(
            truncate(&format!("{indent}{}", entry.label), area.width as usize),
            style,
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme::bg_surface())),
        area,
    );
}

fn draw_document(frame: &mut Frame, app: &mut WritermApp, area: Rect) {
    app.refresh_render_cache();
    let visual = app.visual_document();
    let max_scroll = visual
        .rows
        .len()
        .saturating_sub(area.height.max(1) as usize);
    app.document_scroll = app.document_scroll.min(max_scroll);
    let text = visual.to_text_with_selection(
        app.document_scroll,
        area.height as usize,
        app.editor
            .state
            .selected_char_range(app.editor.buffer.rope()),
        Style::default().bg(theme::selection_bg()),
    );
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(theme::text_primary())),
        area,
    );

    if let Some((x, y)) = cursor_position(app, area, &visual) {
        frame.set_cursor_position((x, y));
    }
}

fn cursor_position(
    app: &WritermApp,
    area: Rect,
    visual: &crate::visual::VisualDocument,
) -> Option<(u16, u16)> {
    let (row, col) = visual.source_to_display(app.editor.cursor_char_pos())?;
    if row < app.document_scroll {
        return None;
    }
    let rel_row = row - app.document_scroll;
    if rel_row >= area.height as usize {
        return None;
    }
    Some((
        area.x + col.min(area.width.saturating_sub(1) as usize) as u16,
        area.y + rel_row as u16,
    ))
}

fn draw_files(frame: &mut Frame, app: &mut WritermApp, area: Rect) {
    let rows = area.height as usize;
    app.workspace_viewport_rows = rows;
    let mut lines = Vec::new();
    for (idx, entry) in app
        .workspace_entries
        .iter()
        .enumerate()
        .skip(app.workspace_scroll as usize)
        .take(rows)
    {
        let selected = idx == app.workspace_selection;
        let (icon, style) = match entry.kind {
            WorkspaceEntryKind::Parent => ("<-", Style::default().fg(theme::text_dim())),
            WorkspaceEntryKind::Directory => ("/", Style::default().fg(theme::dir_color())),
            WorkspaceEntryKind::File if is_markdown_path(std::path::Path::new(&entry.name)) => {
                ("M", Style::default().fg(theme::accent_green()))
            }
            WorkspaceEntryKind::File => ("T", Style::default().fg(theme::text_dim())),
        };
        let style = if selected {
            style.bg(theme::bg_highlight()).add_modifier(Modifier::BOLD)
        } else {
            style
        };
        lines.push(Line::from(Span::styled(
            truncate(&format!(" {icon} {}", entry.name), area.width as usize),
            style,
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme::bg_surface())),
        area,
    );
}

fn draw_metrics(frame: &mut Frame, app: &WritermApp, area: Rect) {
    if area.height == 0 {
        return;
    }
    let metrics = app.document_metrics();
    let reading = format_reading_time(metrics.reading_secs);
    let width = area.width as usize;

    let title_style = Style::default()
        .fg(theme::text_secondary())
        .add_modifier(Modifier::BOLD);
    let value_style = Style::default().fg(theme::text_primary());
    let label_style = Style::default().fg(theme::text_dim());

    let title = Line::from(Span::styled(truncate("─ Doc ─", width), title_style));
    let chars_words = Line::from(Span::styled(
        truncate(
            &format!("{} ch · {} w", metrics.characters, metrics.words),
            width,
        ),
        value_style,
    ));
    let sent_para_read = Line::from(Span::styled(
        truncate(
            &format!(
                "{} sent · {} para · {} read",
                metrics.sentences, metrics.paragraphs, reading
            ),
            width,
        ),
        label_style,
    ));
    let sent_para = Line::from(Span::styled(
        truncate(
            &format!("{} sent · {} para", metrics.sentences, metrics.paragraphs),
            width,
        ),
        label_style,
    ));
    let reading_line = Line::from(Span::styled(
        truncate(&format!("{reading} read"), width),
        value_style,
    ));
    let emergency = Line::from(Span::styled(
        truncate(&format!("{} w · {} read", metrics.words, reading), width),
        value_style,
    ));

    // Show all five metrics whenever the panel is at least 3 rows tall, which
    // is the typical case for any reasonable terminal height. Smaller panels
    // gracefully drop the title and combine labels so the user still sees
    // every metric the spec calls out.
    let lines: Vec<Line> = match area.height {
        1 => vec![emergency],
        2 => vec![chars_words.clone(), sent_para_read],
        3 => vec![title.clone(), chars_words, sent_para_read],
        _ => vec![title, chars_words, sent_para, reading_line],
    };

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme::bg_surface())),
        area,
    );
}

fn draw_prompt(frame: &mut Frame, app: &WritermApp, area: Rect) {
    let prompt = format!(" New Markdown file: {}", app.prompt_buffer);
    frame.render_widget(
        Paragraph::new(truncate(&prompt, area.width as usize)).style(
            Style::default()
                .fg(theme::text_bright())
                .bg(theme::bg_active()),
        ),
        area,
    );
}

fn set_control_areas(app: &mut WritermApp, area: Rect, text: &str, headings: &str, files: &str) {
    let headings_label = format!("[F3 headings:{headings}]");
    let files_label = format!("[F2 files:{files}]");
    app.headings_control_area = control_area(area, text, &headings_label);
    app.files_control_area = control_area(area, text, &files_label);
}

fn control_area(area: Rect, text: &str, label: &str) -> Rect {
    let Some(start) = text.find(label) else {
        return Rect::default();
    };
    let start = start as u16;
    let width = label.len() as u16;
    if start >= area.width {
        return Rect::default();
    }
    Rect::new(
        area.x + start,
        area.y,
        width.min(area.width.saturating_sub(start)),
        area.height.min(1),
    )
}

fn truncate(s: &str, max_width: usize) -> String {
    s.chars().take(max_width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::WritermApp;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tempfile::TempDir;
    use writerm_config::Config;

    fn rendered_buffer(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn rendered_rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|col| buffer[(col, row)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn renders_ribbon_headings_document_files_and_keybar() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Title\n\nBody text").unwrap();
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);

        assert!(rendered.contains("note.md"));
        assert!(rendered.contains("Title"));
        assert!(rendered.contains("Body text"));
        assert!(rendered.contains("Ctrl-S save"));
        assert!(rendered.contains("[F3 headings:on]"));
        assert!(rendered.contains("[F2 files:on]"));
        assert!(app.headings_area.width > 0);
        assert!(app.document_area.width > 0);
        assert!(app.files_area.width > 0);
        assert!(app.metrics_area.width > 0);
        assert!(app.document_area.x >= app.headings_area.x + app.headings_area.width + SIDEBAR_GAP);
        assert!(app.files_area.x >= app.document_area.x + app.document_area.width + SIDEBAR_GAP);
        // The metrics panel sits directly under the filesystem browser and
        // is the bottom eighth (or as close as the layout can manage) of the
        // right-hand sidebar.
        assert_eq!(
            app.files_area.x, app.metrics_area.x,
            "metrics panel must share the files sidebar's column"
        );
        assert_eq!(
            app.files_area.width, app.metrics_area.width,
            "metrics panel must share the files sidebar's width"
        );
        assert!(
            app.metrics_area.y >= app.files_area.y,
            "metrics panel must start at or below the files area"
        );
        let combined_height = app.files_area.height + app.metrics_area.height;
        assert_eq!(combined_height, 22, "top + bottom should fill the body");
        // Bottom slice is approximately the bottom eighth (within 1 row of the
        // true 1/8 of 22) and the top slice gets the rest.
        assert!(
            (2..=4).contains(&app.metrics_area.height),
            "metrics panel height should be 2-4 rows for a 24-line terminal, got {}",
            app.metrics_area.height
        );
        assert!(app.headings_control_area.width > 0);
        assert!(app.files_control_area.width > 0);
    }

    #[test]
    fn metrics_panel_renders_all_five_metrics_for_three_row_height() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        // 3 paragraphs of text, each with one sentence-ending punctuation
        // mark. Words: 3 + 4 + 3 = 10. Chars: 19 + 2 + 19 + 2 + 15 = 57.
        std::fs::write(
            &path,
            "Hello there friend.\n\nA second line here.\n\nThird para now!",
        )
        .unwrap();
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);

        // Title bar of the metrics panel.
        assert!(
            rendered.contains("─ Doc ─"),
            "metrics title should be present"
        );
        // Word count and character count appear on the first data line.
        assert!(rendered.contains("10 w"), "word count should be present");
        assert!(
            rendered.contains("57 ch"),
            "character count should be present"
        );
        // Sentences and paragraphs appear on the combined data line.
        assert!(
            rendered.contains("3 sent"),
            "sentence count should be present"
        );
        assert!(
            rendered.contains("3 para"),
            "paragraph count should be present"
        );
        // Reading time is included on the same line for 3-row panels.
        // 10 words at 180 wpm = ceil(3.33s) = 4s.
        assert!(
            rendered.contains("4s read"),
            "reading time should be present, got: {rendered}"
        );
    }

    #[test]
    fn metrics_panel_updates_as_user_types() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "one.").unwrap();
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();
        app.document_area = Rect::new(0, 0, 80, 1);
        // Park the cursor at the end of the existing text so typing extends
        // the document instead of inserting at position 0.
        app.editor.move_cursor_to_char_pos(app.editor.text().len());

        // Type "two three" at the end so we go from 1 word / 1 paragraph to
        // 3 words / 1 paragraph.
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        for ch in "two three".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);

        assert!(
            rendered.contains("3 w"),
            "should reflect 3 words after typing"
        );
        assert!(
            rendered.contains("1 para"),
            "should be 1 paragraph after typing"
        );
    }

    #[test]
    fn narrow_width_collapses_sidebars_and_uses_unbordered_document() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Title").unwrap();
        let backend = TestBackend::new(60, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);

        assert_eq!(app.headings_area.width, 0);
        assert_eq!(app.files_area.width, 0);
        // Metrics panel only exists as the bottom eighth of the files
        // sidebar, so it must be empty whenever the sidebar is hidden.
        assert_eq!(app.metrics_area.width, 0);
        assert_eq!(app.metrics_area.height, 0);
        assert!(app.document_area.width > 0);
        assert!(!rendered.contains('┌'));
        assert!(!rendered.contains('│'));
    }

    #[test]
    fn hiding_files_sidebar_via_f2_also_hides_metrics_panel() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Title\n\nHello there friend.").unwrap();
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();

        // Sanity check: with the sidebar visible, the metrics panel exists.
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert!(app.files_area.width > 0);
        assert!(app.metrics_area.width > 0);

        // Toggle the files sidebar off; the metrics panel must collapse with
        // it, since the panel only lives inside the files sidebar.
        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);

        assert_eq!(app.files_area.width, 0);
        assert_eq!(app.metrics_area.width, 0);
        assert_eq!(app.metrics_area.height, 0);
        assert!(
            !rendered.contains("─ Doc ─"),
            "metrics title should be hidden when the files sidebar is hidden"
        );
    }

    #[test]
    fn long_document_lines_wrap_in_the_writing_surface() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha beta gamma delta epsilon zeta").unwrap();
        let backend = TestBackend::new(20, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rows = rendered_rows(&terminal);

        assert!(rows[1].contains("alpha beta gamma"));
        assert!(rows[2].contains("delta epsilon"));
    }

    #[test]
    fn ctrl_m_disables_markdown_rendering_and_label_reports_state() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Title").unwrap();
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);
        assert!(rendered.contains("[Ctrl-M render:on]"));
        assert!(!rendered.contains("# Title"));

        app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::CONTROL));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let source = rendered_buffer(&terminal);

        assert!(source.contains("[Ctrl-M render:off]"));
        assert!(source.contains("# Title"));
    }

    #[test]
    fn rendered_shift_selection_uses_selection_background() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Heading").unwrap();
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();
        app.show_headings = false;
        app.show_files = false;
        app.editor.move_cursor_to_char_pos(2);

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let buffer = terminal.backend().buffer();
        let selected = &buffer[(0, 1)];
        let unselected = &buffer[(1, 1)];
        assert_eq!(selected.symbol(), "H");
        assert_eq!(selected.bg, theme::selection_bg());
        assert_eq!(selected.fg, theme::heading_h1());
        assert_ne!(unselected.bg, theme::selection_bg());
    }

    #[test]
    fn source_peek_shift_selection_uses_selection_background() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Heading").unwrap();
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();
        app.show_headings = false;
        app.show_files = false;
        app.source_peek = true;

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let buffer = terminal.backend().buffer();
        let selected = &buffer[(0, 1)];
        let unselected = &buffer[(1, 1)];
        assert_eq!(selected.symbol(), "#");
        assert_eq!(selected.bg, theme::selection_bg());
        assert_eq!(selected.fg, theme::text_primary());
        assert_ne!(unselected.bg, theme::selection_bg());
    }

    #[test]
    fn cursor_advances_after_typing_space_at_end_of_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "hello").unwrap();
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();
        app.show_headings = false;
        app.show_files = false;
        app.editor.move_cursor_to_char_pos(5);

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let before = terminal.backend().cursor_position();

        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let after = terminal.backend().cursor_position();

        assert_eq!(
            after.x,
            before.x + 1,
            "cursor should advance one cell after space"
        );
        assert_eq!(after.y, before.y, "cursor should stay on the same row");
    }

    #[test]
    fn end_key_on_line_with_trailing_whitespace_lands_past_the_space() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "hello ").unwrap();
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();
        app.show_headings = false;
        app.show_files = false;

        // Draw first to populate document_area.
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        app.editor.move_cursor_to_char_pos(0);

        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        assert_eq!(
            app.editor.cursor_char_pos(),
            6,
            "End should reach past the trailing space"
        );
        let cursor = terminal.backend().cursor_position();
        assert_eq!(
            cursor.x,
            app.document_area.x + 6,
            "cursor x should be 6 cells past document start"
        );
    }

    #[test]
    fn cursor_moves_to_wrapped_row_after_typing_at_trailing_space_wrap_boundary() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "abcdefgh").unwrap();
        let backend = TestBackend::new(8, 4);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();
        app.show_headings = false;
        app.show_files = false;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert_eq!(
            app.document_area.width, 8,
            "precondition: doc area is 8 wide"
        );

        app.editor.move_cursor_to_char_pos(8);
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        assert_eq!(app.editor.cursor_char_pos(), 9);
        assert_eq!(app.visual_document().source_to_display(8), Some((1, 0)));
        assert_eq!(app.visual_document().source_to_display(9), Some((1, 1)));
        terminal.backend_mut().assert_cursor_position((1, 2));
    }

    #[test]
    fn selection_over_synthesized_trailing_space_cell() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "hello").unwrap();
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();
        app.show_headings = false;
        app.show_files = false;

        // Draw first to populate document_area.
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        app.editor.move_cursor_to_char_pos(5);
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

        // Select from char 4 to char 6 (covering the synthesized space at char 5).
        app.editor.move_cursor_to_char_pos(4);
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let buffer = terminal.backend().buffer();
        let doc_col = (app.document_area.x + 5).min(buffer.area.width.saturating_sub(1));
        let doc_row = app.document_area.y;
        let cell = &buffer[(doc_col, doc_row)];
        assert_eq!(cell.symbol(), " ");
        assert_eq!(
            cell.bg,
            theme::selection_bg(),
            "synthesized space cell should show selection bg"
        );
    }
}
