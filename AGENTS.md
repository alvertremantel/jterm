# J-SUITE GUIDANCE - version 1

J-Suite is the integrated Rust workspace for the Jones Suite tools. Treat the root workspace as
authoritative and treat the suite as four applications built from many small, shared crates.

## Workspace authority and layout

- Root workspace: `/home/jones/dev/tools/j-suite/Cargo.toml`
- Workspace membership: `members = ["crates/*"]`
- Active crate root: `/home/jones/dev/tools/j-suite/crates/`
- Preserved exclusion: `termite/.worktrees/*` is intentionally excluded from workspace membership

### Authoritative source tree

- `crates/` — all active workspace member crates, including binaries and shared libraries
- top-level `azide/`, `termite/`, `jtop/` — legacy/pre-migration directories; do **not** treat
  them as the active source of truth

If a code/config/docs discrepancy exists between `crates/` and a preserved legacy directory,
`crates/` wins.

## Application map

There are four end-user applications:

- **azide** — RSS/Atom reader with Ratatui TUI plus feed-management CLI behavior
- **termite** — terminal text reader/editor with workspace navigation and search tools
- **writerm** — full-screen terminal Markdown writing app with rendered editing
- **jtop** — laptop power-management monitor/manager TUI

### Binary crates

- `azide`
- `termite`
- `writerm`
- `jtop`

### Azide crates

- `azide-app` — Azide TUI/app logic
- `azide-cli` — Azide command-line command handling
- `azide-config` — Azide config loading/saving and app-specific defaults
- `azide-explore` — curated/default feed exploration support
- `azide-feed` — RSS/Atom parsing and feed-level data handling
- `azide-store` — persisted feed/article storage

### Termite crates

- `termite-app` — Termite application/event-loop/TUI coordination
- `termite-config` — Termite config loading/saving and defaults
- `termite-editor` — compatibility re-export for shared editor behavior

### Writerm crates

- `writerm-app` — Writerm application/event-loop/TUI coordination
- `writerm-config` — Writerm config loading/saving and defaults

### Jtop crates

- `jtop-app` — Jtop TUI/app-state logic
- `jtop-core` — power-management domain logic, runners, parsers, privilege policy, etc.

### Shared Jones crates

- `jones-config` — shared config helpers
- `jones-editor` — shared editor interaction logic and editing workflows
- `jones-event` — shared event/input helpers
- `jones-outline` — outline extraction/breadcrumb-style structure helpers
- `jones-project-search` — recursive project text search
- `jones-render` — terminal rendering helpers for markdown/HTML content
- `jones-search` — shared in-app search state/behavior helpers
- `jones-state` — shared state models
- `jones-syntax` — syntax highlighting/styling support
- `jones-terminal` — terminal/session helpers
- `jones-text` — text-buffer/editing primitives
- `jones-theme` — theme palette and semantic color roles
- `jones-tui` — shared Ratatui UI helpers/widgets
- `jones-workspace` — filesystem/workspace browser logic

## Architectural guidance

- Prefer **small, sharply scoped crates** over growing app crates into monoliths.
- Shared behavior that is not app-specific should migrate toward `jones-*` crates.
- App crates should compose domain/shared crates rather than duplicate their logic.
- Avoid reintroducing dependencies on preserved top-level legacy directories.
- Keep root workspace dependency policy coherent; prefer shared versions in
  `[workspace.dependencies]` where sensible.

## Current dependency policy

The workspace currently aligns around these important shared versions/patterns:

- `ratatui = 0.30`
- `crossterm = 0.29`
- common shared dependencies are pinned at the root workspace where practical

Only add crate-local versions when the dependency is truly crate-specific.

## Licensing nuance

Licensing is intentionally **not** unified across the whole suite.

- **AGPL**: `azide`, `termite`, `writerm`, all `azide-*`, all `termite-*`, all `writerm-*`,
  and all `jones-*` crates
- **MIT**: `jtop`, `jtop-app`, `jtop-core`

Do not casually blur this boundary. If future work makes MIT `jtop*` crates depend on AGPL shared
crates, that is a real project-level licensing decision and should be surfaced explicitly.

## Testing and verification policy

CI/local verification should **not** depend on:

- live network access
- interactive TUI sessions
- real batteries
- `sudo` prompts
- `tlp`, `powertop`, or other host-specific tools being available

Prefer:

- unit tests
- integration tests with fakes/fixtures
- tempdirs
- smoke-check-only CLI help where safe
- pure policy/parsing tests around system integrations

When modifying behavior, verify with the strongest reasonable non-interactive checks first:

- `cargo fmt --all`
- `cargo check --workspace --all-targets`
- `cargo test --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`

## Runtime/config nuance

- `azide-config` preserves historical per-app config/data locations such as
  `~/.config/azide/config.toml` and `~/.local/share/azide/`
- `termite-config` preserves historical per-app config/data locations such as
  `~/.config/termite/config.toml` and `~/.local/share/termite/`
- `writerm-config` uses per-app config/data locations such as
  `~/.config/writerm/config.toml` and `~/.local/share/writerm/`
- `termite/.worktrees/` is preserved user state, not active workspace source

Preserve compatibility with user config/data paths unless there is an explicit migration plan.

## Branch guidance

- `main` — long-lived, durable, more permanent history
- `dev` — active integration/development branch
- `xxx-text` — temporary work branch for merging into `dev`
  - `xxx` = today’s short date tag
  - `text` = brief work description

## Working rules for agents and contributors

- Treat requests as real engineering work, not casual sketching.
- Produce idiomatic, well-tested Rust.
- Prefer explicit, compartmentalized design over clever sprawl.
- Keep documentation aligned with the actual crate layout.
- When removing old structure, verify nothing in `crates/` still depends on it.
- When touching UI colors/themes, use semantic theme roles in `jones-theme` rather than ad hoc
  color literals scattered through app code.
- When adding shared behavior, ask whether it belongs in an existing `jones-*` crate before
  expanding an app crate.

## Practical summary

The big migration already happened. The root workspace and `crates/` tree are the product. Work
from there, preserve noninteractive verification, respect the license split, and keep pushing the
suite toward reusable shared crates instead of app-local duplication.
