use jones_render::{RenderedDocument, RenderedLine};
use jones_text::grapheme_display_width;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use unicode_segmentation::UnicodeSegmentation;

/// Visual width of a single `\t` source character in the writerm document
/// surface. The editor's Tab key inserts a tab character; the virtual
/// document expands it to this many cells of whitespace so indent levels
/// line up consistently across the writing area regardless of the
/// underlying source text.
pub(crate) const TAB_WIDTH: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WrapMode {
    Source,
    Rendered,
}

#[derive(Debug, Clone)]
pub struct VisualDocument {
    pub rows: Vec<VisualRow>,
}

#[derive(Debug, Clone)]
pub struct VisualRow {
    spans: Vec<VisualSpan>,
    col_sources: Vec<usize>,
    source_start: usize,
    source_end: usize,
    mapped: bool,
    /// Number of leading display cells that are visual-only (synthetic
    /// indent).  `source_to_display` skips these cells so the cursor
    /// lands at the first text character rather than in the indent.
    /// `display_to_source` for `col < prefix_width` maps to
    /// `source_start`.  Zero when there is no synthetic indent.
    prefix_width: usize,
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
            let mut cells: Vec<Cell> = Vec::new();
            for grapheme in content.graphemes(true) {
                let grapheme_chars = grapheme.chars().count();
                let start = char_start + rel_chars;
                rel_chars += grapheme_chars;
                if grapheme == "\t" {
                    // A single source tab expands into TAB_WIDTH cells of
                    // whitespace. All cells map back to the same source
                    // character so the cursor can address any of them.
                    let tab_end = char_start + rel_chars;
                    for _ in 0..TAB_WIDTH {
                        cells.push(Cell {
                            text: " ".to_string(),
                            style,
                            source: Some((start, tab_end)),
                        });
                    }
                } else {
                    cells.push(Cell {
                        text: grapheme.to_string(),
                        style,
                        source: Some((start, char_start + rel_chars)),
                    });
                }
            }
            rows.extend(wrap_cells(
                cells,
                Some((char_start, char_start + line_len)),
                width,
                WrapMode::Source,
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
            let mut cells = rendered_line_cells(line);

            // Synthesize trailing-whitespace cells for source positions
            // that pulldown-cmark stripped from Text events but that the
            // paragraph source range still covers. Without these cells the
            // cursor cannot address those source positions.
            if let Some(src) = &line.source
                && !cells.is_empty()
            {
                let last_end = cells
                    .iter()
                    .rev()
                    .find_map(|c| c.source.map(|(_, end)| end));
                let need_synth = last_end.is_some_and(|le| le < src.char_end);
                if need_synth {
                    let le = last_end.unwrap();
                    for pos in le..src.char_end {
                        cells.push(Cell {
                            text: " ".to_string(),
                            style: Style::default(),
                            source: Some((pos, pos + 1)),
                        });
                    }
                }
            }

            rows.extend(wrap_cells(
                cells,
                line.source
                    .as_ref()
                    .map(|source| (source.char_start, source.char_end)),
                width,
                WrapMode::Rendered,
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

    /// How many leading visual-only cells this row has (the synthetic
    /// indent prefix).  Horizontal navigation treats these cells as a
    /// virtual boundary: pressing Left at `prefix_width` jumps to the
    /// previous row instead of entering the indent.
    pub fn row_prefix_width(&self, row: usize) -> Option<usize> {
        self.rows.get(row).map(|r| r.prefix_width)
    }

    /// Returns the `source_end` of the nearest previous mapped visual
    /// row IF that row is indented prose (has a synthetic prefix).
    /// Returns `None` when there is no previous mapped row, or when the
    /// previous row is structural (heading, list, code, blank line, or a
    /// wrapping continuation row).  Used by the Backspace handler to
    /// avoid atomically deleting across structural boundaries.
    pub fn prev_prose_row_end(&self, row: usize) -> Option<usize> {
        let prev = (0..row)
            .rev()
            .find(|&r| self.rows.get(r).is_some_and(|vr| vr.mapped))?;
        if self.rows[prev].prefix_width == 0 {
            return None;
        }
        Some(self.rows[prev].source_end)
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
            if char_pos < row.source_end {
                return Some((row_idx, row.col_for_source(char_pos)));
            }
            closest = Some((row_idx, row.width()));
        }
        closest
    }

    /// Apply a 2-cell visual indent prefix to the first visual row of
    /// each ordinary prose paragraph.  The indent is purely visual:
    /// `source_to_display` skips the prefix cells so the cursor lands at
    /// the first text character, while `display_to_source` (clicks) in
    /// the prefix region maps to the physical source-line start.
    ///
    /// Excluded: headings, lists, blockquotes, fenced code blocks,
    /// CommonMark indented code blocks (tab/≥4-space lines preceded by a
    /// blank line), thematic rules, blank lines, and wrapping
    /// continuation rows.
    ///
    /// Source lines that already have leading whitespace (tabs, spaces)
    /// still receive the uniform 2-cell prefix — the rendered-mode
    /// pipeline strips the source whitespace first, so every paragraph
    /// start looks consistent regardless of how the author indented the
    /// source.
    ///
    /// Line boundaries are precomputed once as byte ranges so the per-row
    /// cost is O(log lines).
    pub fn apply_first_line_indent(&mut self, source_text: &str) {
        let info = build_indent_line_info(source_text);
        if info.byte_bounds.is_empty() {
            return;
        }
        let mut last_source_line: Option<usize> = None;
        for row in &mut self.rows {
            if !row.mapped || row.source_start == row.source_end {
                last_source_line = None;
                continue;
            }
            let source_line = line_for_char_pos(&info, row.source_start);
            if Some(source_line) == last_source_line {
                continue;
            }
            last_source_line = Some(source_line);
            // Never indent content inside fenced code blocks.
            if info.in_fence.get(source_line).copied().unwrap_or(false) {
                continue;
            }
            // Never indent content inside CommonMark indented code blocks
            // (tab or ≥4-space prefixed lines following a blank line).
            if info
                .in_indented_code
                .get(source_line)
                .copied()
                .unwrap_or(false)
            {
                continue;
            }
            let line_text = slice_line(source_text, &info, source_line);
            if is_prose_paragraph(line_text) {
                // Use the physical source line start for the indent
                // mapping, NOT row.source_start.  Rendered-mode trimming
                // may have stripped leading whitespace cells, shifting
                // the row's source_start forward past the original line
                // start (e.g. past a tab character).  The indent must map
                // to the true first source character of the line.
                let line_char_start = info.char_starts[source_line];
                // Also ensure source_start encompasses the indent prefix
                // so source_to_display can find the indent's source
                // position (which is before the trimmed start).
                row.source_start = row.source_start.min(line_char_start);
                // Inherit the row's text style so the indent prefix
                // blends with the prose text rather than standing out.
                let text_style = row.spans.first().map(|s| s.style).unwrap_or_default();
                row.prepend_indent(2, line_char_start, text_style);
            }
        }
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
            source_start,
            source_end,
            mapped: true,
            prefix_width: 0,
        }
    }

    fn unmapped_blank() -> Self {
        Self {
            spans: Vec::new(),
            col_sources: Vec::new(),
            source_start: 0,
            source_end: 0,
            mapped: false,
            prefix_width: 0,
        }
    }

    fn from_cells(mut cells: Vec<Cell>, mode: WrapMode, fallback_source: Option<usize>) -> Self {
        if matches!(mode, WrapMode::Rendered) {
            // In rendered mode, trim leading whitespace (indentation artifacts)
            // but preserve trailing-whitespace-only rows (the wrapped row *is*
            // just whitespace and must be addressable for cursor navigation).
            if cells.iter().any(|c| !cell_is_whitespace(c)) {
                trim_leading_spaces(&mut cells);
            }
        }
        if cells.is_empty() {
            return fallback_source
                .map(Self::blank)
                .unwrap_or_else(|| Self::blank(0));
        }

        let first_source = cells
            .iter()
            .find_map(|cell| cell.source.map(|(start, _)| start));
        let last_source = cells
            .iter()
            .rev()
            .find_map(|cell| cell.source.map(|(_, end)| end));
        let source_start = first_source.or(fallback_source).unwrap_or(0);
        let source_end = last_source.or(fallback_source).unwrap_or(source_start);

        let fallback_source = first_source.or(fallback_source).unwrap_or(source_start);
        let mut spans = Vec::new();
        let mut col_sources = Vec::new();

        for cell in cells {
            push_cell_span(&mut spans, &cell.text, cell.style);
            let width = cell_width(&cell);
            if let Some((start, _end)) = cell.source {
                col_sources.extend(std::iter::repeat_n(start, width));
            } else {
                col_sources.extend(std::iter::repeat_n(fallback_source, width));
            }
        }

        Self {
            spans,
            col_sources,
            source_start,
            source_end,
            mapped: true,
            prefix_width: 0,
        }
    }

    fn include_source_start(&mut self, source: usize) {
        self.source_start = self.source_start.min(source);
    }

    fn include_source_end(&mut self, source: usize) {
        self.source_end = self.source_end.max(source);
    }

    pub(crate) fn to_line(&self) -> Line<'static> {
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
        let mut best_col = self.prefix_width;
        let mut prev_source = usize::MAX;
        for (col, &source) in self.col_sources.iter().enumerate().skip(self.prefix_width) {
            if source == char_pos {
                return col;
            }
            if source > char_pos {
                return best_col;
            }
            // source < char_pos: this cell starts before char_pos.
            // Update best_col only at the FIRST column of a new cell
            // (where source differs from the previous cell's source), to
            // match the old boundary-based logic for wide chars/tabs that
            // repeat the same source across multiple display columns.
            if source != prev_source {
                best_col = col;
                prev_source = source;
            }
        }
        best_col
    }

    /// Prepend `width` blank cells to the front of this row. Every new
    /// cell maps back to `source_pos` so display→source lookups
    /// (e.g. clicks) in the indent resolve correctly.  The cells are
    /// marked as a *non-addressable visual prefix*: `source_to_display`
    /// skips past them so the cursor lands at the first text character
    /// rather than inside the indent.
    ///
    /// `source_pos` should be the character offset of the first source
    /// character of the physical line, which may differ from the row's
    /// `source_start` when the rendered-mode pipeline has stripped
    /// leading whitespace (e.g. tab expansions).
    ///
    /// `style` is inherited from the row's first text span so the indent
    /// blends with the surrounding prose rather than using a bare default.
    fn prepend_indent(&mut self, width: usize, source_pos: usize, style: Style) {
        let prefix = " ".repeat(width);
        self.spans.insert(
            0,
            VisualSpan {
                content: prefix,
                style,
            },
        );
        for i in 0..width {
            self.col_sources.insert(i, source_pos);
        }
        self.prefix_width += width;
    }
}

fn wrap_cells(
    cells: Vec<Cell>,
    line_source: Option<(usize, usize)>,
    width: usize,
    mode: WrapMode,
) -> Vec<VisualRow> {
    if cells.is_empty() {
        return vec![
            line_source
                .map(|(start, end)| VisualRow::blank_range(start, end))
                .unwrap_or_else(VisualRow::unmapped_blank),
        ];
    }

    let mut wrapper = CellWrapper::new(width, mode, line_source.map(|(start, _)| start));
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
    mode: WrapMode,
    fallback_source: Option<usize>,
    rows: Vec<VisualRow>,
    current: Vec<Cell>,
    current_width: usize,
}

impl CellWrapper {
    fn new(width: usize, mode: WrapMode, fallback_source: Option<usize>) -> Self {
        Self {
            width,
            mode,
            fallback_source,
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
            match self.mode {
                WrapMode::Rendered => {
                    // Preserve the whitespace cell on the next row so the
                    // cursor can address it. Invisible when drawn.
                    self.flush_current();
                    self.push_unchecked(cell);
                }
                WrapMode::Source => {
                    // Standard text-editor wrapping: drop the wrap-boundary
                    // space so the wrapped row doesn't start with a blank
                    // cell. The cursor jumps the wrap in one keystroke.
                    self.flush_current();
                }
            }
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
            self.mode,
            self.fallback_source,
        ));
        self.current_width = 0;
    }

    fn recompute_width(&mut self) {
        self.current_width = self.current.iter().map(cell_width).sum();
    }

    fn finish(mut self) -> Vec<VisualRow> {
        if !self.current.is_empty() {
            self.rows.push(VisualRow::from_cells(
                self.current,
                self.mode,
                self.fallback_source,
            ));
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
            // A single source tab expands into TAB_WIDTH cells of
            // whitespace. The rendered path sees the tab the same way the
            // source-peek path does so cursor positions stay consistent
            // between the two views.
            if grapheme == "\t" {
                let Some(source) = span.source.as_ref() else {
                    // No source mapping: still need to advance rel_chars
                    // so subsequent spans stay in sync.
                    rel_chars += grapheme_len;
                    cells.push(Cell {
                        text: " ".repeat(TAB_WIDTH),
                        style: span.style,
                        source: None,
                    });
                    continue;
                };
                let start = (source.char_start + rel_chars).min(source.char_end);
                rel_chars += grapheme_len;
                let end = (source.char_start + rel_chars).min(source.char_end);
                for _ in 0..TAB_WIDTH {
                    cells.push(Cell {
                        text: " ".to_string(),
                        style: span.style,
                        source: Some((start, end)),
                    });
                }
                continue;
            }
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
    grapheme_display_width(text)
}

fn cell_is_whitespace(cell: &Cell) -> bool {
    cell.text.chars().all(char::is_whitespace)
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
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

// ── Paragraph indent helpers ──────────────────────────────────────────

/// Precomputed line boundaries used by `apply_first_line_indent`.
/// Built once per indent pass; per-row lookups are O(log lines).
struct IndentLineInfo {
    /// `(byte_start, byte_end)` for each physical line (excluding the
    /// trailing newline).
    byte_bounds: Vec<(usize, usize)>,
    /// Character offset of the start of each line, monotonically
    /// increasing.  Used for binary search to map a char position to
    /// a line index without a per-row char→byte scan.
    char_starts: Vec<usize>,
    /// `true` when this physical line is inside a fenced code block
    /// (between an opening `` ``` `` / ``~~~`` and its matching closing
    /// fence).  Content lines inside a fence never receive a prose indent.
    in_fence: Vec<bool>,
    /// `true` when this physical line is part of a CommonMark indented
    /// code block (starts with a tab or ≥4 spaces, preceded by a blank
    /// line or another indented-code-block line).  These lines must not
    /// receive a paragraph indent.
    in_indented_code: Vec<bool>,
}

fn build_indent_line_info(source_text: &str) -> IndentLineInfo {
    let mut byte_bounds = Vec::new();
    let mut char_starts = Vec::new();
    let mut char_cursor = 0usize;
    let mut byte_cursor = 0usize;
    let bytes = source_text.as_bytes();
    while byte_cursor < bytes.len() {
        let line_start = byte_cursor;
        char_starts.push(char_cursor);
        let line_end = bytes[byte_cursor..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|off| byte_cursor + off)
            .unwrap_or(bytes.len());
        // Normalise CRLF: if the byte before the newline is '\r',
        // exclude it from the content range.
        let content_end = if line_end > line_start && bytes[line_end - 1] == b'\r' {
            line_end - 1
        } else {
            line_end
        };
        let advance = if line_end < bytes.len() {
            line_end + 1 // skip '\n'
        } else {
            line_end
        };
        byte_bounds.push((line_start, content_end));
        let segment = &source_text[byte_cursor..advance];
        char_cursor += segment.chars().count();
        byte_cursor = advance;
    }
    if source_text.ends_with('\n') && !source_text.is_empty() {
        char_starts.push(char_cursor);
        byte_bounds.push((source_text.len(), source_text.len()));
    }

    // Second pass: mark lines that live inside fenced code blocks.
    let in_fence = mark_fence_lines(source_text, &byte_bounds);

    // Third pass: mark lines that are part of indented code blocks.
    let in_indented_code = mark_indented_code_lines(source_text, &byte_bounds);

    IndentLineInfo {
        byte_bounds,
        char_starts,
        in_fence,
        in_indented_code,
    }
}

/// Returns a `Vec<bool>` parallel to `byte_bounds` indicating which
/// physical lines reside inside a fenced code block.  Opening and closing
/// fence lines themselves are NOT marked (they are structural by nature),
/// but everything between them is.
///
/// A fence marker is a line whose trimmed content begins with at least
/// three backticks or three tildes.  Leading whitespace is permitted.
fn mark_fence_lines(source_text: &str, byte_bounds: &[(usize, usize)]) -> Vec<bool> {
    let mut result = vec![false; byte_bounds.len()];
    let mut fence_char: Option<char> = None;
    for (idx, &(start, end)) in byte_bounds.iter().enumerate() {
        let line = source_text.get(start..end).unwrap_or("");
        let trimmed = line.trim();
        if let Some(fc) = fence_char {
            // Inside a fence — is this the closing marker?
            if is_fence_marker(trimmed, fc) {
                fence_char = None;
            }
            // The opening marker line itself is at idx 0 inside this
            // block; content lines after the opener get marked.
            // We mark *every* line after the opener until the closer
            // is seen.  The opener line itself is NOT marked.
        } else {
            // Not inside a fence — check for an opening marker.
            if let Some(fc) = fence_char_from_line(trimmed) {
                fence_char = Some(fc);
            }
        }
        // Only mark content lines, not the opening fence line.
        if fence_char.is_some() {
            result[idx] = true;
        }
    }
    // Walk back: the opening fence line itself should not be marked.
    // We need to find each opening fence and clear its in_fence flag.
    let mut i = 0;
    while i < result.len() {
        if result[i] {
            // This is either the opening fence line (first line of this
            // block) or a content line.  The opening line was mistakenly
            // marked above.  Find the actual opening line and unmark it.
            let (start, end) = byte_bounds[i];
            let line = source_text.get(start..end).unwrap_or("");
            let trimmed = line.trim();
            if fence_char_from_line(trimmed).is_some() {
                // This is an opening fence line — unmark it.
                result[i] = false;
                // Skip past closing fence.
                let fence_char = fence_char_from_line(trimmed).unwrap();
                for j in (i + 1)..result.len() {
                    let (s, e) = byte_bounds[j];
                    let l = source_text.get(s..e).unwrap_or("");
                    if is_fence_marker(l.trim(), fence_char) {
                        result[j] = false;
                        i = j;
                        break;
                    }
                }
            }
        }
        i += 1;
    }
    result
}

/// Returns `Some(c)` if `trimmed_line` is a fenced-code-block marker
/// (at least three backticks or tildes), where `c` is the fence character.
fn fence_char_from_line(trimmed: &str) -> Option<char> {
    let first = trimmed.chars().next()?;
    if (first != '`' && first != '~') || trimmed.len() < 3 {
        return None;
    }
    // The line must start with at least 3 of the same character,
    // possibly followed by an info string.
    let count = trimmed.chars().take_while(|&c| c == first).count();
    if count >= 3 { Some(first) } else { None }
}

/// Returns `true` when `trimmed_line` is a closing fence matching `fence_char`.
fn is_fence_marker(trimmed: &str, fence_char: char) -> bool {
    fence_char_from_line(trimmed) == Some(fence_char)
}

/// Mark physical lines that are part of CommonMark **indented code
/// blocks**.  An indented code block starts on a line that begins with
/// a tab or at least four spaces, following a blank line (or at the
/// start of the document).  Once inside, consecutive tab/4+-space
/// lines continue the block.  A blank line ends it.
///
/// Prose continuation lines (e.g. the barrens fixture — tab-prefixed
/// lines within the same paragraph, separated only by a soft break)
/// are NOT marked because they are not preceded by a blank line.
fn mark_indented_code_lines(source_text: &str, byte_bounds: &[(usize, usize)]) -> Vec<bool> {
    let mut result = vec![false; byte_bounds.len()];
    let mut in_code: bool = false;
    let bytes = source_text.as_bytes();

    for (i, &(start, end)) in byte_bounds.iter().enumerate() {
        let content = &bytes[start..end];
        let is_blank = content.iter().all(|&b| b.is_ascii_whitespace());

        if is_blank {
            in_code = false;
            // Blank lines are never prose — leave `false`.
            continue;
        }

        let starts_with_indent = content.starts_with(b"\t")
            || (content.len() >= 4 && content[..4].iter().all(|&b| b == b' '));

        if starts_with_indent {
            if in_code {
                // Continuing an existing indented code block.
                result[i] = true;
            } else if i > 0 {
                // A tab/space-prefixed line is an indented code block
                // only when preceded by a blank line.  The first line of
                // the document (i == 0) cannot be preceded by a blank,
                // so we treat it as prose.
                let (ps, pe) = byte_bounds[i - 1];
                let prev_is_blank = bytes[ps..pe].iter().all(|&b| b.is_ascii_whitespace());
                if prev_is_blank {
                    in_code = true;
                    result[i] = true;
                }
                // Otherwise: tab/space-prefixed line following non-blank
                // prose → prose continuation, NOT a code block.
            }
            // i == 0 with indent: not a code block (no preceding blank).
        } else {
            // Non-blank, not indented → ends any indented code block.
            in_code = false;
        }
    }
    result
}

/// Binary search `info.char_starts` for the line whose content contains
/// `char_pos`.  Returns a valid line index.
fn line_for_char_pos(info: &IndentLineInfo, char_pos: usize) -> usize {
    // Find the last line whose char_start <= char_pos.
    let idx = info.char_starts.partition_point(|&cs| cs <= char_pos);
    idx.saturating_sub(1)
}

/// Return the source text for line index `line_idx`, using precomputed
/// byte bounds.
fn slice_line<'a>(source_text: &'a str, info: &IndentLineInfo, line_idx: usize) -> &'a str {
    let Some(&(start, end)) = info.byte_bounds.get(line_idx) else {
        return "";
    };
    source_text.get(start..end).unwrap_or("")
}

/// Returns `true` when `line` looks like an ordinary prose paragraph
/// that should receive a first-line visual indent. Structural lines
/// (headings, lists, blockquotes, fenced code, thematic rules, table
/// rows, blank lines) are excluded.
///
/// Lines that already have source-leading whitespace (tabs, spaces) are
/// still classified as prose: their leading whitespace is stripped during
/// `from_cells` in rendered mode and then replaced with a uniform visual
/// indent so every paragraph start looks consistent.
fn is_prose_paragraph(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Table rows (pipe-delimited).
    if trimmed.starts_with('|') {
        return false;
    }
    // ATX headings.
    if trimmed.starts_with('#') {
        return false;
    }
    // Unordered list bullet: '-', '*', '+'.
    if let Some(rest) = trimmed
        .strip_prefix('-')
        .or_else(|| trimmed.strip_prefix('*'))
        .or_else(|| trimmed.strip_prefix('+'))
        && (rest.is_empty() || rest.starts_with(' '))
    {
        return false;
    }
    // Ordered list: "1." / "99." / "1)" / "99)" style.
    if let Some(rest) = trimmed.strip_prefix(|c: char| c.is_ascii_digit()) {
        let after_digits = rest.trim_start_matches(|c: char| c.is_ascii_digit());
        if after_digits.starts_with(". ")
            || after_digits.starts_with(".)")
            || after_digits.starts_with(") ")
        {
            return false;
        }
    }
    // Blockquote.
    if trimmed.starts_with('>') {
        return false;
    }
    // Fenced code block markers.
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        return false;
    }
    // Thematic break: three or more of the same punctuation.
    if let Some(ch) = trimmed.chars().next()
        && matches!(ch, '-' | '*' | '_' | '=')
        && trimmed.chars().all(|c| c == ch || c == ' ')
        && trimmed.chars().filter(|&c| c == ch).count() >= 3
    {
        return false;
    }
    true
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
    fn rendered_visual_only_rows_use_line_source_as_anchor() {
        let rendered = render_markdown_mapped("\n---\nnext");
        let doc = VisualDocument::from_rendered(&rendered, 40);

        assert_eq!(doc.rows[0].to_line().to_string(), "");
        assert_eq!(doc.rows[1].to_line().to_string(), "─".repeat(32));
        assert_eq!(doc.rows[2].to_line().to_string(), "next");
        assert_eq!(doc.source_to_display(0), Some((0, 0)));
        assert_eq!(doc.source_to_display(1), Some((1, 0)));
        assert_eq!(doc.source_to_display(4), Some((1, 32)));
        assert_eq!(doc.display_to_source(1, 4), Some(1));
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
    fn trailing_rendered_whitespace_maps_to_own_cell() {
        let rendered = render_markdown_mapped("hello ");
        let doc = VisualDocument::from_rendered(&rendered, 20);

        assert_eq!(doc.source_to_display(6), Some((0, 6)));
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

    #[test]
    fn source_mode_expands_tabs_to_three_cells_of_whitespace() {
        let doc = VisualDocument::from_source("\tindented", 20, Style::default());

        // A single tab source character produces three " " display cells
        // followed by the 8 cells of "indented", for 11 cells total.
        assert_eq!(doc.row_width(0), Some(11));
        assert_eq!(doc.rows[0].to_line().to_string(), "   indented");
    }

    #[test]
    fn source_mode_maps_every_tab_cell_back_to_the_same_source_character() {
        let doc = VisualDocument::from_source("\tindented", 20, Style::default());

        // Each of the three cells produced by a single tab maps to source
        // position 0 (the tab itself). Subsequent cells map to positions
        // 1..9 covering "indented".
        for col in 0..3 {
            assert_eq!(
                doc.display_to_source(0, col),
                Some(0),
                "tab cell {col} should map back to the tab source char"
            );
        }
        assert_eq!(doc.display_to_source(0, 3), Some(1));
    }

    #[test]
    fn multiple_tabs_indent_consistently_in_source_mode() {
        let doc = VisualDocument::from_source("\t\talpha", 20, Style::default());

        // Two tab characters expand to six cells of whitespace followed
        // by the five cells of "alpha", for 11 cells total.
        assert_eq!(doc.row_width(0), Some(11));
        assert_eq!(doc.rows[0].to_line().to_string(), "      alpha");
        // The first three cells belong to the first tab, the next three
        // to the second tab, and the remaining cells to "alpha".
        assert_eq!(doc.display_to_source(0, 0), Some(0));
        assert_eq!(doc.display_to_source(0, 2), Some(0));
        assert_eq!(doc.display_to_source(0, 3), Some(1));
        assert_eq!(doc.display_to_source(0, 5), Some(1));
        assert_eq!(doc.display_to_source(0, 6), Some(2));
    }

    // ── Paragraph indent tests ────────────────────────────────────────

    #[test]
    fn indent_adds_two_cells_to_prose_paragraph_first_row() {
        let doc = VisualDocument::from_source("alpha beta gamma", 40, Style::default());
        let mut doc = doc; // make mutable
        doc.apply_first_line_indent("alpha beta gamma");

        // The single prose paragraph gets 2 cells of indent prepended.
        assert_eq!(doc.rows[0].to_line().to_string(), "  alpha beta gamma");
        assert_eq!(doc.row_width(0), Some(2 + "alpha beta gamma".len()));
    }

    #[test]
    fn indent_preserves_source_mapping_on_indented_row() {
        let doc = VisualDocument::from_source("hello world", 40, Style::default());
        let mut doc = doc;
        doc.apply_first_line_indent("hello world");

        // The two indent cells map to source_start (0) for clicks.
        assert_eq!(doc.display_to_source(0, 0), Some(0));
        assert_eq!(doc.display_to_source(0, 1), Some(0));
        // The third display cell is the start of the actual text.
        assert_eq!(doc.display_to_source(0, 2), Some(0)); // first char "h"
        // source_to_display skips the visual prefix, so source_start
        // (position 0) maps to the first text character at col 2.
        assert_eq!(doc.source_to_display(0), Some((0, 2)));
        // source_to_display at position 5 (" " after hello) should return
        // the cell just after "hello", shifted by 2 for the indent.
        assert_eq!(doc.source_to_display(5), Some((0, 7)));
    }

    #[test]
    fn indent_skips_headings() {
        let rendered = render_markdown_mapped("# Heading");
        let mut doc = VisualDocument::from_rendered(&rendered, 40);
        doc.apply_first_line_indent("# Heading");

        // Heading row should NOT get an indent.
        assert!(!doc.rows[0].to_line().to_string().starts_with("  Heading"));
    }

    #[test]
    fn indent_skips_list_items() {
        let rendered = render_markdown_mapped("- first item\n- second item");
        let mut doc = VisualDocument::from_rendered(&rendered, 40);
        doc.apply_first_line_indent("- first item\n- second item");

        // List items should NOT get indented.
        for row in &doc.rows {
            let text = row.to_line().to_string();
            if !text.trim().is_empty() {
                // Should start with the bullet (• or space), NOT "  ".
                assert!(
                    !text.starts_with("  •") && !text.starts_with("   "),
                    "list item should not be indented, got: {text:?}"
                );
            }
        }
    }

    #[test]
    fn indent_skips_blockquote() {
        let rendered = render_markdown_mapped("> quoted text");
        let mut doc = VisualDocument::from_rendered(&rendered, 40);
        doc.apply_first_line_indent("> quoted text");

        // Blockquote rows should NOT get a first-line indent (their
        // visual "│" prefix is their own structural indent).
        let first = doc.rows[0].to_line().to_string();
        assert!(
            !first.starts_with("  │"),
            "blockquote should not get indent, got: {first:?}"
        );
    }

    #[test]
    fn indent_skips_thematic_break() {
        let rendered = render_markdown_mapped("before\n\n---\n\nafter");
        let mut doc = VisualDocument::from_rendered(&rendered, 40);
        doc.apply_first_line_indent("before\n\n---\n\nafter");

        // The thematic break row (───) should not be indented.
        let hr_row = doc
            .rows
            .iter()
            .find(|r| r.to_line().to_string().contains('─'))
            .expect("should have a thematic break row");
        let text = hr_row.to_line().to_string();
        assert!(
            !text.starts_with("  ─"),
            "thematic break should not be indented, got: {text:?}"
        );
    }

    #[test]
    fn indent_skips_blank_lines() {
        let rendered = render_markdown_mapped("text\n\nmore");
        let mut doc = VisualDocument::from_rendered(&rendered, 40);
        doc.apply_first_line_indent("text\n\nmore");

        // Blank rows should have zero width (no indent).
        let blank_row = &doc.rows[1];
        assert_eq!(blank_row.to_line().to_string(), "");
    }

    #[test]
    fn indent_skips_continuation_wrapped_rows() {
        let doc =
            VisualDocument::from_source("alpha beta gamma delta epsilon", 12, Style::default());
        let mut doc = doc;
        doc.apply_first_line_indent("alpha beta gamma delta epsilon");

        // First row gets indent, continuation rows do not.
        assert!(
            doc.rows[0]
                .to_line()
                .to_string()
                .starts_with("  alpha beta")
        );
        assert!(!doc.rows[1].to_line().to_string().starts_with("  gamma"));
        assert!(!doc.rows[2].to_line().to_string().starts_with("  delta"));
    }

    #[test]
    fn indent_preserves_navigation_across_indented_and_unindented_rows() {
        let rendered = render_markdown_mapped("Paragraph one.\n\n# Heading\n\nParagraph two.");
        let mut doc = VisualDocument::from_rendered(&rendered, 40);
        doc.apply_first_line_indent("Paragraph one.\n\n# Heading\n\nParagraph two.");

        // "Paragraph one." — source pos 0 maps to col 2 (after the prefix),
        // because source_to_display skips the visual indent prefix.
        assert_eq!(doc.source_to_display(0), Some((0, 2)));
        // Clicks in the indent prefix (cols 0-1) still map to source_start.
        assert_eq!(doc.display_to_source(0, 0), Some(0));
        assert_eq!(doc.display_to_source(0, 1), Some(0));
        // The text area starts at col 2; clicking there still maps to
        // source pos 0.
        assert_eq!(doc.display_to_source(0, 2), Some(0));

        // "Heading" heading — no indent, source_to_display still works.
        let heading_source_start = "Paragraph one.\n\n# ".chars().count();
        let disp = doc.source_to_display(heading_source_start);
        assert!(disp.is_some(), "should find heading text");

        // "Paragraph two." — source_to_display finds it after the heading.
        let para2_start = "Paragraph one.\n\n# Heading\n\n".chars().count();
        let disp2 = doc.source_to_display(para2_start);
        assert!(disp2.is_some(), "should find second paragraph");
    }

    // ── is_prose_paragraph unit tests ─────────────────────────────────

    #[test]
    fn is_prose_paragraph_recognises_plain_text() {
        assert!(is_prose_paragraph("Hello world"));
        assert!(is_prose_paragraph("This is a sentence."));
        assert!(is_prose_paragraph("singleword"));
    }

    #[test]
    fn is_prose_paragraph_rejects_headings() {
        assert!(!is_prose_paragraph("# Heading"));
        assert!(!is_prose_paragraph("## Subheading"));
        assert!(!is_prose_paragraph("### Deep"));
    }

    #[test]
    fn is_prose_paragraph_rejects_list_markers() {
        assert!(!is_prose_paragraph("- bullet"));
        assert!(!is_prose_paragraph("* star"));
        assert!(!is_prose_paragraph("+ plus"));
        assert!(!is_prose_paragraph("1. ordered"));
        assert!(!is_prose_paragraph("99. many"));
        assert!(!is_prose_paragraph("1) paren"));
        assert!(!is_prose_paragraph("42) paren-style"));
        assert!(!is_prose_paragraph("7.) dot-paren"));
    }

    #[test]
    fn is_prose_paragraph_rejects_blockquote() {
        assert!(!is_prose_paragraph("> quoted"));
        assert!(!is_prose_paragraph(">"));
    }

    #[test]
    fn is_prose_paragraph_rejects_fenced_code() {
        assert!(!is_prose_paragraph("```"));
        assert!(!is_prose_paragraph("```rust"));
        assert!(!is_prose_paragraph("~~~"));
    }

    #[test]
    fn is_prose_paragraph_rejects_thematic_breaks() {
        assert!(!is_prose_paragraph("---"));
        assert!(!is_prose_paragraph("***"));
        assert!(!is_prose_paragraph("___"));
        assert!(!is_prose_paragraph("==="));
    }

    #[test]
    fn is_prose_paragraph_rejects_blank_and_whitespace() {
        assert!(!is_prose_paragraph(""));
        assert!(!is_prose_paragraph("   "));
        assert!(!is_prose_paragraph("\t"));
    }

    #[test]
    fn is_prose_paragraph_accepts_already_indented() {
        // Source-indented lines are still prose — the rendered-mode
        // visual pipeline strips and normalises the whitespace.
        assert!(is_prose_paragraph("    indented"));
        assert!(is_prose_paragraph("\tindented"));
    }

    #[test]
    fn is_prose_paragraph_rejects_table_rows() {
        assert!(!is_prose_paragraph("| Name | Age |"));
        assert!(!is_prose_paragraph("| Alice | 30 |"));
        assert!(!is_prose_paragraph("|---|---|"));
        assert!(!is_prose_paragraph("| single cell"));
    }

    // ── Fenced-code-block indent exclusion ────────────────────────────

    #[test]
    fn indent_skips_content_inside_backtick_fence() {
        let rendered = render_markdown_mapped(
            "Before.\n\n```\nfn main() {\n    println!(\"hi\");\n}\n```\n\nAfter.",
        );
        let mut doc = VisualDocument::from_rendered(&rendered, 40);
        doc.apply_first_line_indent(
            "Before.\n\n```\nfn main() {\n    println!(\"hi\");\n}\n```\n\nAfter.",
        );

        // "Before." gets an indent.
        assert!(doc.rows[0].to_line().to_string().starts_with("  Before"));

        // Find a row containing "fn main" — it must NOT be indented.
        let code_row = doc
            .rows
            .iter()
            .find(|r| r.to_line().to_string().contains("fn main"))
            .expect("should contain fn main");
        let text = code_row.to_line().to_string();
        assert!(
            !text.starts_with("  fn"),
            "code inside fence should not be indented, got: {text:?}"
        );

        // "After." (post-fence prose) gets an indent.
        let after_row = doc
            .rows
            .iter()
            .find(|r| r.to_line().to_string().contains("After"))
            .expect("should contain After");
        assert!(
            after_row.to_line().to_string().starts_with("  After"),
            "prose after fence should be indented"
        );
    }

    #[test]
    fn indent_skips_content_inside_tilde_fence() {
        let rendered = render_markdown_mapped("Intro.\n\n~~~\ncode\nmore code\n~~~\n\nOutro.");
        let mut doc = VisualDocument::from_rendered(&rendered, 40);
        doc.apply_first_line_indent("Intro.\n\n~~~\ncode\nmore code\n~~~\n\nOutro.");

        // "Intro." gets indent.
        assert!(doc.rows[0].to_line().to_string().starts_with("  Intro"));

        // Find a row containing "code" (inside the fence) — NOT indented.
        let code_rows: Vec<_> = doc
            .rows
            .iter()
            .filter(|r| {
                let t = r.to_line().to_string();
                t.contains("code") && !t.contains("Outro") && !t.contains("Intro")
            })
            .collect();
        assert!(!code_rows.is_empty(), "should have code rows inside fence");
        for row in &code_rows {
            let text = row.to_line().to_string();
            assert!(
                !text.starts_with("  code") && !text.starts_with("  more"),
                "tilde-fence code should not be indented, got: {text:?}"
            );
        }

        // "Outro." (post-fence prose) gets indent.
        let outro_row = doc
            .rows
            .iter()
            .find(|r| r.to_line().to_string().contains("Outro"))
            .expect("should contain Outro");
        assert!(
            outro_row.to_line().to_string().starts_with("  Outro"),
            "prose after tilde fence should be indented"
        );
    }

    // Regression: tab-indented paragraph indent (FIX VERIFICATION)
    // ═══════════════════════════════════════════════════════════════════

    /// Consecutive tab-prefixed source lines all show exactly one visual
    /// indent (2-space prefix).  Models the barrens-e2 fixture pattern.
    #[test]
    fn consecutive_tab_prefixed_lines_all_show_indent() {
        let input = "First paragraph.\n\tSecond tab-indented.\n\tThird tab-indented.";
        let rendered = render_markdown_mapped(input);
        let mut doc = VisualDocument::from_rendered(&rendered, 60);
        doc.apply_first_line_indent(input);

        // All three prose rows should start with a 2-space indent prefix.
        assert!(
            doc.rows[0].to_line().to_string().starts_with("  First"),
            "row 0 must have indent"
        );
        assert!(
            doc.rows[1].to_line().to_string().starts_with("  Second"),
            "row 1 must have indent"
        );
        assert!(
            doc.rows[2].to_line().to_string().starts_with("  Third"),
            "row 2 must have indent"
        );
        // Each row has prefix_width == 2.
        assert_eq!(doc.row_prefix_width(0), Some(2));
        assert_eq!(doc.row_prefix_width(1), Some(2));
        assert_eq!(doc.row_prefix_width(2), Some(2));
    }

    /// Space-prefixed lines also get uniform visual indent (normalised
    /// from whatever space count the source uses).
    #[test]
    fn space_prefixed_lines_get_normalised_indent() {
        let input = "First.\n    Second (4 spaces).\n  Third (2 spaces).";
        let rendered = render_markdown_mapped(input);
        let mut doc = VisualDocument::from_rendered(&rendered, 60);
        doc.apply_first_line_indent(input);

        // All three rows have exactly a 2-space prefix.
        assert!(doc.rows[0].to_line().to_string().starts_with("  First"));
        assert!(doc.rows[1].to_line().to_string().starts_with("  Second"));
        assert!(doc.rows[2].to_line().to_string().starts_with("  Third"));
        for i in 0..3 {
            assert_eq!(doc.row_prefix_width(i), Some(2));
        }
    }

    /// Blank-line-separated paragraphs each get one indent on the first
    /// visual row; no double-indent.
    #[test]
    fn blank_line_separated_paragraphs_each_get_one_indent() {
        let input = "Para one.\n\nPara two.\n\nPara three.";
        let rendered = render_markdown_mapped(input);
        let mut doc = VisualDocument::from_rendered(&rendered, 40);
        doc.apply_first_line_indent(input);

        // Find the three paragraph rows (skip the blank row).
        let texts: Vec<String> = doc
            .rows
            .iter()
            .map(|r| r.to_line().to_string())
            .filter(|t| !t.trim().is_empty())
            .collect();
        assert_eq!(texts.len(), 3, "should have 3 non-blank rows");
        for text in &texts {
            assert!(
                text.starts_with("  Para"),
                "paragraph should have single indent, got: {text:?}"
            );
            assert!(
                !text.starts_with("    Para"),
                "paragraph should not be double-indented, got: {text:?}"
            );
        }
    }

    /// `source_to_display` lands at the text-start column (prefix_width),
    /// not inside the indent gutter.
    #[test]
    fn cursor_source_to_display_lands_after_indent() {
        let input = "hello world";
        let doc = VisualDocument::from_source(input, 40, Style::default());
        let mut doc = doc;
        doc.apply_first_line_indent(input);

        // source pos 0 ("h") → col 2 (after the 2-cell prefix).
        assert_eq!(doc.source_to_display(0), Some((0, 2)));
        // source pos 1 ("e") → col 3.
        assert_eq!(doc.source_to_display(1), Some((0, 3)));
    }

    /// With the prefix, `display_to_source` on a prefix cell maps to
    /// `source_start` (for clicks), but `source_to_display` skips it.
    /// Left from text-start (col 2) should NOT get stuck — it moves to
    /// the previous source position, which remaps to col 2, but the app
    /// layer treats `col == prefix_width` as a row boundary.
    #[test]
    fn indent_prefix_navigation_does_not_trap() {
        let input = "hello world";
        let doc = VisualDocument::from_source(input, 40, Style::default());
        let mut doc = doc;
        doc.apply_first_line_indent(input);

        // Click in the prefix (col 0) maps to source_start.
        assert_eq!(doc.display_to_source(0, 0), Some(0));
        assert_eq!(doc.display_to_source(0, 1), Some(0));
        // Click at text-start (col 2) maps to the first char.
        assert_eq!(doc.display_to_source(0, 2), Some(0));
        // source_to_display for source 0 skips to text-start.
        assert_eq!(doc.source_to_display(0), Some((0, 2)));
        // prefix_width is exported for the app layer.
        assert_eq!(doc.row_prefix_width(0), Some(2));
    }

    /// Tab-indented source line: the text-start source position maps to
    /// col 2 (after the visual prefix), and the indent cells (cols 0-1)
    /// map to the tab's source position for clicks.
    #[test]
    fn tab_indented_cursor_positions() {
        // \tShe... → tab at pos 0, S at pos 1, h at pos 2, ...
        let input = "\tShe";
        let rendered = render_markdown_mapped(input);
        let mut doc = VisualDocument::from_rendered(&rendered, 40);
        doc.apply_first_line_indent(input);

        let text = doc.rows[0].to_line().to_string();
        assert!(
            text.starts_with("  She"),
            "expected indent + text, got: {text:?}"
        );

        // Prefix cells map to source_start (the tab, pos 0).
        assert_eq!(doc.display_to_source(0, 0), Some(0));
        assert_eq!(doc.display_to_source(0, 1), Some(0));
        // Text-start ("S") maps to source pos 1.
        assert_eq!(doc.display_to_source(0, 2), Some(1));
        // source_to_display for the tab (pos 0) skips prefix → col 2 (text-start).
        assert_eq!(doc.source_to_display(0), Some((0, 2)));
        // source_to_display for "S" (pos 1) → col 2.
        assert_eq!(doc.source_to_display(1), Some((0, 2)));
    }

    /// Wrapping: first row gets indent, continuation rows do not.
    #[test]
    fn wrapping_continuation_rows_not_indented() {
        let input = "alpha beta gamma delta epsilon";
        let doc = VisualDocument::from_source(input, 12, Style::default());
        let mut doc = doc;
        doc.apply_first_line_indent(input);

        // First row has indent.
        assert!(doc.rows[0].to_line().to_string().starts_with("  alpha"));
        // Continuation rows have no indent.
        let cont1 = doc.rows[1].to_line().to_string();
        let cont2 = doc.rows[2].to_line().to_string();
        assert!(
            !cont1.starts_with("  "),
            "continuation row should not be indented: {cont1:?}"
        );
        assert!(
            !cont2.starts_with("  "),
            "continuation row should not be indented: {cont2:?}"
        );
        // Continuation rows have prefix_width == 0.
        assert_eq!(doc.row_prefix_width(1), Some(0));
        assert_eq!(doc.row_prefix_width(2), Some(0));
    }

    /// Structural exclusions: headings, lists, blockquotes, fences, and
    /// thematic breaks still do not receive synthetic indent.
    #[test]
    fn structural_exclusions_still_work() {
        let input = "# Heading\n\n- list\n\n> blockquote\n\n```\ncode\n```\n\n---\n\nprose";
        let rendered = render_markdown_mapped(input);
        let mut doc = VisualDocument::from_rendered(&rendered, 40);
        doc.apply_first_line_indent(input);

        let texts: Vec<String> = doc.rows.iter().map(|r| r.to_line().to_string()).collect();

        // The last row is the prose paragraph and should be indented.
        let prose = &texts[texts.len() - 1];
        assert!(
            prose.starts_with("  prose"),
            "last prose row must be indented: {prose:?}"
        );

        // Heading row should NOT start with "  ".
        let heading = texts.iter().find(|t| t.contains("Heading")).unwrap();
        assert!(
            !heading.starts_with("  "),
            "heading should not be indented: {heading:?}"
        );

        // List item should NOT start with "  ".
        let list = texts.iter().find(|t| t.contains("list")).unwrap();
        assert!(
            !list.starts_with("  "),
            "list should not be indented: {list:?}"
        );

        // Code inside fence should NOT be indented.
        let code = texts.iter().find(|t| t.contains("code")).unwrap();
        assert!(
            !code.starts_with("  "),
            "code should not be indented: {code:?}"
        );
    }

    /// Paragraph indent does not affect source-peek mode (source mode
    /// doesn't call apply_first_line_indent).
    #[test]
    fn source_mode_unchanged_by_indent_logic() {
        let input = "\tindented";
        let doc = VisualDocument::from_source(input, 20, Style::default());

        // Source mode keeps the tab expansion (3 cells) as-is.
        let text = doc.rows[0].to_line().to_string();
        assert!(
            text.starts_with("   indented"),
            "source mode shows tab expansion: {text:?}"
        );
        // No prefix in source mode.
        assert_eq!(doc.row_prefix_width(0), Some(0));
    }

    // ═══════════════════════════════════════════════════════════════════
    // Indented code block exclusion
    // ═══════════════════════════════════════════════════════════════════

    /// A tab-prefixed line following a blank line is a CommonMark
    /// indented code block and must NOT receive paragraph indent.
    #[test]
    fn indented_code_block_after_blank_skips_indent() {
        let input = "prose\n\n\tcode line\n\tmore code\n\nmore prose";
        let rendered = render_markdown_mapped(input);
        let mut doc = VisualDocument::from_rendered(&rendered, 60);
        doc.apply_first_line_indent(input);

        let code_rows: Vec<String> = doc
            .rows
            .iter()
            .map(|r| r.to_line().to_string())
            .filter(|t| t.contains("code"))
            .collect();
        assert!(!code_rows.is_empty());
        for text in &code_rows {
            assert!(
                !text.starts_with("  "),
                "code should not be indented: {text:?}"
            );
        }
    }

    /// Four-space-indented code blocks (blank-line-separated) also skip
    /// paragraph indent.
    #[test]
    fn four_space_indented_code_block_skips_indent() {
        let input = "Text.\n\n    code line\n    more code\n\nMore text.";
        let rendered = render_markdown_mapped(input);
        let mut doc = VisualDocument::from_rendered(&rendered, 60);
        doc.apply_first_line_indent(input);

        for row in &doc.rows {
            let text = row.to_line().to_string();
            if text.contains("code") {
                assert!(!text.starts_with("  "), "4-space code: {text:?}");
            }
        }
    }

    /// A standalone tab-prefixed line (no preceding blank) is NOT an
    /// indented code block — it is prose and gets indent.
    #[test]
    fn standalone_tab_prefixed_line_is_prose() {
        let input = "\tShe";
        let rendered = render_markdown_mapped(input);
        let mut doc = VisualDocument::from_rendered(&rendered, 40);
        doc.apply_first_line_indent(input);

        assert!(doc.rows[0].to_line().to_string().starts_with("  She"));
        assert_eq!(doc.row_prefix_width(0), Some(2));
    }

    /// Tab-prefixed lines within a prose paragraph (soft breaks, no blank
    /// line) ARE prose continuations — the barrens-e2.md pattern: line 8
    /// is normal prose, lines 9-12 start with tabs within the same
    /// paragraph.
    #[test]
    fn barrens_pattern_tab_continuations_get_indent() {
        let input = "Finally, she had an idea.\n\tShe looked out.\n\tShe was, of course.";
        let rendered = render_markdown_mapped(input);
        let mut doc = VisualDocument::from_rendered(&rendered, 60);
        doc.apply_first_line_indent(input);

        let texts: Vec<String> = doc.rows.iter().map(|r| r.to_line().to_string()).collect();
        assert_eq!(texts.len(), 3);
        assert!(texts[0].starts_with("  Finally"));
        assert!(
            texts[1].starts_with("  She looked"),
            "tab continuation: {:?}",
            texts[1]
        );
        assert!(
            texts[2].starts_with("  She was,"),
            "tab continuation: {:?}",
            texts[2]
        );
    }
}
