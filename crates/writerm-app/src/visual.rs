use jones_render::{RenderedDocument, RenderedLine};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
pub struct VisualDocument {
    pub rows: Vec<VisualRow>,
}

#[derive(Debug, Clone)]
pub struct VisualRow {
    spans: Vec<VisualSpan>,
    col_sources: Vec<usize>,
    boundaries: Vec<(usize, usize)>,
    source_start: usize,
    source_end: usize,
    mapped: bool,
}

#[derive(Debug, Clone)]
struct VisualSpan {
    content: String,
    style: Style,
}

#[derive(Debug, Clone)]
struct Cell {
    text: String,
    style: Style,
    source: Option<(usize, usize)>,
}

impl VisualDocument {
    pub fn from_source(input: &str, width: usize, style: Style) -> Self {
        let width = width.max(1);
        let mut rows = Vec::new();
        let mut char_start = 0usize;

        for line in input.split_inclusive('\n') {
            let content = line.trim_end_matches('\n').trim_end_matches('\r');
            let line_len = content.chars().count();
            let mut rel_chars = 0usize;
            let cells = content
                .graphemes(true)
                .map(|grapheme| {
                    let start = char_start + rel_chars;
                    rel_chars += grapheme.chars().count();
                    Cell {
                        text: grapheme.to_string(),
                        style,
                        source: Some((start, char_start + rel_chars)),
                    }
                })
                .collect::<Vec<_>>();
            rows.extend(wrap_cells(
                cells,
                Some((char_start, char_start + line_len)),
                width,
                false,
            ));
            char_start += line.chars().count();
        }

        if input.is_empty() || input.ends_with('\n') {
            rows.push(VisualRow::blank(char_start));
        }

        Self { rows }
    }

    pub fn from_rendered(rendered: &RenderedDocument, width: usize) -> Self {
        let width = width.max(1);
        let mut rows = Vec::new();
        for line in &rendered.lines {
            rows.extend(wrap_cells(
                rendered_line_cells(line),
                line.source
                    .as_ref()
                    .map(|source| (source.char_start, source.char_end)),
                width,
                true,
            ));
        }
        if rows.is_empty() {
            rows.push(VisualRow::blank(0));
        }
        Self { rows }
    }

    pub fn to_text_with_selection(
        &self,
        scroll: usize,
        height: usize,
        selection: Option<(usize, usize)>,
        selection_style: Style,
    ) -> Text<'static> {
        Text::from(
            self.rows
                .iter()
                .skip(scroll)
                .take(height)
                .map(|row| row.to_line_with_selection(selection, selection_style))
                .collect::<Vec<_>>(),
        )
    }

    pub fn display_to_source(&self, row: usize, col: usize) -> Option<usize> {
        self.rows.get(row).and_then(|row| row.source_at_col(col))
    }

    pub fn row_width(&self, row: usize) -> Option<usize> {
        self.rows.get(row).map(VisualRow::width)
    }

    pub fn is_word_at_display_col(&self, row: usize, col: usize) -> bool {
        self.rows
            .get(row)
            .is_some_and(|row| row.is_word_at_display_col(col))
    }

    pub fn source_to_display(&self, char_pos: usize) -> Option<(usize, usize)> {
        let mut closest = None;
        for (row_idx, row) in self.rows.iter().enumerate() {
            if !row.mapped {
                continue;
            }
            if char_pos < row.source_start {
                continue;
            }
            if char_pos <= row.source_end {
                return Some((row_idx, row.col_for_source(char_pos)));
            }
            closest = Some((row_idx, row.width()));
        }
        closest
    }
}

impl VisualRow {
    fn blank(source: usize) -> Self {
        Self::blank_range(source, source)
    }

    fn blank_range(source_start: usize, source_end: usize) -> Self {
        Self {
            spans: Vec::new(),
            col_sources: Vec::new(),
            boundaries: vec![(source_start, 0), (source_end, 0)],
            source_start,
            source_end,
            mapped: true,
        }
    }

    fn unmapped_blank() -> Self {
        Self {
            spans: Vec::new(),
            col_sources: Vec::new(),
            boundaries: Vec::new(),
            source_start: 0,
            source_end: 0,
            mapped: false,
        }
    }

    fn from_cells(mut cells: Vec<Cell>, trim_edges: bool) -> Self {
        if trim_edges {
            trim_edge_spaces(&mut cells);
        }
        if cells.is_empty() {
            return Self::blank(0);
        }

        let first_source = cells
            .iter()
            .find_map(|cell| cell.source.map(|(start, _)| start));
        let last_source = cells
            .iter()
            .rev()
            .find_map(|cell| cell.source.map(|(_, end)| end));
        let source_start = first_source.unwrap_or(0);
        let source_end = last_source.unwrap_or(source_start);

        let fallback_source = first_source.unwrap_or(source_start);
        let mut spans = Vec::new();
        let mut col_sources = Vec::new();
        let mut boundaries = vec![(source_start, 0)];
        let mut col = 0usize;

        for cell in cells {
            push_cell_span(&mut spans, &cell.text, cell.style);
            let width = cell_width(&cell);
            if let Some((start, end)) = cell.source {
                boundaries.push((start, col));
                boundaries.push((end, col + width));
                col_sources.extend(std::iter::repeat_n(start, width));
            } else {
                col_sources.extend(std::iter::repeat_n(fallback_source, width));
            }
            col += width;
        }
        boundaries.push((source_end, col));
        boundaries.sort_unstable();
        boundaries.dedup();

        Self {
            spans,
            col_sources,
            boundaries,
            source_start,
            source_end,
            mapped: true,
        }
    }

    fn include_source_start(&mut self, source: usize) {
        self.source_start = self.source_start.min(source);
        self.boundaries.push((self.source_start, 0));
        self.boundaries.sort_unstable();
        self.boundaries.dedup();
    }

    fn include_source_end(&mut self, source: usize) {
        self.source_end = self.source_end.max(source);
        self.boundaries.push((self.source_end, self.width()));
        self.boundaries.sort_unstable();
        self.boundaries.dedup();
    }

    fn to_line(&self) -> Line<'static> {
        Line::from(
            self.spans
                .iter()
                .map(|span| Span::styled(span.content.clone(), span.style))
                .collect::<Vec<_>>(),
        )
    }

    fn to_line_with_selection(
        &self,
        selection: Option<(usize, usize)>,
        selection_style: Style,
    ) -> Line<'static> {
        let Some((selection_start, selection_end)) = selection else {
            return self.to_line();
        };
        if selection_start == selection_end {
            return self.to_line();
        }

        let mut spans = Vec::new();
        let mut display_col = 0usize;
        for visual_span in &self.spans {
            for grapheme in visual_span.content.graphemes(true) {
                let width = text_width(grapheme);
                let selected = self.cell_intersects_selection(
                    display_col,
                    width,
                    selection_start,
                    selection_end,
                );
                let style = if selected {
                    visual_span.style.patch(selection_style)
                } else {
                    visual_span.style
                };
                push_cell_span(&mut spans, grapheme, style);
                display_col += width;
            }
        }

        Line::from(
            spans
                .into_iter()
                .map(|span| Span::styled(span.content, span.style))
                .collect::<Vec<_>>(),
        )
    }

    fn cell_intersects_selection(
        &self,
        display_col: usize,
        width: usize,
        selection_start: usize,
        selection_end: usize,
    ) -> bool {
        if width == 0 {
            return false;
        }
        self.col_sources
            .iter()
            .skip(display_col)
            .take(width)
            .any(|source| (selection_start..selection_end).contains(source))
    }

    fn width(&self) -> usize {
        self.col_sources.len()
    }

    fn is_word_at_display_col(&self, col: usize) -> bool {
        self.grapheme_at_display_col(col)
            .is_some_and(|text| text.chars().any(is_word_char))
    }

    fn grapheme_at_display_col(&self, col: usize) -> Option<&str> {
        let mut display_col = 0usize;
        for span in &self.spans {
            for grapheme in span.content.graphemes(true) {
                let width = text_width(grapheme);
                if col < display_col + width {
                    return Some(grapheme);
                }
                display_col += width;
            }
        }
        None
    }

    fn source_at_col(&self, col: usize) -> Option<usize> {
        self.mapped.then(|| {
            if self.col_sources.is_empty() {
                self.source_start
            } else {
                self.col_sources
                    .get(col)
                    .copied()
                    .unwrap_or(self.source_end)
            }
        })
    }

    fn col_for_source(&self, char_pos: usize) -> usize {
        let mut best_col = 0usize;
        for (source, col) in &self.boundaries {
            if *source == char_pos {
                return *col;
            }
            if *source > char_pos {
                return best_col;
            }
            best_col = *col;
        }
        self.width()
    }
}

fn wrap_cells(
    cells: Vec<Cell>,
    line_source: Option<(usize, usize)>,
    width: usize,
    trim_edges: bool,
) -> Vec<VisualRow> {
    if cells.is_empty() {
        return vec![
            line_source
                .map(|(start, end)| VisualRow::blank_range(start, end))
                .unwrap_or_else(VisualRow::unmapped_blank),
        ];
    }

    let mut wrapper = CellWrapper::new(width, trim_edges);
    for cell in cells {
        wrapper.push(cell);
    }
    let mut rows = wrapper.finish();
    if let Some((start, end)) = line_source {
        if let Some(row) = rows.first_mut() {
            row.include_source_start(start);
        }
        if let Some(row) = rows.last_mut() {
            row.include_source_end(end);
        }
    }
    rows
}

struct CellWrapper {
    width: usize,
    trim_edges: bool,
    rows: Vec<VisualRow>,
    current: Vec<Cell>,
    current_width: usize,
}

impl CellWrapper {
    fn new(width: usize, trim_edges: bool) -> Self {
        Self {
            width,
            trim_edges,
            rows: Vec::new(),
            current: Vec::new(),
            current_width: 0,
        }
    }

    fn push(&mut self, cell: Cell) {
        let width = cell_width(&cell);
        if self.current_width + width <= self.width || self.current.is_empty() {
            self.push_unchecked(cell);
            return;
        }

        if cell_is_whitespace(&cell) {
            trim_trailing_spaces(&mut self.current);
            self.recompute_width();
            self.flush_current();
            return;
        }

        if let Some(space_idx) = self.current.iter().rposition(cell_is_whitespace)
            && space_idx > 0
        {
            let mut carry = self.current.split_off(space_idx + 1);
            self.current.pop();
            trim_trailing_spaces(&mut self.current);
            self.recompute_width();
            self.flush_current();
            trim_leading_spaces(&mut carry);
            self.current = carry;
            self.recompute_width();
            self.push(cell);
            return;
        }

        self.flush_current();
        self.push(cell);
    }

    fn push_unchecked(&mut self, cell: Cell) {
        self.current_width += cell_width(&cell);
        self.current.push(cell);
    }

    fn flush_current(&mut self) {
        if self.current.is_empty() {
            return;
        }
        self.rows.push(VisualRow::from_cells(
            std::mem::take(&mut self.current),
            self.trim_edges,
        ));
        self.current_width = 0;
    }

    fn recompute_width(&mut self) {
        self.current_width = self.current.iter().map(cell_width).sum();
    }

    fn finish(mut self) -> Vec<VisualRow> {
        if !self.current.is_empty() {
            self.rows
                .push(VisualRow::from_cells(self.current, self.trim_edges));
        }
        if self.rows.is_empty() {
            self.rows.push(VisualRow::unmapped_blank());
        }
        self.rows
    }
}

fn rendered_line_cells(line: &RenderedLine) -> Vec<Cell> {
    let mut cells = Vec::new();
    for span in &line.spans {
        let mut rel_chars = 0usize;
        for grapheme in span.content.graphemes(true) {
            let grapheme_len = grapheme.chars().count();
            cells.push(Cell {
                text: grapheme.to_string(),
                style: span.style,
                source: span.source.as_ref().map(|source| {
                    let start = (source.char_start + rel_chars).min(source.char_end);
                    rel_chars += grapheme_len;
                    let end = (source.char_start + rel_chars).min(source.char_end);
                    (start, end)
                }),
            });
            if span.source.is_none() {
                rel_chars += grapheme_len;
            }
        }
    }
    cells
}

fn push_cell_span(spans: &mut Vec<VisualSpan>, text: &str, style: Style) {
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.content.push_str(text);
        return;
    }
    spans.push(VisualSpan {
        content: text.to_string(),
        style,
    });
}

fn cell_width(cell: &Cell) -> usize {
    text_width(&cell.text)
}

fn text_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn cell_is_whitespace(cell: &Cell) -> bool {
    cell.text.chars().all(char::is_whitespace)
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn trim_edge_spaces(cells: &mut Vec<Cell>) {
    trim_leading_spaces(cells);
    trim_trailing_spaces(cells);
}

fn trim_leading_spaces(cells: &mut Vec<Cell>) {
    let keep_from = cells
        .iter()
        .position(|cell| !cell_is_whitespace(cell))
        .unwrap_or(cells.len());
    if keep_from > 0 {
        cells.drain(..keep_from);
    }
}

fn trim_trailing_spaces(cells: &mut Vec<Cell>) {
    while cells.last().is_some_and(cell_is_whitespace) {
        cells.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jones_render::render_markdown_mapped;
    use ratatui::style::Color;

    #[test]
    fn source_wraps_on_words_without_changing_source_positions() {
        let doc = VisualDocument::from_source("alpha beta gamma", 10, Style::default());

        assert_eq!(doc.rows.len(), 2);
        assert_eq!(doc.rows[0].to_line().to_string(), "alpha beta");
        assert_eq!(doc.rows[1].to_line().to_string(), "gamma");
        assert_eq!(doc.display_to_source(1, 0), Some(11));
        assert_eq!(doc.source_to_display(13), Some((1, 2)));
    }

    #[test]
    fn source_mode_preserves_leading_spaces_for_navigation() {
        let doc = VisualDocument::from_source("    indented", 20, Style::default());

        assert_eq!(doc.rows[0].to_line().to_string(), "    indented");
        assert_eq!(doc.display_to_source(0, 0), Some(0));
        assert_eq!(doc.source_to_display(4), Some((0, 4)));
    }

    #[test]
    fn source_selection_applies_selection_style_to_visible_range() {
        let doc = VisualDocument::from_source("alpha beta", 20, Style::default());
        let text = doc.to_text_with_selection(0, 1, Some((0, 5)), Style::default().bg(Color::Blue));

        assert_eq!(text.lines[0].spans[0].content, "alpha");
        assert_eq!(text.lines[0].spans[0].style.bg, Some(Color::Blue));
        assert_eq!(text.lines[0].spans[1].content, " beta");
        assert_eq!(text.lines[0].spans[1].style.bg, None);
    }

    #[test]
    fn source_mode_mapping_never_enters_combining_grapheme() {
        let doc = VisualDocument::from_source("xe\u{0301}y", 20, Style::default());

        assert_eq!(doc.display_to_source(0, 0), Some(0));
        assert_eq!(doc.display_to_source(0, 1), Some(1));
        assert_eq!(doc.display_to_source(0, 2), Some(3));
        assert_eq!(doc.source_to_display(1), Some((0, 1)));
        assert_eq!(doc.source_to_display(2), Some((0, 1)));
        assert_eq!(doc.source_to_display(3), Some((0, 2)));
    }

    #[test]
    fn source_mode_mapping_never_enters_zwj_emoji() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let doc = VisualDocument::from_source(family, 20, Style::default());
        let width = doc.row_width(0).unwrap();

        for col in 0..width {
            assert_eq!(doc.display_to_source(0, col), Some(0));
        }
        for char_pos in 1..5 {
            assert_eq!(doc.source_to_display(char_pos), Some((0, 0)));
        }
        assert_eq!(doc.source_to_display(5), Some((0, width)));
    }

    #[test]
    fn rendered_selection_highlights_visible_text_after_hidden_markers() {
        let rendered = render_markdown_mapped("# Heading");
        let doc = VisualDocument::from_rendered(&rendered, 20);
        let text = doc.to_text_with_selection(0, 1, Some((2, 5)), Style::default().bg(Color::Blue));

        assert_eq!(text.lines[0].spans[0].content, "Hea");
        assert_eq!(text.lines[0].spans[0].style.bg, Some(Color::Blue));
        assert_eq!(text.lines[0].spans[1].content, "ding");
        assert_eq!(text.lines[0].spans[1].style.bg, None);
    }

    #[test]
    fn rendered_mapping_never_enters_unicode_graphemes() {
        let rendered =
            render_markdown_mapped("xe\u{0301}y\n\n\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}");
        let doc = VisualDocument::from_rendered(&rendered, 20);

        assert_eq!(doc.display_to_source(0, 2), Some(3));
        assert_eq!(doc.source_to_display(2), Some((0, 1)));

        let emoji_row = doc.source_to_display(6).unwrap().0;
        for char_pos in 7..11 {
            assert_eq!(doc.source_to_display(char_pos), Some((emoji_row, 0)));
        }
    }

    #[test]
    fn rendered_blank_rows_preserve_full_source_range() {
        let rendered = RenderedDocument {
            lines: vec![RenderedLine {
                spans: Vec::new(),
                source: Some(jones_render::SourceRange {
                    byte_start: 0,
                    byte_end: 3,
                    char_start: 0,
                    char_end: 3,
                }),
            }],
        };
        let doc = VisualDocument::from_rendered(&rendered, 20);

        assert_eq!(doc.display_to_source(0, 0), Some(0));
        for char_pos in 0..=3 {
            assert_eq!(doc.source_to_display(char_pos), Some((0, 0)));
        }
    }

    #[test]
    fn wrapped_rendered_lines_keep_hidden_marker_mapping() {
        let rendered = render_markdown_mapped("# Alpha beta gamma");
        let doc = VisualDocument::from_rendered(&rendered, 10);

        assert_eq!(doc.source_to_display(0), Some((0, 0)));
        assert_eq!(doc.display_to_source(0, 0), Some(2));
        assert_eq!(doc.source_to_display(13), Some((1, 0)));
    }

    #[test]
    fn vertical_navigation_can_preserve_columns_across_short_rows() {
        let doc = VisualDocument::from_source("abcdefgh ij klmnopqr", 8, Style::default());

        assert_eq!(doc.source_to_display(6), Some((0, 6)));
        assert_eq!(doc.display_to_source(1, 6), Some(11));
        assert_eq!(doc.display_to_source(2, 6), Some(18));
    }

    #[test]
    fn trailing_rendered_whitespace_maps_to_previous_visible_row() {
        let rendered = render_markdown_mapped("hello ");
        let doc = VisualDocument::from_rendered(&rendered, 20);

        assert_eq!(doc.source_to_display(6), Some((0, 5)));
    }

    #[test]
    fn real_newline_after_text_maps_to_next_visual_row() {
        let rendered = render_markdown_mapped("hello\n");
        let doc = VisualDocument::from_rendered(&rendered, 20);

        assert_eq!(doc.source_to_display(6), Some((1, 0)));
    }

    #[test]
    fn incomplete_hidden_markdown_marker_stays_near_its_source_line() {
        let rendered = render_markdown_mapped("##\n\nnext");
        let doc = VisualDocument::from_rendered(&rendered, 20);

        assert_eq!(doc.source_to_display(2), Some((0, 0)));
    }
}
