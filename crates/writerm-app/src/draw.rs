use crate::app::{WritermApp, is_markdown_path};
use crate::visual::VisualDocument;
use jones_outline;
use jones_text;
use jones_theme as theme;
use jones_workspace::WorkspaceEntryKind;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::metrics::format_reading_time;

const MIN_DOCUMENT_WIDTH: u16 = 40;
/// Each sidebar panel renders inside a `Block` with `Borders::ALL`, so the
/// outer area must be 2 cells wider than the configured inner content width.
const SIDEBAR_BORDER_OVERHEAD: u16 = 2;
/// Width of the vertical-line separator drawn between a sidebar and the
/// document area. The line is rendered on the terminal background so the
/// document area stays visually "open" instead of being walled in.
const SIDEBAR_SEP_WIDTH: u16 = 1;
/// How the right-hand sidebar is split between the filesystem browser and
/// the document-length metrics panel. The user-visible model is "split the
/// sidebar in half, then shrink the bottom half down to roughly 1/8", which
/// leaves 7/8 for the filesystem browser and 1/8 for the metrics readout.
const FILES_PARTS: u32 = 7;
const METRICS_PARTS: u32 = 1;
const SIDEBAR_PARTS: u16 = (FILES_PARTS + METRICS_PARTS) as u16;

/// Shared style for sidebar borders. The writerm surface doesn't currently
/// track focus (the document is the always-active surface), so all sidebars
/// render in the unfocused border style. The thin lines give the layout
/// definition without dominating the visual.
fn sidebar_border_style() -> Style {
    Style::default().fg(theme::border_unfocused())
}

/// Background used to fill the sidebar panels so they read as opaque cards
/// even when the user's terminal background is transparent.
fn sidebar_bg() -> ratatui::style::Color {
    theme::bg_surface()
}

/// Style for the vertical/horizontal separator lines drawn between the
/// sidebar panels and the document area.
fn separator_style() -> Style {
    Style::default().fg(theme::border_unfocused())
}

pub fn draw(frame: &mut Frame, app: &mut WritermApp) {
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
    let headings_inner_w = app.config.layout.headings_width;
    let files_inner_w = app.config.layout.files_width;
    let headings_block_w = headings_inner_w + SIDEBAR_BORDER_OVERHEAD;
    let files_block_w = files_inner_w + SIDEBAR_BORDER_OVERHEAD;

    // Decide which sidebars can actually fit. The document area gets a hard
    // minimum width so we never squeeze the writing surface below the point
    // where prose becomes unreadable.
    let show_headings = app.show_headings
        && area.width >= MIN_DOCUMENT_WIDTH + headings_block_w + SIDEBAR_SEP_WIDTH;
    let show_files = app.show_files
        && area.width
            >= MIN_DOCUMENT_WIDTH
                + headings_block_w
                + SIDEBAR_SEP_WIDTH
                + files_block_w
                + SIDEBAR_SEP_WIDTH;

    let headings_w = if show_headings { headings_block_w } else { 0 };
    let files_w = if show_files { files_block_w } else { 0 };
    let left_sep = if show_headings { SIDEBAR_SEP_WIDTH } else { 0 };
    let right_sep = if show_files { SIDEBAR_SEP_WIDTH } else { 0 };

    let chunks = Layout::horizontal([
        Constraint::Length(headings_w),
        Constraint::Length(left_sep),
        Constraint::Min(MIN_DOCUMENT_WIDTH.min(area.width)),
        Constraint::Length(right_sep),
        Constraint::Length(files_w),
    ])
    .split(area);
    app.document_area = chunks[2];

    if show_headings {
        draw_headings_panel(frame, app, chunks[0]);
    } else {
        app.headings_area = Rect::default();
    }

    if left_sep > 0 {
        // The left separator between the section-browser border and the
        // heading gutter is exactly one blank column — no visible rule is
        // drawn. This keeps the document area visually open.
    }

    draw_document(frame, app, chunks[2]);

    if right_sep > 0 {
        draw_vertical_separator(frame, app, chunks[3]);
    }

    if show_files {
        draw_files_panel(frame, app, chunks[4]);
    } else {
        app.files_area = Rect::default();
        app.metrics_area = Rect::default();
    }
}

fn draw_top_ribbon(frame: &mut Frame, app: &mut WritermApp, area: Rect) {
    let (cursor_word, total_words) = app.cursor_word_progress();
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
    // Mode badge mirrors the "READ / EDIT / SPLIT" pattern from termite so
    // the user can see at a glance whether they're writing rendered
    // markdown or peeking at the raw source.
    let mode = if app.source_peek { "SOURCE" } else { "WRITE" };
    let mode_color = if app.source_peek {
        theme::accent_yellow()
    } else {
        theme::accent_cyan()
    };
    let text = format!(
        " {name} | {dirty} | {cursor_word} / {total_words} words | {} | {}{}",
        truncate(&heading, 28),
        truncate(
            &app.current_file_path.display().to_string(),
            area.width.saturating_sub(56) as usize
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
    // The mode badge sits at the right end of the ribbon as a small accent
    // block. We overlay it on the ribbon so its color is visible regardless
    // of where the rest of the text is truncated.
    if area.width >= 8 {
        let badge_text = format!(" {mode} ");
        let badge_width = badge_text.chars().count() as u16;
        let badge_x = area.x + area.width - badge_width;
        frame.render_widget(
            Paragraph::new(badge_text).style(
                Style::default()
                    .fg(theme::bg_dark())
                    .bg(mode_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Rect::new(badge_x, area.y, badge_width, 1),
        );
    }
}

fn draw_bottom_bar(frame: &mut Frame, app: &mut WritermApp, area: Rect) {
    let render = if app.source_peek { "off" } else { "on" };
    let headings = if app.show_headings { "on" } else { "off" };
    let files = if app.show_files { "on" } else { "off" };
    let indent = if app.paragraph_indent { "on" } else { "off" };
    let text = format!(
        " WRITERM | ^S:save  ^B/I/K:fmt  ^N:new  ^Q:quit | [^M:render {render}] [F4:indent {indent}] [F3:hd {headings}] [F2:files {files}] "
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

fn draw_headings_panel(frame: &mut Frame, app: &mut WritermApp, area: Rect) {
    let title = " Headings ";
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            title,
            Style::default()
                .fg(theme::text_secondary())
                .add_modifier(Modifier::BOLD),
        )))
        .border_style(sidebar_border_style())
        .style(Style::default().bg(sidebar_bg()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.headings_area = inner;
    draw_headings_content(frame, app, inner);
}

fn draw_headings_content(frame: &mut Frame, app: &mut WritermApp, area: Rect) {
    let max_rows = area.height as usize;
    let all_lines = app.build_heading_visual_lines(area.width);

    // Clamp scroll so the panel never becomes spuriously empty when
    // outline entries (and thus visual lines) shrink.
    let max_scroll = all_lines.len().saturating_sub(area.height.max(1) as usize);
    let max_scroll = u16::try_from(max_scroll).unwrap_or(u16::MAX);
    app.heading_scroll = app.heading_scroll.min(max_scroll);

    let lines: Vec<Line> = all_lines
        .iter()
        .skip(app.heading_scroll as usize)
        .take(max_rows)
        .map(|hl| Line::from(Span::styled(hl.content.clone(), hl.style)))
        .collect();

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(sidebar_bg())),
        area,
    );
}

fn draw_files_panel(frame: &mut Frame, app: &mut WritermApp, area: Rect) {
    let cwd_display = app.cwd.display().to_string();
    let title = truncate(&format!(" Files: {cwd_display} "), area.width as usize);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            title,
            Style::default()
                .fg(theme::text_secondary())
                .add_modifier(Modifier::BOLD),
        )))
        .border_style(sidebar_border_style())
        .style(Style::default().bg(sidebar_bg()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split the inner area into the file browser (top 7/8) and the metrics
    // panel (bottom 1/8). We need at least SIDEBAR_PARTS rows to form a
    // non-zero metrics slice; below that threshold the file browser gets
    // the whole inner area and the metrics panel collapses.
    if inner.height < SIDEBAR_PARTS {
        app.files_area = inner;
        app.metrics_area = Rect::default();
        draw_files_content(frame, app, inner);
        return;
    }
    let (files_only, metrics_only) = {
        let vchunks = Layout::vertical([
            Constraint::Ratio(FILES_PARTS, METRICS_PARTS),
            Constraint::Ratio(METRICS_PARTS, FILES_PARTS),
        ])
        .split(inner);
        (vchunks[0], vchunks[1])
    };
    app.files_area = files_only;
    app.metrics_area = metrics_only;
    draw_files_content(frame, app, files_only);
    // The horizontal divider sits at the boundary between the file
    // browser and the metrics panel. We draw it on the sidebar's
    // background so it reads as a thin structural line, not a separator
    // wall.
    if metrics_only.height > 0 {
        let divider_y = files_only.y + files_only.height;
        let line: Line = Line::from(Span::styled(
            "─".repeat(inner.width as usize),
            separator_style(),
        ));
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(sidebar_bg())),
            Rect::new(inner.x, divider_y, inner.width, 1),
        );
        draw_metrics_content(frame, app, metrics_only);
    }
}

fn draw_files_content(frame: &mut Frame, app: &mut WritermApp, area: Rect) {
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
        // Color-coded icons mirror the termite workspace panel so the
        // user can scan entries at a glance: directories in cyan, the
        // current markdown file in green, other files in dim text.
        let (icon, icon_style, name_style) = match entry.kind {
            WorkspaceEntryKind::Parent => (
                "\u{2190}",
                Style::default().fg(theme::text_dim()),
                Style::default().fg(theme::text_dim()),
            ),
            WorkspaceEntryKind::Directory => (
                "/",
                Style::default().fg(theme::dir_color()),
                Style::default().fg(theme::text_primary()),
            ),
            WorkspaceEntryKind::File if is_markdown_path(std::path::Path::new(&entry.name)) => (
                "M",
                Style::default()
                    .fg(theme::accent_green())
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(theme::accent_green()),
            ),
            WorkspaceEntryKind::File => (
                "T",
                Style::default().fg(theme::text_dim()),
                Style::default().fg(theme::text_secondary()),
            ),
        };
        let base_style = if selected {
            Style::default()
                .bg(theme::bg_highlight())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(sidebar_bg())
        };
        let final_icon = icon_style.patch(base_style);
        let final_name = name_style.patch(base_style);
        let cursor = if selected { ">" } else { " " };
        let line = Line::from(vec![
            Span::styled(cursor.to_string(), final_icon),
            Span::styled(format!(" {icon} "), final_icon),
            Span::styled(
                truncate(&entry.name, area.width.saturating_sub(5) as usize),
                final_name,
            ),
        ]);
        lines.push(line);
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(sidebar_bg())),
        area,
    );
}

fn draw_metrics_content(frame: &mut Frame, app: &WritermApp, area: Rect) {
    if area.height == 0 {
        return;
    }
    let metrics = app.document_metrics();
    let reading = format_reading_time(metrics.reading_secs);
    let width = area.width as usize;

    // Numbers get a brighter, more saturated style than the labels so the
    // small panel reads at a glance. This is the "finer colors pass" that
    // gives the writerm sidebar the same visual rhythm as the termite
    // workspace panel.
    let title_style = Style::default()
        .fg(theme::text_secondary())
        .add_modifier(Modifier::BOLD);
    let value_style = Style::default()
        .fg(theme::accent_cyan())
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(theme::text_dim());
    let sep = Span::styled(" · ", label_style);

    let title = Line::from(Span::styled(truncate("── Doc ──", width), title_style));
    let reading_value = format!("{reading} read");
    let read_span = Span::styled(reading_value, value_style);
    let read_line = Line::from(read_span.clone());
    let chars_words_line = Line::from(vec![
        Span::styled(format!("{} w", metrics.words), value_style),
        sep.clone(),
        Span::styled(format!("{} ch", metrics.characters), value_style),
    ]);
    let sents_para_line = Line::from(vec![
        Span::styled(format!("{} sent", metrics.sentences), value_style),
        sep.clone(),
        Span::styled(format!("{} para", metrics.paragraphs), value_style),
    ]);
    let sents_para_read_line = Line::from(vec![
        Span::styled(format!("{} sent", metrics.sentences), value_style),
        sep,
        Span::styled(format!("{} para", metrics.paragraphs), value_style),
        Span::styled(" · ", label_style),
        read_span,
    ]);

    // Adapt the layout to the panel's height so the user always sees as
    // much of the readout as possible. The values are clustered on the
    // left and the labels on the right so eye scanning is cheap.
    let lines: Vec<Line> = match area.height {
        1 => vec![read_line],
        2 => vec![chars_words_line, sents_para_read_line],
        3 => vec![title, chars_words_line, sents_para_read_line],
        _ => vec![title, chars_words_line, sents_para_line, read_line],
    };

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(sidebar_bg())),
        area,
    );
}

fn draw_vertical_separator(frame: &mut Frame, app: &WritermApp, area: Rect) {
    // The vertical line between a sidebar and the document is drawn on the
    // terminal background (no fill) so the document area stays visually
    // "open". The line is a thin unfocused-border color so it reads as a
    // structural separator without competing with the document text.
    // A rudimentary scrollbar thumb is rendered as a proportional block
    // of the track when there are more visual rows than fit in the viewport.
    let track_style = separator_style();
    let thumb_style = Style::default().fg(theme::accent_cyan());
    let height = area.height as usize;
    let total = app.visual_rows_len();
    let viewport = height;
    let scroll = app.document_scroll;

    if total == 0 || viewport >= total {
        let line = Line::from(Span::styled("│".repeat(height), track_style));
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let thumb_size = ((viewport as u64 * viewport as u64) / total as u64)
        .max(1)
        .min(viewport as u64) as usize;
    let thumb_start = ((scroll as u64 * viewport as u64) / total as u64)
        .min((viewport - thumb_size) as u64) as usize;

    // Build three spans for a single-paint vertical line: track above
    // the thumb, the thumb itself, and track below.
    let mut spans: Vec<Span> = Vec::with_capacity(3);
    if thumb_start > 0 {
        spans.push(Span::styled("│".repeat(thumb_start), track_style));
    }
    spans.push(Span::styled("█".repeat(thumb_size), thumb_style));
    let after = height.saturating_sub(thumb_start + thumb_size);
    if after > 0 {
        spans.push(Span::styled("│".repeat(after), track_style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_document(frame: &mut Frame, app: &mut WritermApp, area: Rect) {
    app.refresh_render_cache();
    let visual = app.visual_document();
    let max_scroll = visual
        .rows
        .len()
        .saturating_sub(area.height.max(1) as usize);
    app.document_scroll = app.document_scroll.min(max_scroll);

    let layout = app.heading_gutter_layout();

    // Draw the heading-marker gutter on the left. Non-heading rows and
    // wrapped continuation rows show blank gutter cells.
    if layout.gutter_cells > 0 {
        let gutter_area = Rect::new(area.x, area.y, layout.gutter_cells, area.height);
        draw_heading_gutter(frame, app, &visual, gutter_area);
    }

    // The text area begins after the gutter + blank separator and ends
    // before the right margin. layout already accounts for the margin.
    let text_area = Rect::new(
        area.x + layout.text_x_offset,
        area.y,
        layout.text_width,
        area.height,
    );

    let text = visual.to_text_with_selection(
        app.document_scroll,
        text_area.height as usize,
        app.editor
            .state
            .selected_char_range(app.editor.buffer.rope()),
        Style::default().bg(theme::selection_bg()),
    );
    // The document area deliberately has no background color, so the user's
    // terminal background shows through wherever the rendered markdown
    // doesn't have its own background. The sidebars keep their filled-in
    // surfaces for definition against the open document.
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(theme::text_primary())),
        text_area,
    );

    if let Some((x, y)) = cursor_position(app, text_area, &visual) {
        frame.set_cursor_position((x, y));
    }
}

/// Render the ATX `#` markers for heading rows in the gutter area. Only
/// the *first* visual row of each heading receives markers; wrapped
/// continuation rows and non-heading rows leave the gutter blank.
fn draw_heading_gutter(frame: &mut Frame, app: &WritermApp, visual: &VisualDocument, area: Rect) {
    use std::collections::HashMap;

    if area.width == 0 || area.height == 0 {
        return;
    }

    // Source line → heading depth for quick lookup. Only actual heading
    // entries (not section/symbol fallbacks) receive gutter markers.
    let line_depths: HashMap<usize, usize> = app
        .outline_entries
        .iter()
        .filter(|e| matches!(e.kind, jones_outline::OutlineKind::Heading))
        .map(|e| (e.line, e.depth))
        .collect();

    let rope = app.editor.buffer.rope();
    let max_depth = app.max_heading_depth().min(area.width as usize);

    let mut lines: Vec<Line> = Vec::with_capacity(area.height as usize);
    let mut prev_source_line: Option<usize> = None;

    let scroll = app.document_scroll;
    let end = (scroll + area.height as usize).min(visual.rows.len());

    // Seed prev_source_line from the row just above the scroll origin so
    // that a wrapped continuation row at the top of the viewport is
    // correctly recognised as a continuation (and gets no gutter marker)
    // rather than being mistaken for the first row of a heading.
    if scroll > 0
        && let Some(source_pos) = visual.display_to_source(scroll - 1, 0)
    {
        prev_source_line = Some(rope.char_to_line(source_pos));
    }

    for row in scroll..end {
        let marker_depth = if let Some(source_pos) = visual.display_to_source(row, 0) {
            let source_line = rope.char_to_line(source_pos);
            let is_first_row = prev_source_line != Some(source_line);
            prev_source_line = Some(source_line);
            if is_first_row {
                line_depths.get(&source_line).copied()
            } else {
                None
            }
        } else {
            prev_source_line = None;
            None
        };

        let marker_text = match marker_depth {
            Some(depth) if depth > 0 => "#".repeat(depth.min(max_depth)),
            _ => String::new(),
        };

        let display = jones_text::truncate_to_display_width(&marker_text, area.width as usize);
        lines.push(Line::from(Span::styled(
            display.to_string(),
            Style::default().fg(theme::text_dim()),
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
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
    let headings_label = format!("[F3:hd {headings}]");
    let files_label = format!("[F2:files {files}]");
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
    jones_text::truncate_to_display_width(s, max_width).to_string()
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

    fn test_config() -> Config {
        let mut config = Config::default();
        config.layout.paragraph_indent = false;
        config
    }

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
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);

        assert!(rendered.contains("note.md"));
        assert!(rendered.contains("Title"));
        assert!(rendered.contains("Body text"));
        assert!(rendered.contains("^S:save"));
        assert!(rendered.contains("F3:hd on"));
        assert!(rendered.contains("F2:files on"));
        assert!(rendered.contains(" WRITE "));
        assert!(app.headings_area.width > 0);
        assert!(app.document_area.width > 0);
        assert!(app.files_area.width > 0);
        assert!(app.metrics_area.width > 0);
        // The headings sidebar lives to the left of the document area with
        // a single vertical-line separator between them. The headings area
        // is the inner content of a bordered block, so the document area
        // sits one cell after the right border of that block plus the
        // separator.
        assert_eq!(
            app.document_area.x,
            app.headings_area.x
                + app.headings_area.width
                + SIDEBAR_BORDER_OVERHEAD / 2
                + SIDEBAR_SEP_WIDTH,
            "headings sidebar must end one cell before the document area"
        );
        // Symmetric relationship on the right: the files sidebar starts
        // one cell after the document area ends (separator + left border
        // of the files block).
        assert_eq!(
            app.files_area.x,
            app.document_area.x
                + app.document_area.width
                + SIDEBAR_SEP_WIDTH
                + SIDEBAR_BORDER_OVERHEAD / 2,
            "files sidebar must start one cell after the document area"
        );
        // The metrics panel sits directly under the filesystem browser and
        // is the bottom eighth (or as close as the layout can manage) of
        // the right-hand sidebar.
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
        assert!(
            (20..=22).contains(&combined_height),
            "files + metrics areas should fill the body, got {combined_height}"
        );
        // Bottom slice is approximately the bottom eighth (within 1 row of
        // the true 1/8 of 22) and the top slice gets the rest.
        assert!(
            (2..=4).contains(&app.metrics_area.height),
            "metrics panel height should be 2-4 rows for a 24-line terminal, got {}",
            app.metrics_area.height
        );
        assert!(app.headings_control_area.width > 0);
        assert!(app.files_control_area.width > 0);
    }

    #[test]
    fn sidebar_panels_have_visible_borders() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Title").unwrap();
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        // Each sidebar's top-left corner must show a block border.
        let headings_corner = buffer[(app.headings_area.x - 1, app.headings_area.y - 1)].symbol();
        let files_corner = buffer[(app.files_area.x - 1, app.files_area.y - 1)].symbol();
        assert_eq!(
            headings_corner, "┌",
            "headings panel should have a top-left border"
        );
        assert_eq!(
            files_corner, "┌",
            "files panel should have a top-left border"
        );

        // The left separator between the headings panel and the document
        // area is a blank column (no visible rule). The right separator
        // still uses a │ character.
        let left_sep_x = app.document_area.x - SIDEBAR_SEP_WIDTH;
        let left_sep_char = buffer[(left_sep_x, app.document_area.y)].symbol();
        let right_sep_x = app.document_area.x + app.document_area.width;
        let right_sep_char = buffer[(right_sep_x, app.document_area.y)].symbol();
        assert_eq!(
            left_sep_char, " ",
            "left separator should be a blank column"
        );
        assert_eq!(
            right_sep_char, "│",
            "right separator should be a vertical line"
        );
    }

    #[test]
    fn files_panel_renders_a_horizontal_divider_above_metrics() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Title\n\nBody text.").unwrap();
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        // The divider sits at the boundary between the files area and the
        // metrics area, on the sidebar's background.
        let divider_y = app.files_area.y + app.files_area.height;
        let divider_char = buffer[(app.files_area.x, divider_y)].symbol();
        assert_eq!(
            divider_char, "─",
            "horizontal divider should use the ─ character, got {divider_char:?}"
        );
    }

    #[test]
    fn document_area_keeps_the_terminal_background_visible() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Title\n\nBody text.").unwrap();
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        // Cells in the document area that are not covered by the
        // selection or a code-block background must use the
        // `Color::Reset` background so the terminal's own background
        // bleeds through. This is what makes the document area read as
        // an open surface against the filled-in sidebars.
        let doc_cell = &buffer[(app.document_area.x + 1, app.document_area.y + 2)];
        assert_eq!(
            doc_cell.bg,
            ratatui::style::Color::Reset,
            "document area should have no background, got {:?}",
            doc_cell.bg
        );
    }

    #[test]
    fn sidebar_panels_keep_their_filled_in_background() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Title").unwrap();
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        // Cells inside the sidebar panels should be filled with the
        // surface background so the sidebar reads as an opaque card
        // against the (transparent) document area. We sample cells in
        // the body of the headings panel (away from the top border) and
        // a non-selected file row to avoid the selection highlight.
        let headings_cell = &buffer[(app.headings_area.x, app.headings_area.y + 1)];
        // The parent entry at the top of the files panel is selected by
        // default, so sample a few rows down at the actual file entries.
        let files_cell = &buffer[(app.files_area.x, app.files_area.y + 2)];
        assert_eq!(headings_cell.bg, theme::bg_surface());
        assert_eq!(files_cell.bg, theme::bg_surface());
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
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);

        // Title bar of the metrics panel.
        assert!(
            rendered.contains("── Doc"),
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
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
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
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);

        assert_eq!(app.headings_area.width, 0);
        assert_eq!(app.files_area.width, 0);
        // Metrics panel only exists as the bottom eighth of the files
        // sidebar, so it must be empty whenever the sidebar is hidden.
        assert_eq!(app.metrics_area.width, 0);
        assert_eq!(app.metrics_area.height, 0);
        assert!(app.document_area.width > 0);
        // No block borders should appear when the sidebars are hidden.
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
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();

        // Sanity check: with the sidebar visible, the metrics panel exists.
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert!(app.files_area.width > 0);
        assert!(app.metrics_area.width > 0);

        // Toggle the files sidebar off; the metrics panel must collapse
        // with it, since the panel only lives inside the files sidebar.
        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);

        assert_eq!(app.files_area.width, 0);
        assert_eq!(app.metrics_area.width, 0);
        assert_eq!(app.metrics_area.height, 0);
        assert!(
            !rendered.contains("── Doc"),
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
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();

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
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);
        // The Ctrl-M render label sits in the bottom keybar; on a narrow
        // terminal it may be truncated, so check for the prefix that's
        // always visible.
        assert!(rendered.contains("^M:render"));
        // In rendered mode the heading marker gutter shows a dim '#' and
        // the rendered text shows the heading content without ATX markers.
        // The source `# Title` text should not appear in the document area.
        assert!(rendered.contains("Title"));

        app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::CONTROL));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let source = rendered_buffer(&terminal);

        assert!(source.contains("^M:render"));
        // Source-peek mode renders the raw text including ATX markers.
        assert!(source.contains("# Title"));
    }

    #[test]
    fn rendered_shift_selection_uses_selection_background() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Heading").unwrap();
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.show_headings = false;
        app.show_files = false;
        app.editor.move_cursor_to_char_pos(2);

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let buffer = terminal.backend().buffer();
        // With a level-1 heading the marker gutter occupies 1 cell and the
        // blank separator 1 cell, so the rendered text starts at column 2.
        let gutter_offset: u16 = (app.max_heading_depth() + 1) as u16;
        let selected = &buffer[(gutter_offset, 1)];
        let unselected = &buffer[(gutter_offset + 1, 1)];
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
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.show_headings = false;
        app.show_files = false;
        app.source_peek = true;

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let buffer = terminal.backend().buffer();
        let gutter_offset: u16 = (app.max_heading_depth() + 1) as u16;
        let selected = &buffer[(gutter_offset, 1)];
        let unselected = &buffer[(gutter_offset + 1, 1)];
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
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
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
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
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
        // +1 width to account for the 1-cell right margin.
        let backend = TestBackend::new(9, 4);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.show_headings = false;
        app.show_files = false;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert_eq!(
            app.document_area.width, 9,
            "precondition: doc area is 9 wide"
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
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
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

    #[test]
    fn source_peek_renders_tab_indents_as_three_cells() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "x\ty").unwrap();
        let backend = TestBackend::new(40, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.source_peek = true;
        app.show_headings = false;
        app.show_files = false;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        // The tab between "x" and "y" must render as three cells of " ".
        let x_cell = &buffer[(app.document_area.x, app.document_area.y)];
        let tab_first = &buffer[(app.document_area.x + 1, app.document_area.y)];
        let tab_second = &buffer[(app.document_area.x + 2, app.document_area.y)];
        let tab_third = &buffer[(app.document_area.x + 3, app.document_area.y)];
        let y_cell = &buffer[(app.document_area.x + 4, app.document_area.y)];

        assert_eq!(x_cell.symbol(), "x");
        assert_eq!(tab_first.symbol(), " ");
        assert_eq!(tab_second.symbol(), " ");
        assert_eq!(tab_third.symbol(), " ");
        assert_eq!(y_cell.symbol(), "y");
    }

    #[test]
    fn top_ribbon_shows_a_mode_badge_for_write_and_source() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha").unwrap();
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.show_headings = false;
        app.show_files = false;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);
        assert!(
            rendered.contains(" WRITE "),
            "should show WRITE badge in rendered mode"
        );

        app.source_peek = true;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);
        assert!(
            rendered.contains(" SOURCE "),
            "should show SOURCE badge in source peek"
        );
    }

    #[test]
    fn tab_key_inserts_a_tab_that_renders_as_three_cells_in_source_peek() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "alpha").unwrap();
        let backend = TestBackend::new(80, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.source_peek = true;
        app.show_headings = false;
        app.show_files = false;
        // Move the cursor to the end of the existing text so the Tab key
        // inserts a tab between "alpha" and any subsequent text.
        app.editor.move_cursor_to_char_pos(app.editor.text().len());

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));

        // The editor should hold a single tab character followed by "b".
        assert_eq!(app.editor.text(), "alpha\tb");

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        // "alpha" occupies cells 0..5, the tab expands to 3 cells of " "
        // (5..8), and "b" lands at cell 8.
        let alpha_end = &buffer[(app.document_area.x + 4, app.document_area.y)];
        let tab_first = &buffer[(app.document_area.x + 5, app.document_area.y)];
        let tab_third = &buffer[(app.document_area.x + 7, app.document_area.y)];
        let b_cell = &buffer[(app.document_area.x + 8, app.document_area.y)];
        assert_eq!(alpha_end.symbol(), "a");
        assert_eq!(tab_first.symbol(), " ");
        assert_eq!(tab_third.symbol(), " ");
        assert_eq!(b_cell.symbol(), "b");
    }

    // ── Heading gutter rendering ─────────────────────────────────────

    #[test]
    fn heading_gutter_does_not_repeat_markers_on_continuation_row_at_scroll_origin() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        // Heading + body text long enough that at least 2 visual rows fit
        // before the end, so document_scroll = 1 is valid.
        std::fs::write(
            &path,
            "# Heading Long Title For Wrapping Test\n\n\
             Body paragraph text that also wraps nicely here.\n\n\
             Another paragraph for more scroll room.\n\n\
             Yet one more to ensure scroll works.\n",
        )
        .unwrap();
        let backend = TestBackend::new(20, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.show_headings = false;
        app.show_files = false;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        let visual = app.visual_document();
        assert!(
            visual.rows.len() >= 3,
            "need at least 3 visual rows; got {}",
            visual.rows.len()
        );

        let doc_x = app.document_area.x;
        let doc_y = app.document_area.y;

        // First visual row at scroll origin should have a gutter marker.
        let gutter_cell = &buffer[(doc_x, doc_y)];
        assert_eq!(gutter_cell.symbol(), "#");

        // Second visual row is a continuation — gutter must be blank.
        let cont_gutter = &buffer[(doc_x, doc_y + 1)];
        assert_eq!(
            cont_gutter.symbol(),
            " ",
            "continuation row gutter must be blank"
        );

        // Advance scroll so a continuation row is at the top of the viewport.
        let max_scroll = visual
            .rows
            .len()
            .saturating_sub(app.document_area.height.max(1) as usize);
        assert!(
            max_scroll >= 1,
            "need scroll room; max_scroll={max_scroll} rows={} height={}",
            visual.rows.len(),
            app.document_area.height,
        );
        app.document_scroll = 1;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        let cont_gutter_now_top = &buffer[(doc_x, doc_y)];
        assert_eq!(
            cont_gutter_now_top.symbol(),
            " ",
            "scrolled continuation row at top of viewport must still be blank"
        );
    }

    #[test]
    fn heading_gutter_shows_markers_for_heading_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Title\n\nBody text").unwrap();
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.show_headings = false;
        app.show_files = false;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        // The gutter is at column 0 in the document area. For a level-1
        // heading we should see a single '#' with the dim gutter style.
        let gutter_cell = &buffer[(app.document_area.x, app.document_area.y)];
        assert_eq!(gutter_cell.symbol(), "#");
        assert_eq!(gutter_cell.fg, theme::text_dim());

        // The blank separator follows at column 1.
        let blank = &buffer[(app.document_area.x + 1, app.document_area.y)];
        assert_eq!(blank.symbol(), " ");

        // The rendered heading text starts at column 2 (after gutter+blank).
        let text_cell = &buffer[(app.document_area.x + 2, app.document_area.y)];
        assert_eq!(text_cell.symbol(), "T");
    }

    #[test]
    fn heading_gutter_leaves_non_heading_rows_blank() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "Body text\n# Heading\nMore body").unwrap();
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.show_headings = false;
        app.show_files = false;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        // First body row: gutter cell at column 0 should be blank.
        let gutter_cell = &buffer[(app.document_area.x, app.document_area.y)];
        assert_eq!(gutter_cell.symbol(), " ");
    }

    #[test]
    fn heading_gutter_markers_adapt_to_deepest_heading() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "### Deep").unwrap();
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.show_headings = false;
        app.show_files = false;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        // Depth 3 → 3 gutter cells, all with '#'.
        let doc_x = app.document_area.x;
        let doc_y = app.document_area.y;
        assert_eq!(buffer[(doc_x, doc_y)].symbol(), "#");
        assert_eq!(buffer[(doc_x + 1, doc_y)].symbol(), "#");
        assert_eq!(buffer[(doc_x + 2, doc_y)].symbol(), "#");
        // Blank separator at column 3.
        assert_eq!(buffer[(doc_x + 3, doc_y)].symbol(), " ");
    }

    // ── Left separator blank column ──────────────────────────────────

    #[test]
    fn left_separator_is_a_blank_cell_between_browser_and_gutter() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Title\n\nBody text.").unwrap();
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        // The left separator sits between the headings panel right border
        // and the document area start. We assert it is blank.
        let left_sep_x = app.document_area.x - SIDEBAR_SEP_WIDTH;
        // Check every row of that column within the body area.
        let body_y = app.document_area.y;
        for row_off in 0..app.document_area.height {
            let cell = &buffer[(left_sep_x, body_y + row_off)];
            assert_eq!(
                cell.symbol(),
                " ",
                "left sep col {left_sep_x} row {} must be blank, got {:?}",
                body_y + row_off,
                cell.symbol(),
            );
        }
    }

    // ── Right margin ─────────────────────────────────────────────────

    #[test]
    fn right_margin_leaves_last_document_column_blank() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "short").unwrap();
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.show_headings = false;
        app.show_files = false;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        // The rightmost cell of the document area should be blank (margin).
        let last_col = app.document_area.x + app.document_area.width - 1;
        let last_cell = &buffer[(last_col, app.document_area.y)];
        assert_eq!(last_cell.symbol(), " ");
    }

    #[test]
    fn cursor_never_enters_right_margin() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        // Fill enough text to fill the line completely.
        std::fs::write(&path, "abcdefghijklmnop").unwrap();
        let backend = TestBackend::new(20, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.show_headings = false;
        app.show_files = false;

        // Move cursor to the last visible column in the text area and
        // verify it does not land in the right-margin column.
        app.editor.move_cursor_to_char_pos(0);
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let cursor = terminal.backend().cursor_position();
        // document_area.width includes the margin; cursor must be < doc_area.x + doc_area.width - 1
        assert!(cursor.x < app.document_area.x + app.document_area.width.saturating_sub(1));

        // Move cursor to the last source character. The cursor must be at
        // the last text cell, which is exactly one cell before the margin.
        app.editor.move_cursor_to_char_pos(app.editor.text().len());
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let end_cursor = terminal.backend().cursor_position();

        // The last text cell is at doc_area.x + text_x_offset + text_width - 1.
        // The margin cell at doc_area.x + doc_area.width - 1 must have no cursor.
        let layout = app.heading_gutter_layout();
        let margin_col = app.document_area.x + app.document_area.width - 1;
        assert!(
            end_cursor.x < margin_col,
            "cursor at end must be before the right-margin column"
        );
        // The cursor should land past the last non-margin text column.
        let text_end = app.document_area.x + layout.text_x_offset + layout.text_width;
        assert!(
            end_cursor.x <= text_end.saturating_sub(1),
            "cursor at end x={} beyond text_end={text_end}",
            end_cursor.x,
        );
    }

    // ── Unicode display-width truncation ─────────────────────────────

    #[test]
    fn truncate_to_display_width_uses_cell_width_not_char_count() {
        // CJK characters are 2 cells wide each.
        let s = "日本語text";
        // "日本語" = 6 cells, "text" = 4 cells → 10 cells total.
        // Truncate to 5 display-width cells should keep "日本" (4 cells) only.
        let result = truncate(s, 5);
        assert!(
            result.chars().count() <= 3,
            "CJK chars at 2 cells each mean fewer chars fit"
        );
        assert!(jones_text::grapheme_display_width(&result) <= 5);
    }

    #[test]
    fn truncate_to_display_width_empty_string() {
        assert_eq!(truncate("", 10), "");
    }

    #[test]
    fn truncate_to_display_width_zero_max() {
        assert_eq!(truncate("hello", 0), "");
    }

    // ── Rendered Markdown styling ────────────────────────────────────

    #[test]
    fn rendered_italic_text_produces_italic_modifier_cells() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        // Rendered output: "x italic y" with "italic" in ITALIC.
        std::fs::write(&path, "x *italic* y").unwrap();
        let backend = TestBackend::new(80, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.show_headings = false;
        app.show_files = false;
        // source_peek is already false for .md files — be explicit.
        assert!(!app.source_peek);

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        // Compute the text region: after the heading-marker gutter (if
        // any), before the right margin.  The right-margin column is the
        // last column of the document area, so exclude it.
        let layout = app.heading_gutter_layout();
        let text_start = app.document_area.x + layout.text_x_offset;
        let text_end = text_start + layout.text_width;

        // Gather every cell in the text region that carries ITALIC.
        let mut italic_cells: Vec<(u16, u16, String)> = Vec::new();
        for row in app.document_area.y..app.document_area.y + app.document_area.height {
            for col in text_start..text_end {
                let cell = &buffer[(col, row)];
                if cell.modifier.contains(Modifier::ITALIC) {
                    italic_cells.push((col, row, cell.symbol().to_string()));
                }
            }
        }

        assert!(
            !italic_cells.is_empty(),
            "Expected cells with ITALIC modifier for rendered *italic*"
        );
        let italic_text: String = italic_cells.iter().map(|(_, _, s)| s.as_str()).collect();
        assert_eq!(
            italic_text, "italic",
            "Italic-styled cells should form the word 'italic', got {italic_text:?}"
        );
        // The italic word should use the default text foreground (not a
        // heading color or any other special color).
        for (col, row, sym) in &italic_cells {
            let cell = &buffer[(*col, *row)];
            assert_eq!(
                cell.fg,
                theme::text_primary(),
                "Italic cell {sym:?} at ({col},{row}) should use text_primary"
            );
        }

        // ── Paired assertion: source_peek does not style source text ──
        app.source_peek = true;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        // In source-peek mode the source text "x *italic* y" is rendered
        // verbatim with a uniform style (no ITALIC modifier).
        let mut source_italic_cells: Vec<(u16, u16)> = Vec::new();
        for row in app.document_area.y..app.document_area.y + app.document_area.height {
            for col in text_start..text_end {
                let cell = &buffer[(col, row)];
                if cell.modifier.contains(Modifier::ITALIC) {
                    source_italic_cells.push((col, row));
                }
            }
        }
        assert!(
            source_italic_cells.is_empty(),
            "Source-peek mode should not apply ITALIC modifier, found {} cells: {source_italic_cells:?}",
            source_italic_cells.len(),
        );
    }

    // ── Scrollbar tests ───────────────────────────────────────────────

    #[test]
    fn scrollbar_renders_in_separator_column() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        // Generate enough lines to overflow a small viewport.
        let content = (0..50)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, &content).unwrap();
        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.show_headings = false;
        app.show_files = true;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        // The separator must be present when the files sidebar is visible.
        assert!(app.files_area.width > 0, "files sidebar should be visible");
        assert!(
            app.document_area.width > 0,
            "document area should be visible"
        );

        let sep_x = app.document_area.x + app.document_area.width;
        let doc_y = app.document_area.y;
        let doc_h = app.document_area.height;

        // Sample the separator column: should contain non-blank characters
        // (either track '│' or thumb '█').
        let mut non_blank_count = 0usize;
        for row_off in 0..doc_h {
            if let Some(cell) = buffer.cell((sep_x, doc_y + row_off))
                && cell.symbol() != " "
            {
                non_blank_count += 1;
            }
        }
        assert!(
            non_blank_count > 0,
            "separator column should have non-blank cells"
        );
    }

    #[test]
    fn scrollbar_thumb_present_when_content_overflows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        // Generate enough lines to overflow a small viewport.
        let content = (0..50)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, &content).unwrap();
        let backend = TestBackend::new(120, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.show_headings = false;
        app.show_files = true;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        let sep_x = app.document_area.x + app.document_area.width;
        let doc_y = app.document_area.y;
        let doc_h = app.document_area.height;

        // Count thumb ('█') cells. With 50 lines and a small viewport,
        // the thumb should occupy a visible proportion of the track.
        let thumb_cells: Vec<_> = (0..doc_h)
            .filter(|row_off| {
                buffer
                    .cell((sep_x, doc_y + row_off))
                    .is_some_and(|c| c.symbol() == "█")
            })
            .collect();
        assert!(
            !thumb_cells.is_empty(),
            "scrollbar should render thumb (█) cells when content overflows viewport"
        );
        // Thumb should not occupy the entire track (there should be some
        // track '│' cells too when content is much larger than viewport).
        assert!(
            thumb_cells.len() < doc_h as usize,
            "thumb should not fill the entire separator column"
        );
    }

    // ── Word progress in ribbon ─────────────────────────────────────

    #[test]
    fn ribbon_shows_cursor_word_progress() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "one two three four five").unwrap();
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        // Move cursor to the start of word "three" (char pos 8).
        app.editor.move_cursor_to_char_pos(8);
        app.show_headings = false;
        app.show_files = false;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);

        // Should show "2 / 5 words" since cursor is before the third word
        // (0-indexed: "one"=0, "two"=1, so before "three"=2).
        assert!(
            rendered.contains("2 / 5 words"),
            "ribbon should show cursor word progress, got: {rendered}"
        );
    }

    #[test]
    fn ribbon_shows_zero_for_empty_document() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "").unwrap();
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = WritermApp::with_config(Some(path), test_config()).unwrap();
        app.show_headings = false;
        app.show_files = false;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = rendered_buffer(&terminal);

        assert!(
            rendered.contains("0 / 0 words"),
            "empty doc should show 0 / 0 words, got: {rendered}"
        );
    }
}
