# ghreviewer — MVP plan

Local-only macOS desktop app to triage and read GitHub PRs. Snappy, keyboard-first (vim), monospace, straight lines.

## Decisions

- App name `ghreviewer`, bundle id `dev.johnoppenheimer.ghreviewer`.
- git, one commit per milestone; work through all milestones, stop only on blockers.
- First run writes a starter config (rules `review` + `mine`, bots excluded, dark + light themes).
- Font: system monospace only (`ui-monospace, "SF Mono", Menlo, monospace`), nothing bundled.
- Tests: Rust unit tests on logic (rule merge/dedupe, filters, unread, config read/write) + `bun test` for the keyboard state machine. No e2e.

## Stack

- **Tauri 2** shell, **Rust** backend (owns GitHub access, cache, config).
- **Vite + React + TypeScript**, **bun**, **TanStack Query**, plain CSS + CSS variables.
- Rust crates: `reqwest` (rustls), `serde`/`serde_json`, `tokio`, `rusqlite` (bundled), `toml_edit` (keeps user comments when the app writes config), `notify` (config hot reload), `dirs`.
- Frontend: `dompurify` (sanitize GitHub HTML before rendering).
- Tooling: project-local `mise.toml` pins rust (+clippy, rustfmt) and bun, and defines tasks: `mise run dev | build | fmt | lint | test | install`. The Tauri CLI is a bun dev dependency, so it isn't installed globally.

## Auth & GitHub access

- On startup: `gh auth token` → keep the token in memory only. On a 401, run it again once.
- All calls go to **GraphQL** `https://api.github.com/graphql` from Rust. github.com only for the MVP.
- Show the remaining rate limit (`rateLimit { remaining resetAt }`) in a status bar.

## Config (source of truth)

`~/.config/ghreviewer/config.toml`, hot-reloaded. An in-app settings view edits the same file through `toml_edit`.

```toml
poll_interval_secs = 300
theme = "dark"

[[rules]]
name = "review"
query = "is:open review-requested:@me"

[[rules]]
name = "mine"
query = "is:open author:@me"

[filters] # local, applied after fetch, global to all rules
exclude_authors = ["dependabot[bot]", "renovate[bot]"]
exclude_repos = ["acme/legacy"]
hide_drafts = false

[repos]
pinned = ["acme/api"]
hidden = ["acme/old-thing"]

[themes.dark]
bg = "#0b0b0b"
fg = "#d4d4d4"
dim = "#6b6b6b"
accent = "#7aa2f7"
border = "#262626"
unread = "#e0af68"
ok = "#9ece6a"
fail = "#f7768e"
font = "ui-monospace, 'SF Mono', Menlo, monospace"

[themes.light]
# same keys
```

The app always adds `is:pr` to each rule's query.

## Data flow

```
gh auth token ─▶ Rust GraphQL client ─▶ SQLite cache ─▶ Tauri commands ─▶ TanStack Query ─▶ React
                         ▲                     │
                 poller (N secs, `r`)          └─ emits "data-updated" → frontend invalidates queries
```

Startup reads from the cache, so the UI shows data right away. A background refresh then updates it.

### Fetches

1. **Repos** (on launch and every hour): `viewer.repositories(affiliations: [OWNER, COLLABORATOR, ORGANIZATION_MEMBER])`, paginated.
2. **Inbox** (every poll): one `search(type: ISSUE, query: rule.query + " is:pr")` per rule, paginated. Combine the results, dedupe by PR id, and record which rules matched each PR. Apply the local filters.
   - Each PR fetches `timelineItems(last: 10)` with actor and timestamp, which gives `last_others_activity_at`, the latest activity whose actor isn't the viewer.
     shortcut: only checks the last 10 timeline items; widen it if unread status misses activity.
3. **PR detail** (when a PR is selected; shows the cached copy first, then refreshes): `bodyHTML`, author, labels, assignees, reviewRequests, latestReviews, mergeable/mergeStateStatus, head/base refs, last commit `statusCheckRollup` (checks), timelineItems (comments, reviews, commits, events; paginated), reviewThreads (path, line, isResolved, isOutdated, comments{bodyHTML, author, diffHunk}).

GitHub's own `bodyHTML` means no markdown renderer: output matches GitHub. Sanitize it with DOMPurify, and open links in the system browser.

### SQLite (`~/Library/Application Support/ghreviewer/cache.db`)

| table       | columns                                                                                             |
| ----------- | --------------------------------------------------------------------------------------------------- |
| `repos`     | id, owner, name, is_private, pushed_at                                                              |
| `prs`       | id, repo_id, number, title, author, is_draft, state, updated_at, last_others_activity_at, list_json |
| `pr_rules`  | pr_id, rule_name                                                                                    |
| `pr_detail` | pr_id, detail_json, fetched_at                                                                      |
| `pr_views`  | pr_id, last_viewed_at, forced_unread                                                                |

**Unread** means `last_others_activity_at > last_viewed_at` OR `forced_unread`. Opening a PR sets `last_viewed_at`; `u` toggles `forced_unread`.

## Tauri commands

`list_repos`, `list_inbox(scope: all | org | repo)`, `get_pr(id)`, `mark_viewed(id)`, `toggle_unread(id)`, `refresh()`, `get_config`, `save_config`, `set_repo_flag(repo, pinned|hidden)`, `open_url(url)`.

## UI

Three columns separated by 1px lines, no rounding, no shadows, everything monospace.

1. **Sources**: `All`, then orgs (collapsible) → repos. Repos with matching PRs come first, with an unread/total count. Pinned repos sit at the top of their org; hidden repos are not shown (a toggle reveals them).
2. **Inbox**: rows for the selected scope (All / org / repo), sorted by updated time. Each row shows: unread marker, `owner/repo#123`, title, author, `[rule]` tags, draft, check glyph (✓ ✗ ●), review state, relative time.
3. **PR pane**:
   - Header: title, state, branches, author, labels, reviewers + their state, checks summary, mergeability.
   - Description (bodyHTML).
   - Conversation timeline: comments, reviews (summary body + state), commits, notable events.
   - Review threads: `path:line`, a diff hunk snippet, then the comments. Resolved and outdated threads start collapsed.
4. **Status bar**: last sync, syncing indicator, rate limit, errors.
5. **Settings view** (`:settings`): edits rules, filters, poll interval, theme.

### Keys (fixed for MVP)

| key                        | action                                                                                   |
| -------------------------- | ---------------------------------------------------------------------------------------- |
| `j` / `k`                  | next / prev item in focused column                                                       |
| `h` / `l`, `Tab` / `S-Tab` | move focus between columns                                                               |
| `gg` / `G`                 | top / bottom                                                                             |
| `Ctrl-d` / `Ctrl-u`        | scroll PR pane                                                                           |
| `/`                        | filter focused list                                                                      |
| `o`                        | open PR in browser                                                                       |
| `Cmd-r`                    | refresh now (works everywhere; footer pulses green while syncing)                        |
| `u`                        | toggle unread                                                                            |
| `w`                        | open the PR's worktree with the first `[[open_with]]`, or create it (`wt switch pr:N`)   |
| `W`                        | pick any `[[open_with]]` command for the worktree (palette)                              |
| `[` / `]`                  | previous / next PR tab: the PR, then one per `[[prompts]]` entry                         |
| `{` / `}`                  | previous / next file tab of the prompt tab (window 1920px+)                              |
| `r`                        | run or re-run the tab's prompt headless (opencode / claude), creating the worktree first |
| `c`                        | continue the tab's agent session in a new terminal (`terminal`)                          |
| `p` / `x` (sidebar)        | pin / hide repo                                                                          |
| `e` (sidebar)              | hide repos without PRs                                                                   |
| `H`                        | show hidden repos                                                                        |
| `Space` (sidebar)          | collapse org                                                                             |
| `?`                        | help overlay                                                                             |
| `:`                        | command palette (refresh, settings, theme, open config file)                             |
| `Esc`                      | close overlay / clear filter                                                             |

One global keydown handler drives a small focus state (`focusedColumn`, a pending-key buffer for `gg`). It ignores keys while an input has focus.

## Milestones

0. **Setup**: `mise install` (project-local `mise.toml` pins rust + bun, nothing global), scaffold Tauri 2 + React-TS with bun, oxlint + oxfmt for frontend lint/format (bun dev deps).
1. **Shell**: 3-column layout, theme CSS vars, keyboard navigation on fake data.
2. **Backend core**: token from gh, GraphQL client, config load and hot reload, SQLite, repo fetch → column 1.
3. **Inbox**: rule searches, combine and tag, local filters, scopes, poller + events → column 2.
4. **PR pane**: detail query and cache, header/meta, description, timeline, review threads.
5. **Unread**: activity-by-others tracking, mark viewed, `u`.
6. **Settings & repo flags**: settings view writing TOML, pin/hide.
7. **Polish**: `?` help, `:` palette, status bar, error states (gh missing or not logged in).

## Status

All milestones are in; checked against real GitHub data.

- Beyond the plan: after each poll, the poller caches details for PRs that are new or changed (4 at a time), so opening a PR is usually instant.
- Repo/org counts and scope filtering are computed on the frontend (`src/model.ts`) from the inbox list. Local filters and unread are computed in Rust (`db.rs`).
- Live GraphQL check: `cd src-tauri && cargo test -- --ignored live --nocapture`.

## Out of scope (MVP)

Diff/files view, write actions (comment, approve), GitHub Enterprise, Linux/Windows, keymap remapping, OS notifications, per-rule filters, resizable columns, list virtualization.

## Risks

- Search returns at most 1000 results per query. Fine for "concerns me" rules, but a broad rule would be cut off.
- Images in private-repo `bodyHTML` may need auth to load. Accept broken images for the MVP.
- Fetching timeline data inside search adds GraphQL cost. Watch the rate limit with many rules and large result sets.
- "All accessible repos" can be in the thousands. The plain DOM list should be OK; add virtualization if it isn't.
