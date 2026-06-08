use crate::app::{WritermApp, is_markdown_path};
use jones_theme as theme;
use jones_workspace::WorkspaceEntryKind;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

const MIN_DOCUMENT_WIDTH: u16 = 40;

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
        && area.width >= MIN_DOCUMENT_WIDTH + app.config.layout.headings_width
    {
        app.config.layout.headings_width
    } else {
        0
    };
    let files_w = if app.show_files
        && area.width >= MIN_DOCUMENT_WIDTH + headings_w + app.config.layout.files_width
    {
        app.config.layout.files_width
    } else {
        0
    };

    let chunks = Layout::horizontal([
        Constraint::Length(headings_w),
        Constraint::Min(MIN_DOCUMENT_WIDTH.min(area.width)),
        Constraint::Length(files_w),
    ])
    .split(area);

    app.headings_area = if headings_w > 0 {
        chunks[0]
    } else {
        Rect::default()
    };
    app.document_area = chunks[1];
    app.files_area = if files_w > 0 {
        chunks[2]
    } else {
        Rect::default()
    };

    if headings_w > 0 {
        draw_headings(frame, app, chunks[0]);
    }
    draw_document(frame, app, chunks[1]);
    if files_w > 0 {
        draw_files(frame, app, chunks[2]);
    }
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

fn draw_bottom_bar(frame: &mut Frame, app: &WritermApp, area: Rect) {
    let mode = if app.source_peek {
        "source"
    } else {
        "rendered"
    };
    let text = format!(
        " Ctrl-S save  Ctrl-B/I/K format  Ctrl-1..6 heading  Ctrl-F find  Ctrl-M {mode}  F2 files  F3 headings  Ctrl-N new  Ctrl-Q quit "
    );
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
    let text = if app.source_peek {
        source_text(app, area.height as usize)
    } else {
        rendered_text(app, area.height as usize)
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(theme::text_primary())),
        area,
    );

    if let Some((x, y)) = cursor_position(app, area) {
        frame.set_cursor_position((x, y));
    }
}

fn source_text(app: &WritermApp, height: usize) -> Text<'static> {
    let lines = app
        .editor
        .text()
        .lines()
        .skip(app.document_scroll)
        .take(height)
        .map(|line| {
            Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(theme::text_primary()),
            ))
        })
        .collect::<Vec<_>>();
    Text::from(lines)
}

fn rendered_text(app: &WritermApp, height: usize) -> Text<'static> {
    Text::from(
        app.rendered
            .lines
            .iter()
            .skip(app.document_scroll)
            .take(height)
            .map(|line| {
                Line::from(
                    line.spans
                        .iter()
                        .map(|span| Span::styled(span.content.clone(), span.style))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>(),
    )
}

fn cursor_position(app: &WritermApp, area: Rect) -> Option<(u16, u16)> {
    let (row, col) = if app.source_peek {
        (app.editor.state.cursor_line, app.editor.state.cursor_col)
    } else {
        app.rendered
            .source_to_display(app.editor.cursor_char_pos())?
    };
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

fn truncate(s: &str, max_width: usize) -> String {
    s.chars().take(max_width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::WritermApp;
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
        assert!(app.headings_area.width > 0);
        assert!(app.document_area.width > 0);
        assert!(app.files_area.width > 0);
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
        assert!(app.document_area.width > 0);
        assert!(!rendered.contains('┌'));
        assert!(!rendered.contains('│'));
    }
}
