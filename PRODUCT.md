# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

Rendered in a Tauri 2 webview on macOS. The UI is a custom design language, not native macOS controls.

## Users

A developer who works in GitHub pull requests every day: first the author, later other developers once it is shared. They open it many times a day, between other work, to see which PRs need them and to catch up on what changed. They live in the keyboard and a terminal.

## Product Purpose

A local-only macOS desktop app to triage and read the GitHub PRs that concern the user. Success: in a few keystrokes the user knows what needs attention, has read the new activity, and has moved on, without opening the browser. Reviewing code (diffs, comments) stays on GitHub; `o` opens the PR there.

## Positioning

- **Keyboard-first (vim):** every action works without the mouse.
- **Instant:** a local SQLite cache shows data right away, a background poll refreshes it, and PR details are prefetched.
- **Rule-based inbox:** the user's own GitHub search rules plus local filters, instead of GitHub's notification noise.
- **Terminal aesthetic:** monospace, straight lines, no rounding, no shadows. This identity is binding.

## Operating Context

- Auth comes from the `gh` CLI (`gh auth token`); the token stays in memory. github.com only.
- Config lives in `~/.config/ghreviewer/config.toml`, hot-reloaded, and is the source of truth. The in-app settings view edits the same file and keeps the user's comments.
- Layout: three columns (sources, inbox, PR pane) plus a status bar, a `?` help overlay, a `:` command palette, and a settings view.
- Unread means someone else was active since the user last opened the PR, or the user forced it unread with `u`.

## Capabilities and Constraints

- Read-only, except approving a PR and posting an agent's findings as a review. No other write actions, diff/files view, GitHub Enterprise, Linux/Windows, keymap remapping, OS notifications, per-rule filters, resizable columns, or list virtualization in the MVP.
- PR content is GitHub's own `bodyHTML`, sanitized with DOMPurify. Links open in the system browser.
- Themes are user-defined in config through a fixed set of keys (`bg`, `fg`, `dim`, `accent`, `border`, `unread`, `ok`, `fail`, `font`). Dark and light ship by default.
- Font: system monospace only (`ui-monospace, "SF Mono", Menlo, monospace`); nothing bundled.
- Fixed keymap: see `README.md`.
- Known limits: search returns at most 1000 results per rule; images in private-repo bodies may not load; the rate limit is shown in the status bar.
- Open decision: how it will be distributed to other developers.

## Brand Commitments

- Name: `ghreviewer`, lowercase. Bundle id `dev.johnoppenheimer.ghreviewer`.
- Monospace everywhere, 1px lines, no rounding, no shadows (see Positioning).

## Evidence on Hand

- `README.md` and `AGENTS.md`: features, keymap and architecture.
- No users, testimonials, screenshots, or metrics beyond the author's own use. Do not invent them.

## Product Principles

1. Speed is the feature: show cached data first and never block on the network.
2. The keyboard is the main input; the mouse is optional.
3. Show what needs attention, not everything: unread, rules, and filters exist to cut noise.
4. Read here, act on GitHub: stay a fast reader, not a second GitHub.
5. The user owns the config: a plain file they can edit by hand, and the app respects it.
