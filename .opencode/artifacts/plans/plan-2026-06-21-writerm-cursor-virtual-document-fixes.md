# Writerm cursor & virtual document cursor-mapping fixes

**Date:** 2026-06-21
**Status:** draft
**Branch:** `fix/cursor` (already exists, has a broken attempted fix in the working tree)

---

## Goal

Fix the `writerm` "cursor stuck after typing a space" bug reported in
`.opencode/bug-reports/writerm-cursor-stuck-on-space.md`, plus the closely
related cursor-mapping defects discovered during investigation (End key on
trailing whitespace, wrap-boundary whitespace loss, source-mode wrapping
regression introduced by the current attempted fix). Replace the muddled
`trim_edges: bool` knob with a clear mode enum so the source (`source_peek`)
and rendered Markdown paths get wrapping behavior that is correct for each
mode. Add end-to-end render tests that assert the actual terminal cursor
cell, not just `source_to_display`, so this class of bug cannot recur
silently.

No code is changed in this plan-writing pass; this is the implementation
contract for the next agent.

## Understanding

### Where the cursor actually comes from

`writerm` is the only J-Suite app with an intermediate visual
representation. `termite` maps the rope straight to screen coordinates via
`jones_text::buffer_to_screen_pos` (`crates/termite-app/src/ui.rs:645`); `termex`
doesn't call `set_cursor_position` at all (it just highlights the cursor
line — `crates/termex-app/src/ui.rs:215`). Only `writerm` goes:

```
EditorContext::cursor_char_pos()                          // source char offset
  -> WritermApp::visual_document()                        // VisualDocument
  -> VisualDocument::source_to_display(char_pos)          // (row, col)
  -> draw::cursor_position(app, area, &visual)            // (x, y) in screen
  -> frame.set_cursor_position((x, y))                    // terminal cursor
```

So the cursor bug is entirely a `writerm-app` mapping defect. The editor
primitives in `jones-editor` (`EditorContext::cursor_char_pos` /
`move_cursor_to_char_pos`, `crates/jones-editor/src/editor.rs:153-167`) and
the grapheme-aware `EditorState::move_cursor` / `next_grapheme_boundary`
(`crates/jones-text/src/lib.rs:317`, `:599`) are correct — verified by the
bug report's investigation and re-confirmed during this planning pass.

### The visual pipeline

`WritermApp::visual_document` (`crates/writerm-app/src/app.rs:680`) produces
the visual document one of two ways depending on `source_peek`:

- `source_peek = true`: `VisualDocument::from_source(text, width, style)`
  (`crates/writerm-app/src/visual.rs:36`). Splits the input into lines,
  builds a `Cell` per grapheme (trailing spaces preserved — only `\n`/`\r`
  are trimmed at line `:42`), then `wrap_cells(cells, …, trim_edges=false)`.
- `source_peek = false` (default for `.md`): `VisualDocument::from_rendered(
  &rendered, width)` (`visual.rs:73`). Takes a `RenderedDocument` from
  `jones_render::render_markdown_mapped`, builds `Cell`s per grapheme of
  each span via `rendered_line_cells` (`visual.rs:478`), then
  `wrap_cells(cells, …, trim_edges=true)`.

`wrap_cells` (`visual.rs:356`) feeds cells to `CellWrapper` which flushes
rows of width ≤ `width`; on a wrap boundary it has a special branch for
whitespace cells (`visual.rs:414`). Each flushed row is built by
`VisualRow::from_cells` (`visual.rs:168`), which trims edges if
`trim_edges`, then builds three parallel structures:

- `spans: Vec<VisualSpan>` — what `to_line_with_selection` draws.
- `col_sources: Vec<usize>` — display col → source char (length = row width).
- `boundaries: Vec<(usize, usize)>` — sorted (source, col) pairs for
  `col_for_source` (the reverse lookup).

`VisualDocument::source_to_display` (`visual.rs:123`) finds the row whose
`source_start..=source_end` contains the char, then `col_for_source`; if no
row contains it, it falls back to `closest = (last_row, last_row.width())`.

`VisualDocument::display_to_source` (`visual.rs:109`) is
`row.col_sources.get(col).copied().unwrap_or(row.source_end)`.

### Root cause of the reported bug (Case 1)

`render_markdown_mapped("hello ")` produces a single line with one span:

```
span content: "hello"   span source: char 0..5
line source:  char 0..5  (AFTER trim_source_to_visible_span_bounds)
```

Verified directly against `pulldown-cmark`: the parser emits
`Text(Borrowed("hello")) at 0..5` — it strips the trailing space **before
the renderer ever sees it**, and the paragraph range `0..6` is then
collapsed back to `0..5` by `trim_source_to_visible_span_bounds`
(`crates/jones-render/src/markdown.rs:581`). So the rendered layer has no
knowledge that source position 5 is a space, and no cell is ever built for
it.

Result: `source_to_display(5)` (cursor right after `o`) and
`source_to_display(6)` (cursor right after the space the user just typed)
both return `Some((0, 5))` — the `closest = (row, row.width())` fallback.
`frame.set_cursor_position` is called with the same `(x, y)` before and
after the keystroke; the blinking cursor doesn't move. The character is
inserted correctly, the editor's `cursor_char_pos` is correct (6), only the
visual mapping is lossy.

### Root cause of Case 2 (trailing space at a wrap boundary)

`CellWrapper::push` (`visual.rs:407`) has a branch for "next cell is
whitespace and current row is full":

```rust
if cell_is_whitespace(&cell) {
    self.flush_current();
    self.push_unchecked(cell);   // working-tree attempt
    return;
}
```

The original code dropped the cell on the floor
(`trim_trailing_spaces` + `flush_current` + `return`). The working tree
keeps it but pushes it onto the next row for **both** modes, which breaks
source-mode wrapping (see "current attempted fix is broken" below). In
rendered mode, dropping the cell leaves its source position unmapped —
`source_to_display` falls through to the previous row's `width()`, so the
cursor lands at the end of the row above instead of on the wrapped row.

### Root cause of the End-key variant

`WritermApp::move_visual_line_boundary(true, …)` (`app.rs:516`) does
`visual.row_width(row)` then `display_to_source(row, width)`. With no cell
for the trailing space, `row_width` is 5 and `display_to_source(row, 5)`
returns `source_end = 5` — End lands on the space, not past it. The user
presses End and the cursor sits one cell short of the real end of line.

### Why the current attempted fix in the working tree is broken

`fix/cursor` has uncommitted changes to `visual.rs` and `draw.rs`. They:

1. Change `from_cells` to call `trim_leading_spaces` instead of
   `trim_edge_spaces` when `trim_edges` is set — **this part is correct**
   for rendered mode (trailing spaces are real content and must be
   addressable).
2. Change `CellWrapper::push` to `flush_current()` + `push_unchecked(cell)`
   for whitespace at a wrap boundary, unconditionally — **this is wrong**
   for source mode. Standard text-editor wrapping drops the wrap-boundary
   space so the wrapped row doesn't start with a blank cell.
3. Add three new tests + one render test, all of which **fail**:

```
visual::tests::trailing_rendered_whitespace_maps_to_previous_visible_row  // expects (0,6), gets (0,5)
visual::tests::trailing_rendered_whitespace_preserves_cell_in_visual_row  // expects width 6, gets 5
visual::tests::trailing_whitespace_at_wrap_boundary_is_preserved          // expects row 1, gets row 0
draw::tests::cursor_advances_after_typing_space_at_end_of_line            // expects x+1, gets x
app::tests::ctrl_m_remaps_current_cursor_and_scroll_without_losing_position // expects 23, gets 22
visual::tests::source_wraps_on_words_without_changing_source_positions      // expects "gamma", gets " gamma"
```

The first three fail because the synthesized trailing cell is never created
— pulldown-cmark already dropped the space upstream, so there's nothing in
the `Cell` stream for `from_cells` to preserve. The fourth fails for the
same reason. The last two fail because the unconditional
preserve-on-next-row behavior breaks source-mode wrapping: `"alpha beta
gamma"` at width 10 now wraps to `"alpha beta"` / `" gamma"` instead of
`"alpha beta"` / `"gamma"`, so `display_to_source(1, 1)` returns 22 (the
carried space) instead of 23 (the `g`).

So the working tree fix is both incomplete (doesn't fix the reported bug)
and regressive (breaks source mode). It should be replaced, not patched.

### Other findings worth noting

- `trim_edges: bool` is overloaded. It controls (a) whether `from_cells`
  trims leading spaces, (b) whether it trims trailing spaces, and (c) —
  after the working-tree change — whether wrap-boundary whitespace is
  preserved. These are independent decisions and should not share one knob.
- `trim_edge_spaces` (`visual.rs:531`) becomes dead code under the
  working-tree change; the compiler warns about it.
- `draw::cursor_position` (`draw.rs:212`) clamps `col` to
  `area.width.saturating_sub(1)`. This is correct for wrapping but means
  the cursor can be silently clamped when a row exactly fills the area.
  This is a pre-existing edge case, not the reported bug, and the
  synthesis fix doesn't make it worse (synthesized cells go through the
  same `wrap_cells` width check as any other cell).
- `boundaries` and `col_sources` are redundant (you can derive either from
  the other), but the redundancy is a deliberate O(1) vs O(log n) tradeoff
  for the two lookup directions. Keep both; don't redesign the data
  structure as part of this fix.
- `jones-render::RenderedDocument` also has its own
  `display_to_source` / `source_to_display` (`markdown.rs:74`, `:103`) that
  duplicate logic the visual layer re-derives. Not used on the cursor path
  for `writerm` (the visual layer's own mapping is what
  `draw::cursor_position` consults), so leave it alone.
- No existing test asserts the actual `TestBackend` cursor position for the
  spacebar case. The closest regression test
  (`cursor_after_space_stays_on_current_visual_row_in_rendered_mode`,
  `app.rs:1250`) only checks `source_to_display` and `document_scroll`,
  which is exactly the half of the story that already works.

## Approach

The fix is layered, bottom-up:

1. **Renderer**: stop collapsing the line source range down to the last
   visible span's source end. The paragraph range from `pulldown-cmark`
   already covers the trailing whitespace; `trim_source_to_visible_span_bounds`
   is the only thing throwing that information away. Keep the start-side
   trim (it's a defensive clamp, never actually shrinks in practice) but
   stop trimming `char_end` / `byte_end`.

2. **Visual document**: in `from_rendered`, after building cells from the
   rendered spans, synthesize a whitespace `Cell` for every source char in
   the gap `[last_cell.source.end .. line_source.char_end)`. Each
   synthesized cell gets `text: " "` (renders as a blank cell — invisible),
   the line's current style, and a real `source: (pos, pos+1)`. These then
   flow through `wrap_cells` normally, so a trailing space that doesn't fit
   wraps to the next row just like any other cell.

3. **Wrap mode**: replace `trim_edges: bool` with a `WrapMode` enum
   (`Source` / `Rendered`). `CellWrapper::push`'s wrap-boundary whitespace
   branch becomes mode-sensitive: `Rendered` preserves the whitespace cell
   on the next row (so the cursor can address it); `Source` drops it
   (standard text-editor wrapping — the existing
   `source_wraps_on_words_without_changing_source_positions` and
   `ctrl_m_remaps_…` tests encode this contract). `from_cells` only trims
   **leading** spaces when `mode == Rendered` (trailing spaces are real
   content); `Source` mode trims nothing, as today.

4. **Tests**: update the two existing tests that assert the old broken
   `(0, 5)` mapping to assert the correct `(0, 6)` mapping. Keep the
   working tree's new `cursor_advances_after_typing_space_at_end_of_line`
   render test (it's the right end-to-end assertion, it just needs the
   engine fix to pass). Add a `TestBackend::assert_cursor_position`-based
   test for the End-key variant and one for the wrap-boundary case. Add a
   `visual.rs` unit test that asserts the synthesized cell exists.

5. **Clean up**: delete `trim_edge_spaces` (dead under the new design) and
   the stale `trim_edges` call sites.

### Key design decisions and tradeoffs

- **Why synthesize cells in `from_rendered` rather than in the renderer?**
  Doing it in the renderer would mean emitting span content like `"hello "`
  (with the trailing space) so the existing `rendered_line_cells` would
  naturally produce a cell. That changes the rendered `Text` output for
  every consumer of `render_markdown_mapped`, not just `writerm`, and
  could subtly affect wrapping/width calculations elsewhere. Synthesizing
  in `from_rendered` localizes the change to the cursor path and leaves
  the renderer's output contract (no trailing whitespace in span content)
  intact.

- **Why preserve wrap-boundary whitespace in rendered mode but drop it in
  source mode?** Source mode is a plain text editor — standard wrapping
  drops the wrap-boundary space so wrapped rows don't start with a blank
  cell, and the cursor "jumps" the wrap boundary in one keystroke. That's
  what the existing source-mode tests encode and what users expect from a
  text editor. Rendered mode is a WYSIWYG markdown view where every source
  char the user can place a cursor on should be addressable; carrying the
  space onto the next row as an invisible blank cell makes Right/Left
  navigation land on each source position faithfully. The wrapped row
  starts with an invisible cell, which is visually indistinguishable from
  not having it.

- **Why a `WrapMode` enum instead of two bools?** The three behaviors
  (leading trim, trailing trim, wrap-boundary preservation) are not
  independent — they're facets of "which mode are we in." A single enum
  makes illegal states unrepresentable (no "trim trailing but drop
  wrap-boundary" combination) and makes the call sites self-documenting.

- **Why not redesign `VisualRow` to drop `boundaries` or `col_sources`?**
  The dual representation is a deliberate O(1) vs O(log n) tradeoff for
  the two lookup directions, and the cell-construction bugs are
  independent of the data structure. A redesign would be a much larger
  change with its own regression risk, and the user asked for the cursor
  bugs fixed, not a refactor. Note it as a future direction only.

## Steps

### Phase 1: Revert the broken working-tree attempt and establish the baseline

1. **Revert the uncommitted changes in `visual.rs` and `draw.rs`**
   - **Location:** working tree on `fix/cursor`
   - **Action:** `git checkout -- crates/writerm-app/src/visual.rs
     crates/writerm-app/src/draw.rs`. This discards the incomplete
     `from_cells`/`CellWrapper::push` change and the four failing tests,
     restoring the clean `06efbc6` state. The bug report stays in
     `.opencode/bug-reports/`; it's not part of the revert.
   - **Verification:** `cargo test -p writerm-app --lib` is green again
     (75/75, the pre-attempt baseline). `git diff` is empty.

### Phase 2: Stop the renderer from dropping trailing-whitespace source ranges

2. **Stop trimming the line source end in
   `trim_source_to_visible_span_bounds`**
   - **Location:** `crates/jones-render/src/markdown.rs:581`
   - **Action:** Change the function so it only clamps the **start** side
     (or remove it entirely if the start clamp also turns out to be a
     no-op in practice — check with the tests first). Concretely: delete
     the two lines that do `source.byte_end = source.byte_end.min(
     last_span.byte_end)` and `source.char_end = source.char_end.min(
     last_span.char_end)`. Keep the `byte_start`/`char_start` clamp lines
     unless the tests show they're also unused, in which case delete the
     whole function and its call site at `:561`.
   - **Rationale:** The paragraph range `0..6` from `pulldown-cmark` is
     the authoritative "what source does this line cover" range. Collapsing
     it to `0..5` because the last visible span ends at 5 is exactly what
     erases the trailing space from the mapping. The visual layer will
     use the preserved `char_end` to synthesize a cell in Phase 3.
   - **Verification:** `cargo test -p jones-render` still green. Add a
     unit test in `crates/jones-render/src/markdown.rs::tests` (new test
     `rendered_line_source_covers_trailing_whitespace`) that renders
     `"hello "` and asserts `lines[0].source.unwrap().char_end == 6` and
     `lines[0].spans[0].content == "hello"` (the span content is still
     trimmed — only the line source range is preserved). Also test
     `"hello   "` → `char_end == 8`. Run `cargo test -p jones-render`.

### Phase 3: Synthesize trailing-whitespace cells in the visual layer

3. **Add `WrapMode` enum and replace `trim_edges: bool`**
   - **Location:** `crates/writerm-app/src/visual.rs` — the `wrap_cells`
     signature (`:356`), `CellWrapper::new` (`:396`), `CellWrapper` struct
     (`:386`), `CellWrapper::push` (`:407`), `VisualRow::from_cells`
     (`:168`), and the two call sites in `VisualDocument::from_source`
     (`:57`) and `from_rendered` (`:77`).
   - **Action:**
     - Add `enum WrapMode { Source, Rendered }` (pub(crate) is fine).
     - Replace every `trim_edges: bool` parameter with `mode: WrapMode`.
     - In `VisualRow::from_cells`, replace `if trim_edges { trim_leading_spaces(
       &mut cells); }` with `if matches!(mode, WrapMode::Rendered) {
       trim_leading_spaces(&mut cells); }` — same behavior as the
       working-tree attempt, just named clearly. **Trailing spaces are
       never trimmed in either mode** (source mode never trimmed them;
       rendered mode now keeps them because they're real content).
     - In `CellWrapper::push` (`:414`), replace the whitespace branch with:
       ```rust
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
       ```
     - Delete `trim_edge_spaces` (`:531`) — no longer called anywhere.
   - **Verification:** `cargo test -p writerm-app --lib` — the
     source-mode tests (`source_wraps_on_words_without_changing_source_positions`,
     `ctrl_m_remaps_current_cursor_and_scroll_without_losing_position`)
     must still pass unchanged because `WrapMode::Source` reproduces the
     original drop-at-boundary behavior. The rendered-mode tests still
     pass because the synthesis step (next) hasn't been added yet — they
     were passing against the original broken mapping. `cargo clippy -p
     writerm-app --all-targets -- -D warnings` clean (no dead-code warning
     for `trim_edge_spaces`).

4. **Synthesize trailing-whitespace cells in `from_rendered`**
   - **Location:** `crates/writerm-app/src/visual.rs:73`
     (`VisualDocument::from_rendered`).
   - **Action:** After `let mut cells = rendered_line_cells(line);` and
     before `rows.extend(wrap_cells(cells, …, WrapMode::Rendered))`, if
     `line.source` is `Some(src)`, find the last cell's source end
     (`cells.iter().rev().find_map(|c| c.source.map(|(_, end)| end))`) and
     if it's less than `src.char_end`, push one synthesized `Cell` per
     missing char position:
      ```rust
      if let Some(src) = &line.source {
          let last_end = cells.iter().rev()
              .find_map(|c| c.source.map(|(_, end)| end));
          if let Some(last_end) = last_end {
              for pos in last_end..src.char_end {
                  cells.push(Cell {
                      text: " ".to_string(),
                      style: Style::default(),
                      source: Some((pos, pos + 1)),
                  });
              }
          }
      }
      ```
      For `style`: use `Style::default()` (the space is invisible, style
      doesn't matter for rendering, and `col_sources` only cares about the
      `source` field). If a non-default style turns out to matter for
      selection background, use the line's first span style or
      `theme::text_primary()` — verify with the selection render test in
      Phase 5. The snippet above uses `Style::default()` directly.
   - **Rationale:** The gap `[last_end .. src.char_end)` is exactly the
     trailing whitespace `pulldown-cmark` stripped. Each char in the gap
     is by construction whitespace (if it weren't, `pulldown-cmark` would
     have emitted it as a `Text` event). Synthesizing one cell per
     position gives the cursor a real addressable cell at every source
     position the editor can place a cursor on. The cells then flow
     through `wrap_cells` so a trailing space that doesn't fit the width
     wraps to the next row naturally — no special-casing in `CellWrapper`.
   - **Edge case — `cells` is empty but `line.source` is `Some`** (e.g. a
     blank line that `assign_blank_line_sources` produced): `last_end` is
     `None`, skip the synthesis — the existing `blank_range` path already
     handles this. Add an explicit `if cells.is_empty() { skip }` guard so
     the `find_map` returning `None` is clearly handled.
    - **Verification:** New unit test `trailing_rendered_whitespace_gets_cell`
      in `visual.rs::tests`: `render_markdown_mapped("hello ")` at width 20,
      assert `doc.rows[0].width() == 6`, `doc.rows[0].col_sources == vec![0,1,2,3,4,5]`,
      `doc.source_to_display(5) == Some((0, 5))` (the space),
      `doc.source_to_display(6) == Some((0, 6))` (after the space),
      `doc.display_to_source(0, 5) == Some(5)`. Also
     `trailing_rendered_whitespace_multi_space`: `"hello   "` (3 spaces) →
     `width() == 8`, `source_to_display(8) == Some((0, 8))`. Also
     `trailing_rendered_whitespace_wraps_when_too_wide`: `"hello "` at
     width 5 → row 0 is `"hello"`, row 1 is `" "` (the synthesized space
     wrapped), `source_to_display(5) == Some((1, 0))`,
     `source_to_display(6) == Some((1, 1))`. Run `cargo test -p
     writerm-app --lib visual::tests`.

### Phase 4: Update existing tests that encoded the broken mapping

5. **Update `trailing_rendered_whitespace_maps_to_previous_visible_row`**
   - **Location:** `crates/writerm-app/src/visual.rs:698`
   - **Action:** Change the assertion from
     `assert_eq!(doc.source_to_display(6), Some((0, 5)))` to
     `assert_eq!(doc.source_to_display(6), Some((0, 6)))`. The test name
     is now a misnomer (the cursor is no longer on the "previous visible
     row" — it's on the same row, one cell past the space); rename it to
     `trailing_rendered_whitespace_maps_to_own_cell` for honesty.
   - **Verification:** `cargo test -p writerm-app --lib visual::tests
     trailing_rendered_whitespace_maps_to_own_cell` passes.

6. **Update `cursor_after_space_stays_on_current_visual_row_in_rendered_mode`**
   - **Location:** `crates/writerm-app/src/app.rs:1250`
   - **Action:** Change
     `assert_eq!(app.visual_document().source_to_display(6), Some((0, 5)))`
     to `Some((0, 6))`. The test name is still accurate (the cursor stays
     on row 0); only the column expectation changes.
   - **Verification:** `cargo test -p writerm-app --lib
     app::tests::cursor_after_space_stays_on_current_visual_row_in_rendered_mode`
     passes.

### Phase 5: Add end-to-end render tests that assert the actual terminal cursor

7. **Re-add and fix the `cursor_advances_after_typing_space_at_end_of_line`
   render test**
   - **Location:** `crates/writerm-app/src/draw.rs` (in `#[cfg(test)] mod
     tests`, after `source_peek_shift_selection_uses_selection_background`).
   - **Action:** Re-add the test that was reverted in Phase 1. Use the
     ratatui 0.30 `TestBackend::cursor_position(&self) -> Position`
     const accessor (NOT the `Backend::get_cursor_position(&mut self)`
     trait method — that needs `&mut` and returns `Result`):
     ```rust
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

         assert_eq!(after.x, before.x + 1, "cursor should advance one cell after space");
         assert_eq!(after.y, before.y, "cursor should stay on the same row");
     }
     ```
     The document area for an 80×8 terminal with both sidebars off is
     wide enough that `"hello "` (6 cells) doesn't wrap, so the cursor
     moves from col 5 to col 6 on row 1 (row 0 is the top ribbon).
   - **Verification:** `cargo test -p writerm-app --lib
     draw::tests::cursor_advances_after_typing_space_at_end_of_line`
     passes.

8. **Add an End-key render test**
   - **Location:** `crates/writerm-app/src/draw.rs` tests, next to the
     above.
   - **Action:**
     ```rust
     #[test]
     fn end_key_on_line_with_trailing_whitespace_lands_past_the_space() {
         let dir = TempDir::new().unwrap();
         let path = dir.path().join("note.md");
         std::fs::write(&path, "hello ").unwrap();  // trailing space
         let backend = TestBackend::new(80, 8);
         let mut terminal = Terminal::new(backend).unwrap();
         let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();
         app.show_headings = false;
         app.show_files = false;
         app.editor.move_cursor_to_char_pos(0);

         app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
         terminal.draw(|frame| draw(frame, &mut app)).unwrap();

         assert_eq!(app.editor.cursor_char_pos(), 6, "End should reach past the trailing space");
         let cursor = terminal.backend().cursor_position();
         // "hello " is 6 cells; cursor at col 6 (one past the space) on the document row.
         assert_eq!(cursor.x, app.document_area.x as u16 + 6);
     }
     ```
   - **Verification:** test passes. Before the fix it would fail with
     `cursor_char_pos() == 5`.

9. **Add a wrap-boundary render test (Case 2)**
   - **Location:** `crates/writerm-app/src/draw.rs` tests.
   - **Action:** Use an 8-wide terminal so `draw` naturally produces an
     8-wide document area (both sidebars off). Do NOT set
     `document_area` manually — `draw` overwrites it. Draw once first to
     populate `document_area`, then drive the keystroke, then draw again:
     ```rust
     #[test]
     fn cursor_moves_to_wrapped_row_after_typing_at_trailing_space_wrap_boundary() {
         let dir = TempDir::new().unwrap();
         let path = dir.path().join("note.md");
         std::fs::write(&path, "abcdefgh").unwrap();
         let backend = TestBackend::new(8, 4);  // 8 wide → document width 8
         let mut terminal = Terminal::new(backend).unwrap();
         let mut app = WritermApp::with_config(Some(path), Config::default()).unwrap();
         app.show_headings = false;
         app.show_files = false;

         // First draw populates app.document_area from the terminal size.
         terminal.draw(|frame| draw(frame, &mut app)).unwrap();
         assert_eq!(app.document_area.width, 8, "precondition: doc area is 8 wide");

         app.editor.move_cursor_to_char_pos(8);  // end of "abcdefgh"
         app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
         terminal.draw(|frame| draw(frame, &mut app)).unwrap();

         assert_eq!(app.editor.cursor_char_pos(), 9);
         // The synthesized space wraps to visual row 1.
         assert_eq!(app.visual_document().source_to_display(8), Some((1, 0)));
         assert_eq!(app.visual_document().source_to_display(9), Some((1, 1)));
         // Screen coords: x = document_area.x + 1, y = document_area.y + 1.
         // document_area is Rect{ x: 0, y: 1, width: 8, height: 2 } for an
         // 8×4 terminal (row 0 = top ribbon, rows 1-2 = body, row 3 = bottom).
         terminal.backend_mut().assert_cursor_position((1, 2));
     }
     ```
   - **Verification:** test passes. If the `document_area.width` assertion
     fails, adjust the terminal size — `MIN_DOCUMENT_WIDTH.min(area.width)`
     in `draw_body` means the document width is `min(40, terminal_width)`.

10. **Add a selection render test over a synthesized trailing-space cell**
    - **Location:** `crates/writerm-app/src/draw.rs` tests.
    - **Action:** Type `"hello "`, select from char 4 to char 6 (covering
      the synthesized space cell), render, and assert that the cell at
      document col 5 has `bg == theme::selection_bg()`. This verifies the
      `to_line_with_selection` path sees the synthesized cell in
      `col_sources` and highlights it correctly.
    - **Verification:** test passes; the space cell is highlighted as
      expected (visually a highlighted blank cell, which is the correct
      behavior for selecting a space).

### Phase 6: Clean up and full verification

11. **Remove dead code**
    - **Location:** `crates/writerm-app/src/visual.rs:531`
      (`trim_edge_spaces`).
    - **Action:** Delete the function. It's no longer called after the
      `WrapMode` switch.
    - **Verification:** `cargo clippy -p writerm-app --all-targets -- -D
      warnings` reports no dead-code warning.

12. **Full workspace verification**
    - **Action:** Run, in order:
      - `cargo fmt --all`
      - `cargo check --workspace --all-targets`
      - `cargo test --workspace --all-targets`
      - `cargo clippy --workspace --all-targets -- -D warnings`
    - **Verification:** all four clean. In particular:
      - `writerm-app` lib tests: all green including the four
        new/updated tests from Phases 4-5 and the pre-existing 75.
      - `jones-render` tests: green including the new
        `rendered_line_source_covers_trailing_whitespace` test.
      - No regressions in `termite-app` or `termex-app` (they don't touch
        the visual layer but they do share `jones-render` — the
        `trim_source_to_visible_span_bounds` change is the only one that
        could affect them, and it only widens source ranges, which
        `termite`/`termex` don't consume).

13. **Update `.opencode/context/` if the design context is durable**
    - **Action:** If the `WrapMode` enum and the "synthesize trailing
      whitespace cells" rule turn out to be non-obvious to future
      contributors, add a short `.opencode/context/writerm-visual-mapping.md`
      note documenting: (a) the two `WrapMode` variants and when each is
      used, (b) the rule that every source char must have an addressable
      cell in rendered mode, (c) the fact that `pulldown-cmark` strips
      trailing whitespace from `Text` events and the visual layer
      re-synthesizes it from the line source range. Skip this if the code
      is sufficiently self-documenting after the changes.
    - **Verification:** the file exists and is accurate (re-read it after
      writing).

## Risks

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `trim_source_to_visible_span_bounds` change affects `termite`/`termex` rendering | Very low — `trim_source_to_visible_span_bounds` is only called from `MappedMarkdownRenderer::finish_line`, which is only used by `render_markdown_mapped`. `termite` (`crates/termite-app/src/ui.rs:61,310`) and `termex` (`crates/termex-app/src/app.rs:169`) both call `render_markdown` (the `Text`-returning `MarkdownRenderer`), not `render_markdown_mapped`. Verified by grep. | None | Run full `cargo test --workspace --all-targets`; the isolation is structural, not behavioral |
| Synthesized whitespace cells change `row_width` and break `move_visual_line_boundary` (End) for lines that didn't have trailing whitespace before | Low — synthesis only fires when `last_end < src.char_end`, which only happens when `pulldown-cmark` stripped trailing whitespace; lines without trailing whitespace have `last_end == src.char_end` | Low — End now goes one cell further on trailing-whitespace lines, which is the fix | The End-key render test in Phase 5 pins the new behavior; existing `rendered_home_end_move_to_wrapped_visual_row_boundaries` test (`app.rs:1720`) uses `"alpha beta gamma"` with no trailing whitespace and is unaffected |
| `WrapMode::Rendered` preserving wrap-boundary whitespace makes wrapped rows look indented by one blank cell | Medium — this is a visual change for any rendered line that wraps at a space | Low — the cell is invisible (a space); visually indistinguishable from a non-indented wrap, just the cursor can sit on it | This is the intended tradeoff (see Approach). If user feedback says the cursor-on-invisible-cell behavior is confusing, a follow-up could keep the cell in `col_sources`/`boundaries` but exclude it from `spans` — but that's a larger change and not needed for the fix. |
| Synthesized cells interact badly with `to_line_with_selection`'s `cell_intersects_selection` | Low — `cell_intersects_selection` reads `col_sources`, which the synthesized cell is part of | Low — selection over a space should highlight a blank cell, which is correct | Phase 5 step 10 has a dedicated selection-over-synthesized-cell render test |
| `from_rendered` synthesis pushes a cell that overflows the wrap width when the row is exactly `width` cells and has trailing whitespace | Low — `wrap_cells` handles overflow by wrapping; the synthesized cell is just another cell in the stream | None — wrapping is the correct behavior; the space goes to the next row | Phase 3 step's `trailing_rendered_whitespace_wraps_when_too_wide` unit test pins this |
| Removing `trim_source_to_visible_span_bounds`'s end-trim changes blank-line source assignment in `assign_blank_line_sources` | Low — `assign_blank_line_sources` (`markdown.rs:624`) operates on `line.source.char_start` for ordering, not `char_end` | Low — blank lines have `char_start == char_end`, so end-trim is a no-op on them | `rendered_blank_rows_preserve_full_source_range` test in `visual.rs::tests` covers this; verify it still passes |
| `Cursor` visibility / `hide_cursor` / `show_cursor` interactions with `TestBackend` | Low — `TestBackend::cursor_position(&self)` returns the stored `Position` regardless of the `cursor` visibility flag (`ratatui-core 0.1.1 src/backend/test.rs:113`) | None for tests; real terminal behavior unchanged | N/A — we don't change cursor visibility, only position |

## Verification

The fix is verified when **all** of the following hold:

1. `cargo fmt --all` — no diff.
2. `cargo check --workspace --all-targets` — clean.
3. `cargo test --workspace --all-targets` — all green, including:
   - `jones-render::tests::rendered_line_source_covers_trailing_whitespace`
     (new, Phase 2).
   - `writerm-app::visual::tests::trailing_rendered_whitespace_gets_cell`
     (new, Phase 3).
   - `writerm-app::visual::tests::trailing_rendered_whitespace_wraps_when_too_wide`
     (new, Phase 3).
   - `writerm-app::visual::tests::trailing_rendered_whitespace_maps_to_own_cell`
     (renamed/updated, Phase 4).
   - `writerm-app::app::tests::cursor_after_space_stays_on_current_visual_row_in_rendered_mode`
     (updated assertion, Phase 4).
   - `writerm-app::draw::tests::cursor_advances_after_typing_space_at_end_of_line`
     (new, Phase 5 — this is the direct end-to-end regression test for the
     reported bug).
   - `writerm-app::draw::tests::end_key_on_line_with_trailing_whitespace_lands_past_the_space`
     (new, Phase 5).
   - `writerm-app::draw::tests::cursor_moves_to_wrapped_row_after_typing_at_trailing_space_wrap_boundary`
     (new, Phase 5 — Case 2 regression test).
   - `writerm-app::draw::tests::selection_over_synthesized_trailing_space_cell`
     (new, Phase 5).
   - All pre-existing `writerm-app` tests still green (the 75 from the
     `06efbc6` baseline, with the two updated in Phase 4).
   - All pre-existing `termite-app` and `termex-app` tests still green.
4. `cargo clippy --workspace --all-targets -- -D warnings` — clean, no
   dead-code warning for `trim_edge_spaces`.
5. Manual sanity (optional, not CI-blocking): in a real terminal, `writerm
   note.md` with content `hello`, cursor at end of line, press Space — the
   blinking cursor visibly advances one cell to the right. Then press End
   — cursor lands past the space, not on it. Then Ctrl+M to source mode —
   wrapping still drops wrap-boundary spaces (no leading blank cell on
   wrapped rows).

The single most important acceptance criterion is **#3's
`cursor_advances_after_typing_space_at_end_of_line`**: it's the
end-to-end render test that ties `EditorContext::handle_char_insert(' ')`
→ `cursor_char_pos()` → `visual_document()` → `source_to_display` →
`draw::cursor_position` → `frame.set_cursor_position` →
`TestBackend::cursor_position` together, which is exactly the chain
the bug report identified as untested.
