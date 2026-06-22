# Bug Report: Writerm cursor position does not visually update after typing a space (and likely other trailing-whitespace edits)

## Summary

In `writerm` (the rendered-Markdown editor), after typing a space — and possibly
after other edits that put a trailing space at the end of a visible row — the
**terminal cursor (the blinking cell) does not visibly move** even though:

- the editor's internal cursor state advances correctly
- the inserted character renders in the correct place on screen
- `cursor_char_pos()` and `source_to_display(...)` return the post-edit
  position

The user sees the character appear, but the cursor stays where it was, which
gives the impression that "the cursor position usually doesn't update after
hitting the spacebar." The bug is **specific to `writerm`** — `termite` and
`termex` (which don't have a rendered-Markdown visual layer) do not exhibit
this behavior.

## Severity

Low–medium. No data loss. Cosmetic / confusing UX. The user is editing a
document and the cursor appears "stuck" in a position the user is no longer
at, which causes real-world misedits (e.g. typing into the wrong place because
the user trusted the visual cursor position).

## Environment

- Repository: `termite` (J-Suite) — `/home/jones/dev/tools/termite`
- Affected binary: `writerm`
- Unaffected binaries: `termite`, `termex`
- Branch where reproduced: `fix/cursor` (and presumably `main` / `dev`)
- Workspace commit at time of investigation: `06efbc6` ("updated guidance")
- Rust edition 2024; `ratatui = 0.30`; `crossterm = 0.29`

## How to reproduce

These are minimal conditions that trigger the symptom. Any of them are
sufficient; they are listed from most reliable to most common in real
documents.

### 1. End-of-line space (most reliable, matches the reported user complaint)

1. Open `writerm` on a plain text or Markdown file (e.g. create `note.md`
   containing `hello`).
2. Position the cursor at the end of the line (col 5 in `hello`).
3. Press `Space`.

**Expected:** the cursor visibly advances one cell to the right (col 6).

**Observed:** the space character is drawn at col 5, but the blinking
terminal cursor remains at col 5 (or somewhere inconsistent with the new
position). To the eye, "the cursor doesn't update."

### 2. Trailing space at end of a line that wraps to the next visual row

1. Open a document with a line that contains trailing whitespace and wrap
   (e.g. `"abcdefgh "` rendered into a width-8 column).
2. Press any character at the end of that line.

**Expected:** the cursor moves into / past the wrapped row.

**Observed:** the cursor mapping is wrong because the visual representation
of the trailing space is dropped during wrapping, and the source → display
mapping is computed off the trimmed cells.

### 3. Trailing whitespace that becomes the last character of a wrapped row

Any edit that leaves a row of `VisualRow` ending in a whitespace cell that
gets trimmed in the visual layer.

## Why this is hard to catch with current tests

The current regression test
`cursor_after_space_stays_on_current_visual_row_in_rendered_mode` in
`crates/writerm-app/src/app.rs` only checks
`source_to_display(cursor_char_pos())` and the document scroll. That is the
**half** of the story — it confirms the source-to-display mapping is
correct, but it does not check what the terminal cursor coordinate actually
is on screen. The terminal cursor is placed by
`draw_document → cursor_position → frame.set_cursor_position((x, y))` in
`crates/writerm-app/src/draw.rs`, and the relationship between
`source_to_display` and the final `(x, y)` is not covered by an end-to-end
rendering assertion.

There is also `trailing_rendered_whitespace_maps_to_previous_visible_row`
in `crates/writerm-app/src/visual.rs` that asserts
`source_to_display(6) == Some((0, 5))` for `"hello "` — this test also
passes today. It captures the same surface behavior as the first test
above, and similarly does not assert anything about the rendered terminal
output or the cursor cell.

So both halves of the bug (mapping correct, terminal placement also correct
in isolation) pass their unit tests individually; what's missing is a
test that ties them together, and a fix that ensures they stay tied.

## Suspected root cause

Investigation centered on the `VisualDocument` wrapping pipeline in
`crates/writerm-app/src/visual.rs`. Two distinct mechanisms in that file
silently drop trailing whitespace cells from the rendered output:

### Mechanism A — `VisualRow::from_cells` (line 168)

```rust
fn from_cells(mut cells: Vec<Cell>, trim_edges: bool, fallback_source: Option<usize>) -> Self {
    if trim_edges {
        trim_edge_spaces(&mut cells);   // <-- trims trailing whitespace
    }
    ...
    let last_source = cells
        .iter()
        .rev()
        .find_map(|cell| cell.source.map(|(_, end)| end));
    let source_start = first_source.or(fallback_source).unwrap_or(0);
    let source_end = last_source.or(fallback_source).unwrap_or(source_start);
    ...
    boundaries.push((source_end, col));
    ...
}
```

When `trim_edges = true` (which is what `wrap_cells` passes for
`from_rendered`; see line 83), a row whose last cell is whitespace loses
that cell from `cells` and therefore from `spans`, `col_sources`, and
`boundaries`. After typing `"hello "` the row's `source_end` is `5` (not
`6`), `col_sources` has length 5, and `boundaries` ends with `(5, 5)`.
The `source_to_display` helper falls through to the
`closest = Some((row_idx, row.width()))` branch and returns
`Some((0, 5))` — which is technically the column **one past the last
visible 'o'** but is not the column the cell would have occupied had the
space been preserved. That distinction matters because the rendered
"hello" line is only 5 cells wide, so the cursor at col 5 lands on the
blank cell immediately after the last 'o' — which is exactly the
position the user would expect — but the user is now editing a character
position (`6`) that, if rendered, would have a cell to its right, and
any subsequent cursor movement (e.g. `Right`, or a selection drag) is
computed against the trimmed geometry.

In short: the space is "there" for the cursor (col 5) and for the
character (it was inserted at char position 5 — wait, char position 6,
display col 5), but it is **not there as a visible cell**, so the
visual cursor and the visual text are off by one cell of buffer, and
that's the "stuck cursor" impression.

### Mechanism B — `CellWrapper::push` (line 410)

```rust
if cell_is_whitespace(&cell) {
    trim_trailing_spaces(&mut self.current);
    self.recompute_width();
    self.flush_current();
    return;   // <-- the whitespace cell is dropped on the floor
}
```

This branch fires when a whitespace cell arrives at a wrap boundary (a
row is already full and the next cell is a space). The current row has
its trailing spaces trimmed, is flushed, and the whitespace cell is
discarded. The text never makes it into the next row, and there's no
boundary entry recording its source position. Any source character
position covered by that space then has no `source_to_display` mapping
at all in the wrapped region.

### Why `termite` / `termex` don't see this

`termite` and `termex` draw the buffer text directly with a per-line
`Paragraph`. There is no Markdown rendering step, no `RenderedDocument`,
no `VisualDocument::from_rendered`, no `wrap_cells`, and no `trim_edges`.
The cursor position is computed from the same line text the user sees,
so the cursor and the character are always in lockstep. `writerm`
introduces an intermediate representation (rendered Markdown cells, then
wrapped visual rows) that discards trailing whitespace, and that's where
the desync is born.

## What was tried

This is intentionally a **bug report**, not a fix. The following was
attempted in this investigation session, none of which surfaced a
reproducing failure in CI:

1. Read the editor key path
   (`EditorContext::handle_key` → `handle_char_insert(' ')` → `insert_char` →
   `state.move_cursor(Direction::Right, rope)`) in
   `crates/jones-editor/src/editor.rs`. The state mutation is correct; a
   temporary unit test (`spacebar_moves_cursor_forward`) confirmed
   `cursor_col` advances from 1 → 2 after typing a space into `"hello"`,
   and the buffer becomes `"h ello"`. The test was removed before
   writing this report; the working tree is clean on `fix/cursor`.

2. Read the `Direction::Right` arm of `EditorState::move_cursor` and the
   grapheme-boundary helpers in `crates/jones-text/src/lib.rs`. They
   advance `cursor_col` using `next_grapheme_boundary` on a
   `trim_end_matches('\n'/'r')` of the line content. For a single
   ASCII space the new column is `cursor_col + 1`. This is correct.

3. Read `draw_editor` in `crates/termite-app/src/ui.rs` and
   `draw_document` in `crates/writerm-app/src/draw.rs`. `writerm` is
   the only path that uses a `VisualDocument` for the cursor; `termite`
   and `termex` map straight from the buffer to the screen.

4. Ran `cargo build --workspace --all-targets` — clean.
5. Ran `cargo test --workspace --all-targets` — all green, including:
   - `termite-editor`, `jones-editor` (97/97)
   - `writerm-app` (75/75), including the
     `cursor_after_space_stays_on_current_visual_row_in_rendered_mode`
     regression test
   - The visual test
     `trailing_rendered_whitespace_maps_to_previous_visible_row`
6. Removed the temporary test from `jones-editor`; working tree is
   clean. Branch is `fix/cursor`, ahead of `main` by exactly the
   workspace-trim work, no extra commits.

No fix was applied because no regression test currently fails, and the
suspected root cause above is plausible but not yet reproduced
deterministically with an end-to-end assertion.

## Proposed test (to convert this report into a fix)

Add a `ratatui::Terminal<TestBackend>`-driven render test in
`crates/writerm-app/src/draw.rs` (alongside
`renders_ribbon_headings_document_files_and_keybar`) that:

1. Creates a `WritermApp` over a file containing `"hello"`.
2. Sets `document_area = Rect::new(0, 0, 20, 3)`.
3. Calls `app.editor.move_cursor_to_char_pos(5)`.
4. Calls `app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))`.
5. Draws the frame.
6. Asserts that the cell at `(area.x + 5, area.y)` is the post-space
   position — concretely, asserts that the cursor is set there. With
   `TestBackend`, the cursor position is not directly observable, but
   the rendered cell at that coordinate should be the inserted space
   (`' '`) when rendered, and the mapping for the next typing
   operation should be consistent.

A complementary unit test should assert that a
`VisualDocument::from_rendered` over `"hello "` at width 20 has
`col_sources.len() == 6` and `boundaries` ending with `(6, 6)` (or an
equivalent source-to-display mapping that preserves the trailing
whitespace cell), so that the visual layer never silently drops
trailing whitespace from a rendered row.

## Proposed fix shape (for the next agent)

In `crates/writerm-app/src/visual.rs`, change the contract so
**trailing whitespace is never silently dropped from the visual
representation**:

1. In `VisualRow::from_cells` (line 168), either:
   - stop calling `trim_edge_spaces` when `trim_edges` is true and the
     trailing whitespace carries a `source` (i.e. it is a real
     whitespace cell from the document, not an inserted padding
     cell), or
   - keep the trimming for the *displayed* `spans` (so the rendered
     line doesn't grow) but retain the whitespace cells in
     `col_sources` and `boundaries` so the source-to-display mapping
     stays correct.

2. In `CellWrapper::push` (line 410), instead of `return` after
   `flush_current()`, push the whitespace cell into the new
   `current` row, and only drop it if it would overflow the width
   (which it won't, by definition of the branch — we just flushed).

3. Audit the `from_source` path
   (`VisualDocument::from_source`, line 36) for the same
   trailing-whitespace loss; today it uses `wrap_cells(..., trim_edges =
   false)`, which is the correct setting, but the `CellWrapper` bug
   above still applies.

4. After the visual layer is correct, re-evaluate whether
   `cursor_position` in `draw.rs` needs to add 1 to `col` when the
   character at the current col is a whitespace cell that is being
   "kept but not drawn" — the cursor in `ratatui` overwrites the cell
   it sits on, so if the trailing space is rendered as an empty span,
   the cursor's visual position is the space's cell, which is what the
   user expects.

## Files of interest

- `crates/writerm-app/src/visual.rs` — `VisualDocument`,
  `VisualRow::from_cells` (line 168), `CellWrapper::push` (line 403),
  `wrap_cells` (line 352), `trim_trailing_spaces` (line 540)
- `crates/writerm-app/src/draw.rs` — `draw_document` (line 186),
  `cursor_position` (line 212), `frame.set_cursor_position` (line 208)
- `crates/writerm-app/src/app.rs` — `handle_key` (line 152),
  `handle_editor_key` (line 221), `visual_document` (line 680),
  `refresh_render_cache` (line 627)
- `crates/jones-editor/src/editor.rs` — `EditorContext::handle_key`,
  `handle_char_insert`, `cursor_char_pos` (line 164); not implicated,
  included for completeness
- `crates/jones-text/src/lib.rs` — `EditorState::move_cursor`,
  `next_grapheme_boundary`; not implicated
- `crates/termite-app/src/ui.rs::draw_editor` and
  `crates/termite-app/src/draw.rs` — `termite` cursor path; works
  correctly, included for comparison
- `crates/termex-app/src/app.rs` — `termex` cursor path; works
  correctly, included for comparison

## Workarounds for the user

Until this is fixed, the user can avoid the symptom by:

- Using `Ctrl+M` to enter `source_peek` mode in `writerm`; in source
  mode the buffer is drawn 1:1, no `VisualDocument` is involved, and
  the cursor stays in lockstep with the characters.
- Switching to `termite` or `termex` for documents where cursor
  accuracy is critical.
- Using `Right` / `Left` / arrow keys after a space; the cursor
  position is recomputed by `ensure_cursor_visible` and
  `move_visual_horizontal`, which use the same `source_to_display` —
  the symptom is that the cursor's *idle* position is wrong, not that
  movement is broken.
