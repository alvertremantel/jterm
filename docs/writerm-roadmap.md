# Writerm Feature Roadmap — Toward a Competitive Terminal Word Processor

> **Status:** Design spec / forward-looking roadmap.
> **Scope:** `writerm` (binary) + `writerm-app`, `writerm-config`, and the shared
> `jones-*` crates it composes.
> **Audience:** contributors extending writerm from a "rendered Markdown editor"
> into a genuine, document-focused word processor for prose writers.

---

## 1. What writerm is today

writerm is a full-screen terminal Markdown writing app. Its distinguishing idea
is **rendered editing**: the raw Markdown stays the source of truth in the
buffer, but the user edits against a *rendered* view of the document, with a
bidirectional map (`VisualDocument` / `VisualRow.col_sources`) translating every
display cell back to a source char index.

`writerm-app` is a thin TUI shell. The real editing engine is `jones-editor`
(`EditorContext`), the renderer is `jones-render` (`render_markdown_mapped` →
`RenderedDocument`), and file-browsing/outline come from `jones-workspace` /
`jones-outline`. writerm's own novel code is the visual-mapping model
(`visual.rs`), layout/drawing (`draw.rs`), and live document metrics
(`metrics.rs`).

### Current capabilities (baseline)

| Area | State |
|---|---|
| Rendered editing (WRITE) vs source view (SOURCE) | Yes — toggled with `Ctrl+M`; non-Markdown files open in SOURCE automatically |
| Cursor / selection / word-jump | Yes — visual-aware in rendered mode; mouse click+drag select |
| Insert / delete / auto-close pairs / smart Enter (list continuation, auto-indent) / Tab-outdent | Yes (via `jones-editor`) |
| Undo / redo, system clipboard (arboard) | Yes |
| Inline formatting: bold `Ctrl+B`, italic `Ctrl+I`, link `Ctrl+K`, headings `Ctrl+1..6` | Yes |
| Markdown render coverage | Broad (pulldown-cmark): H1–H6, emphasis/strong/strikethrough, links, inline+fenced code, blockquotes, nested lists, task checkboxes, HR, tables w/ alignment, images/alt, footnotes |
| Live metrics | Characters, words, sentences, paragraphs, reading time @ 180 wpm |
| Outline / breadcrumb navigation | Yes — headings sidebar, click-to-jump, active-heading breadcrumb in ribbon |
| File browser sidebar | Yes — Markdown-first sort, click to open, `Ctrl+N` new file |
| Autosave + save-on-switch/quit | Yes — debounced (`autosave.delay_ms`, default 1000 ms), atomic temp-file+rename |
| Config | `[ui].mouse`, `[autosave]`, `[workspace]`, `[layout]` panel widths |

### Notable gaps / latent capability (the starting point for this roadmap)

These came out of a code survey and are the cheapest, highest-leverage wins:

1. **Find/replace is unreachable.** `jones-editor` contains a *complete*
   find/replace state machine, but writerm treats `EditorAction::Find` as a
   no-op (`app.rs:248`) and draws no search bar. Worse, `Ctrl+F` currently sets
   `search_active` and then routes typed keys into an invisible
   `handle_search_key` — a latent trap. **This is the single biggest dormant
   feature.**
2. **`ExitEditor`, `ToggleSplitPreview`, `ReloadFile` are silently swallowed**
   (`app.rs:249-252`). No split preview, no reload-from-disk, `Esc` does nothing.
3. **No keyboard focus for sidebars.** File browser and outline are
   mouse-only; there is no way to Tab into them from the keyboard.
4. **No theme switching at runtime.** `jones-theme` exposes
   `set_current` / `next_id` / `available` and a second theme (`CLEAN_BLUE`),
   all unwired. No theme config field exists.
5. **No spellcheck, export, print/pagination, front-matter handling, comments,
   or track-changes** — the core word-processor features prose writers expect.
6. **Prompt system is minimal** — only `PromptMode::NewFile`; no rename,
   delete, save-as, or open-path. New files are restricted to the current folder.
7. **No keybinding config**; chords are hardwired. Tabs hardcoded to 3 cells.
8. **Unused shared crates:** `jones-syntax` (per-line highlighting for code
   fences / SOURCE view), `jones-tui` (`centered_rect`, `draw_help`/`HelpSection`
   overlays), `jones-search` (sidebar fuzzy filter).

---

## 2. Design principles

Keep these in mind for every feature below; they are what makes writerm
distinct from "vim with a Markdown plugin".

- **Prose-first, not code-first.** The target user is writing documents, not
  programs. Defaults, metrics, and shortcuts should favor authors.
- **Rendered editing is the identity.** Every new feature should work *in the
  rendered view*, not force the user back to raw source. New formatting must
  round-trip through the `VisualDocument` source-map.
- **Markdown stays the on-disk truth.** Export formats are derived; `.md` is
  canonical. No proprietary binary format.
- **Compose shared crates; don't fork them.** Per `AGENTS.md`: shared behavior
  belongs in `jones-*`. Wire up dormant `jones-editor`/`jones-theme`/`jones-tui`
  surface before writing new code. Push genuinely reusable additions down into
  `jones-*`.
- **Non-interactive verifiability.** Everything must be testable without a live
  TTY (unit tests + fixtures + tempdirs), per the workspace testing policy.
- **Semantic theming.** No ad-hoc color literals; use `jones-theme` roles.

---

## 3. Roadmap

Phases are ordered by dependency and value. Each item notes the primary
crate(s) touched and whether it's mostly *wiring* (dormant capability) or *new*.

### Phase 0 — Close the obvious gaps (wiring, low risk)

These unlock capability that already exists in the shared crates. Do them first.

- **0.1 In-document find & replace UI** *(wiring; `writerm-app`, uses
  `jones-editor` + `jones-tui`)*
  Wire `EditorAction::Find` to open a search bar; render match highlights using
  the existing `SEARCH_MATCH_BG` / `SEARCH_CURRENT_BG` theme roles; map source
  match ranges through `source_to_display` so highlights land correctly in the
  rendered view. Expose replace (`replace_current_match`, `replace_all_matches`
  already exist). Draw the bar with `jones-tui::draw_search` *or* a hand-rolled
  bottom bar (avoid dragging in `jones-state` if not otherwise needed).
  **Acceptance:** `Ctrl+F` shows a bar; `Enter`/`n`/`N` cycle matches; matches
  highlight in WRITE mode; `Esc` dismisses without the current invisible trap.

- **0.2 Reload-from-disk & external-change detection** *(wiring;
  `EditorAction::ReloadFile`)*
  Wire `ReloadFile`; detect on-disk mtime changes (writerm autosaves, so guard
  against clobbering external edits) and prompt on conflict.

- **0.3 Escape / cancel semantics** *(new, small)*
  Give `Esc` real behavior: dismiss search bar / prompt / notification; make it
  predictable rather than a no-op.

- **0.4 Help overlay** *(wiring; `jones-tui::draw_help` + `HelpSection`)*
  A `?` / `F1` overlay listing keybindings via `centered_rect` + `draw_help`.
  Removes the need to memorize hardwired chords. Cheapest possible win.

- **0.5 Keyboard focus for sidebars** *(new; `writerm-app`)*
  `Tab` cycles focus document ↔ outline ↔ files; arrow keys navigate the
  focused list; `Enter` jumps/opens. Render focused vs unfocused states (theme
  already has the roles). Unblocks keyboard-only use.

### Phase 1 — Core word-processor authoring features

- **1.1 Spellcheck** *(new; candidate new crate `jones-spell`)*
  Live spellcheck is the #1 expected word-processor feature. Underline
  misspellings in the rendered view (map word ranges via the source-map),
  offer suggestions in a popup, per-document + user dictionary
  (`~/.local/share/writerm/`). Suggest a pure-Rust dictionary approach
  (bundled word list + edit-distance suggestions) to honor the no-network
  test policy; hunspell/`.dic` support optional and offline. Skip code fences,
  URLs, and inline code. **Belongs in a shared `jones-spell` crate** so termite
  can use it too.

- **1.2 Export** *(new; extend `jones-render`)*
  Derive output formats from the canonical Markdown:
  - **HTML** — `jones-render::html` already exists (currently unused by
    writerm); wrap with a stylesheet for standalone export.
  - **Plain text** — strip formatting.
  - **PDF / DOCX** — larger effort; gate behind optional feature flags and
    external tooling (e.g. shell out to `pandoc` if present, detected
    gracefully). Do *not* require it for build/test.
  Command surface: a `PromptMode::Export` with format choice, or `:export html`.

- **1.3 Word-count goals & session stats** *(new; extend `metrics.rs`)*
  Writers set targets. Add a per-document word-count goal (progress bar in the
  metrics panel), words-written-this-session, and optional selection-scoped
  counts (count words in the current selection). Persist goals in front-matter
  (see 1.5) or a sidecar.

- **1.4 Improve metric accuracy** *(new; `metrics.rs`)*
  Current sentence/paragraph heuristics are deliberately simple and ASCII-only
  (`.!?`, ellipsis counts as 3, CJK `。` uncounted). Improve for real prose:
  Unicode sentence terminators, abbreviation handling, and exclude Markdown
  syntax (heading `#`, list markers, code fences) from word counts so metrics
  reflect *prose*, not markup.

- **1.5 YAML/TOML front-matter** *(new; `writerm-app` + maybe `jones-render`)*
  Recognize `---`-fenced front matter: title, author, date, tags, word-goal.
  Render it as a distinct document-header block (not raw), surface title in the
  ribbon, and use it for export metadata. This is how a Markdown file behaves
  like a "document with properties".

### Phase 2 — Prose-writing UX polish

- **2.1 Focus / typewriter / zen modes** *(new; `writerm-app` + config)*
  - **Focus mode:** hide both sidebars, center the text column at a comfortable
    measure (e.g. 72–80 cols), dim everything but the current
    sentence/paragraph.
  - **Typewriter scrolling:** keep the cursor line vertically centered.
  These are signature "distraction-free writer" features (iA Writer, Typora).
  Config: `[editor].focus_mode`, `[editor].typewriter`, `[editor].measure`.

- **2.2 Theme selection & runtime switching** *(wiring; `jones-theme` + config)*
  Add `[ui].theme` config; wire `set_current` at startup and a theme-cycle
  keybind (`next_id` / `available`). Ships `CLEAN_BLUE` immediately. Consider a
  light theme and a high-contrast theme for accessibility.

- **2.3 Syntax highlighting in SOURCE view & code fences** *(wiring;
  `jones-syntax`)*
  Apply `Highlighter::for_path` in SOURCE view and to fenced code blocks in the
  rendered view. Makes SOURCE mode legible and code samples in documents
  readable. Zero new engine work — the highlighter exists.

- **2.4 Smart typography** *(new; `jones-editor` or `writerm-app`)*
  Optional auto-conversion of straight quotes → curly, `--` → en/em dash,
  `...` → ellipsis, as-you-type (toggleable — writers of Markdown-for-code will
  want it off). Keep the *source* smart-typographically correct so it renders
  and exports identically everywhere.

- **2.5 Configurable keybindings & tab width** *(new; `writerm-config`)*
  Move the hardwired chord table into config (`[keys]`), with the help overlay
  (0.4) reading from the same source. Make the hardcoded 3-cell tab width
  (`visual.rs:12`) a `[editor].tab_width` setting.

### Phase 3 — Document management & structure

- **3.1 Richer file operations** *(new; extend `PromptMode`)*
  Add rename, delete (to trash where possible), save-as, and open-arbitrary-path
  prompts. Lift the current-folder-only restriction on new files where safe.

- **3.2 Fuzzy quick-open / command palette** *(new; `jones-search` +
  `jones-state`, or a lightweight local matcher)*
  `Ctrl+P`-style fuzzy file open across the workspace, and a command palette for
  discoverable actions. `jones-search::update_search_results` +
  `handle_search_key` provide the machinery (couples to `jones-state::CoreState`;
  decide whether to adopt that dependency or write a small local matcher).

- **3.3 Outline as document navigator + reorder** *(new; `jones-outline` +
  `writerm-app`)*
  Beyond click-to-jump: collapse/expand sections in the outline, and
  **move/promote/demote whole sections** from the outline (a hallmark of
  serious document editors — think Scrivener/Word Navigation pane). Requires
  section-range operations on the buffer.

- **3.4 Multi-document / project sessions** *(new)*
  Remember open file + cursor position per workspace; optional recent-files
  list. Lightweight, honors `~/.local/share/writerm/`.

### Phase 4 — Collaboration-adjacent & advanced

Higher effort; pursue once the core writing experience is strong.

- **4.1 Comments / annotations** — margin or inline comments stored as an
  HTML-comment / footnote convention so the `.md` stays portable.
- **4.2 Track changes / revision diff** — visualize changes vs the last saved
  version or a git baseline; writerm lives in a git workspace already.
- **4.3 Snapshots / version history** — periodic local snapshots in
  `~/.local/share/writerm/` (the roadmap for backups; note writerm currently
  keeps *no* backups beyond atomic save).
- **4.4 Live word-wrap reflow at configurable measure** and print-style
  pagination preview for export.
- **4.5 Citations / footnote management** — footnotes already render; add
  insertion helpers and a references panel for academic/long-form writers.

---

## 4. Competitive positioning

Where writerm should aim to beat the field, per competitor:

| Competitor | Their strength | writerm's angle |
|---|---|---|
| iA Writer / Typora | Beautiful rendered editing, focus mode | Match rendered editing (already have it) + focus mode (2.1); win on *terminal-native*, keyboard-first, zero-GUI |
| Word / Google Docs | Spellcheck, track changes, export, styles | Cover the 80%: spellcheck (1.1), export (1.2), comments/track-changes (4.1/4.2) — without the bloat, in a `.md`-portable format |
| Scrivener | Document structure, outline reorder, project mgmt | Outline reorder (3.3) + project sessions (3.4) |
| Vim/Emacs + plugins | Power, extensibility | Prose-first defaults, discoverable keys (help overlay + configurable binds), no plugin assembly required |

**The wedge:** a terminal app that renders while you write, counts words like a
writer cares about, checks spelling, exports cleanly, and keeps everything as
portable Markdown — with no mouse required and no GUI.

---

## 5. Suggested near-term sequencing

1. **Phase 0 in full** — it's almost entirely wiring dormant capability and
   removes the current find/replace trap. Highest value-per-effort.
2. **1.1 Spellcheck** and **1.2 Export (HTML first)** — the two features whose
   absence most disqualifies writerm as a "word processor".
3. **2.1 Focus mode** and **2.3 syntax highlighting** — cheap, high-visibility
   polish that leans into the rendered-editing identity.
4. Everything else as capacity allows, keeping shared logic flowing into
   `jones-*` crates.

## 6. Cross-cutting engineering notes

- **New shared crates to consider:** `jones-spell` (spellcheck),
  `jones-export` (format derivation) — both reusable by termite/termex.
- **The source-map is load-bearing.** Any feature that highlights, underlines,
  or annotates rendered text (find highlights, spellcheck squiggles, comments)
  must go through `VisualDocument::source_to_display` — budget for extending
  that API rather than bypassing it.
- **Testing:** follow the workspace policy — unit tests, fixtures, tempdirs; no
  live network, no interactive TUI in CI. Spellcheck dictionaries and export
  fixtures must be bundled/offline.
- **Config compatibility:** `writerm-config` fields are all
  `#[serde(default)]`; keep new fields defaulted so existing
  `~/.config/writerm/config.toml` files keep loading.
