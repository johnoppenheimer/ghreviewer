# AGENTS.md

Local macOS app to triage GitHub PRs. Tauri 2 (Rust in `src-tauri/`) + React/TS (`src/`). User docs: `README.md`. Design rules: `PRODUCT.md` (binding: monospace, straight lines, no rounding, no shadows).

## Commands

- `mise run install | dev | build | dmg | fmt | lint | test`. Run `fmt` and `lint` before committing; lint must be clean.
- `cd src-tauri && cargo test -- --ignored live --nocapture`: real sync against GitHub. Run it after changing a GraphQL query.
- Ignore oxlint warnings under `.opencode/`: those files aren't part of the app.

## Workflow

- One commit per feature. Stage only your own files: the user often has work in progress in the tree.
- Tests: Rust unit tests, plus `bun test` (`src/logic.test.ts`) for pure frontend logic. No e2e. Put testable logic in `src/model.ts`, not in components.
- Never spend agent tokens to test prompt runs. Insert fake rows into `runs` in `cache.db` with sqlite3, then delete them.

## Architecture

- **GitHub.** All calls are GraphQL from Rust (`sync.rs`), with the token from `gh auth token`. Search and stack-parent lookups share the `PrFields` fragment.
- **Pushing changes to the UI.** The Rust side emits `changed`, `pr-updated` and `run-updated`. The frontend invalidates its TanStack queries on those events and never polls.
- **Running tools.** Run external tools through `local::login_command` (a login shell): a GUI app doesn't get the user's `PATH`.
- **Paths.**
  - Config: `~/.config/ghreviewer/config.toml`. It's hot-reloaded, and the app writes it with `toml_edit` so comments survive. Document new keys in `config::STARTER`.
  - Data: `~/Library/Application Support/ghreviewer/`, holding `cache.db` and `runs/`.
- **Jira** (optional, `jira.rs`). After GitHub, each sync fetches the tickets named in inbox PR titles (`[TI-123]` or `TI-123`) into the `jira` table. `list_inbox` fills `PrSummary.jira` from that cache. The token never goes to the webview (`skip_serializing`). Jira's HTML is sanitized in `JiraView.tsx`.
- **New `PrSummary` fields** need `#[serde(default)]`: cached rows were written without them.
- **`context` PRs** are fetched stack parents, or PRs the local filters drop. Show them only greyed, as stack parents. Never count, prefetch or mark them unread.
- **Untrusted content.**
  - Sanitize GitHub HTML with DOMPurify.
  - Repo images (`github.com/<repo>/blob|raw/...`) load through `ghimg://` (`lib.rs::gh_image`), fetched with the token. It serves images only.
  - Agent HTML goes through `runfile://` (`lib.rs::run_file`, same scope as the asset protocol) into `<iframe sandbox="allow-scripts">`.
  - `open_path` only opens files listed in that PR's runs.
- **Agent runs** stop when the app quits. Killing the `opencode run` client aborts the turn, even with opencode's background service. At startup they're marked `interrupted`.

## Testing the UI by hand

- The user runs either `mise run dev` or `/Applications/ghreviewer.app`, and both share the same data dir. Before starting a second instance, check `SELECT ... FROM runs WHERE status='running'` (startup marks those interrupted). If port 1420 is busy, the user's dev server is already running and hot-reloads your frontend changes.
- To screenshot:
  1. Find the window id by the app's pid with a swift `CGWindowListCopyWindowInfo` one-liner.
  2. Capture it with `screencapture -x -o -l <id>`.
  3. Crop the image with a swift `NSBitmapImageRep` script.
- Send keystrokes and resize the window with `osascript` System Events. Stop any instance you started.

## Gotchas

- macOS 27: `hdiutil create` fails ("resource busy"), so `tauri build --bundles dmg` doesn't work. `mise run dmg` uses `diskutil image create` instead.
- The build is ad-hoc signed and not notarized (no Developer ID), and `arm64` only.
- The PR tab splits into two columns above 1512px of window width (a full-size 14" MacBook Pro window).
