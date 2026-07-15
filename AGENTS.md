# J-Suite Guidance

J-Suite is an integrated Rust workspace for the Jones Suite terminal tools. The root
workspace is authoritative; focus development on **termite** (workspace-oriented text
editor) and **writerm** (rendered-Markdown writing app). **termex** remains a workspace
member but is not a primary development target.

## Workspace layout

- **Root:** `Cargo.toml` — workspace membership is an explicit allowlist in `members = [...]`
- **Source tree:** `crates/` — all 24 active workspace member crates
- **Preserved exclusion:** `termite/.worktrees/*` is intentionally excluded from workspace
  membership (not an active source directory)
- **Config/data:** platform conventions handled by `jones-config`; per-app config lives at
  `$XDG_CONFIG_HOME/<app>/config.toml`, data at `$XDG_DATA_HOME/<app>/`

If code, config, or docs disagree with the active crates under `crates/`, the `crates/` tree
wins.

## Application map

### Primary targets

- **termite** — terminal text editor with workspace navigation, project search, and
  multi-buffer editing. Binary: `crates/termite`.
- **writerm** — full-screen terminal Markdown writing app. Source Markdown is canonical;
  the rendered visual representation (`VisualDocument` / `VisualRow.col_sources`) maps
  every visual column back to a source-buffer byte offset. Cursor, selection, and scroll
  operate on source positions; the visual layer is derived and must never drift. Binary:
  `crates/writerm`.

### Additional targets

- **termex** — single-document terminal reader/writer. Present and maintained but receives
  lower development priority. Binary: `crates/termex`.

## Crate layer architecture

### Binary crates (thin entry points)

- `termite` — depends on `termite-app`
- `writerm` — depends on `writerm-app`
- `termex` — depends on `termex-app`

### App crates (event loop, TUI coordination, per-app behavior)

- `termite-app` — termite application orchestration and TUI
- `writerm-app` — writerm full-screen Markdown app
- `termex-app` — termex single-document app

### Config crates (load/save, defaults)

- `termite-config` — termite config loading, depends on `jones-config`
- `writerm-config` — writerm config loading, depends on `jones-config`
- `termex-config` — termex config loading, depends on `jones-config`

### Compatibility crate

- `termite-editor` — thin re-export shim over `jones-editor` for termite/termex app crates
  that have not yet migrated directly

### Shared jones-* crates

- `jones-config` — shared config helpers (platform path resolution via `dirs`)
- `jones-editor` — shared editor interaction logic and editing workflows
- `jones-event` — shared event/input helpers (crossterm event streams)
- `jones-outline` — outline extraction / breadcrumb-style structure helpers
- `jones-project-search` — recursive project text search
- `jones-render` — terminal rendering for Markdown / HTML content (pulldown-cmark → styled output)
- `jones-search` — shared in-app search state and behavior
- `jones-state` — shared state models
- `jones-syntax` — syntax highlighting / styling support
- `jones-terminal` — terminal / session helpers
- `jones-text` — text-buffer / editing primitives (Ropey-based gap buffer)
- `jones-theme` — theme palette and semantic color roles
- `jones-tui` — shared Ratatui UI helpers / widgets
- `jones-workspace` — filesystem / workspace browser logic

## Architectural guidance

- Prefer **small, sharply scoped crates** over app-local monoliths.
- Shared behavior not specific to one app should migrate toward `jones-*` crates.
- App crates compose domain / shared crates rather than duplicating logic.
- **Writerm visual-layer discipline:** source Markdown is the single source of truth.
  `VisualDocument` / `VisualRow.col_sources` must always provide a correct byte-offset
  mapping from every visual column back to the source buffer. Cursor, selection, and
  highlight positions are computed in source space; the visual representation is derived
  and must stay in sync.
- Keep root workspace dependency policy coherent; prefer `[workspace.dependencies]` for
  shared versions.
- Only add crate-local versions when the dependency is truly crate-specific.
- Avoid reintroducing dependencies on preserved non-workspace directories.

## Licensing

All active workspace crates under `crates/` are **AGPL-3.0**. Keep manifests and
contributor guidance aligned with that scope.

## Verification policy

CI / local verification must not depend on live network access or interactive TUI sessions.
Prefer unit tests, integration tests with fakes / fixtures, tempdirs, and smoke-check CLI
help where safe.

When modifying behavior, verify with the strongest reasonable non-interactive checks:

```
cargo fmt --all
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

## Current workspace dependency pins

- `ratatui = 0.30` (with crossterm feature)
- `crossterm = 0.29` (with event-stream feature)
- `pulldown-cmark = 0.12`, `serde = 1`, `toml = 0.8`, `dirs = 6`, `color-eyre = 0.6`,
  `tokio = 1` (with macros, rt-multi-thread, sync, time)
- `tempfile = 3`, `futures-lite = 2`

## Working rules

- Treat requests as real engineering work, not casual sketching.
- Produce idiomatic, well-tested Rust.
- Prefer explicit, compartmentalized design over clever sprawl.
- Keep documentation aligned with the actual crate layout.
- When removing old structure, verify nothing in `crates/` still depends on it.
- When touching UI colors / themes, use semantic theme roles from `jones-theme` rather
  than ad hoc color literals.
- When adding shared behavior, ask whether it belongs in an existing `jones-*` crate
  before expanding an app crate.

## Branch guidance

- `main` — long-lived, durable, more permanent history
- `dev` — active integration / development branch
- `<date>-<brief-description>` — temporary work branches merging into `dev`
  (e.g., `0715-visual-fix`)
