# ghreviewer

A local macOS app for triaging and reading the GitHub pull requests that concern you. It has three columns like a mail client, is driven by the keyboard (vim keys), and looks like a terminal.

- **Your own inbox.** PRs come from GitHub searches you define ("review requested from me", "mine", ...), with local filters on top, instead of GitHub's notifications.
- **Unread tracking.** A PR is unread when someone else did something on it after you last opened it.
- **Stacks.** Stacked PRs show as a tree. Parents that aren't in your inbox show greyed out.
- **Worktrees.** See whether a PR is checked out locally, create a worktree for it, and open it in your editor or terminal.
- **Agent prompts.** Run your own prompts with [opencode](https://opencode.ai) or [Claude Code](https://claude.com/claude-code) on a PR's worktree, and read the answer (with any HTML, SVG or image files it makes) in a tab next to the PR. A review prompt can return findings you pick and post as a GitHub review.

Reading diffs and replying to threads stays on GitHub: `o` opens the PR there. The app only writes to GitHub when you approve a PR or post a review from an agent's findings.

Everything runs on your Mac. The app talks to the GitHub API directly with your `gh` login and keeps a local cache. There is no server and no account.

## Requirements

- macOS on Apple Silicon.
- [GitHub CLI](https://cli.github.com), logged in: `gh auth login`. It's the only hard requirement.
- Optional:
  - [worktrunk](https://github.com/max-sixty/worktrunk) (`wt`) to create worktrees from the app.
  - `opencode` or `claude` for agent prompts.

The app finds these tools through your login shell's `PATH`. A login shell doesn't read `~/.zshrc`, so if a tool isn't found, set its `PATH` in `~/.zprofile`.

## Install

### From a .dmg

Open the `.dmg` and drag `ghreviewer` onto Applications.

The app is not signed with an Apple Developer ID, so macOS blocks it the first time. Either run this once:

```sh
xattr -dr com.apple.quarantine /Applications/ghreviewer.app
```

or try to open it, then go to System Settings → Privacy & Security → **Open Anyway**.

### From source

You need [mise](https://mise.jdx.dev), which installs the pinned Rust and Bun versions.

```sh
mise install
mise run install   # frontend dependencies
mise run build     # src-tauri/target/release/bundle/macos/ghreviewer.app
mise run dmg       # ...and a .dmg in src-tauri/target/release/bundle/dmg/
```

## Using it

On first launch the app writes a commented starter config to `~/.config/ghreviewer/config.toml`, with two rules: PRs where your review is requested, and your own open PRs. It then syncs.

| Column | What it shows                                                                                                                       |
| ------ | ----------------------------------------------------------------------------------------------------------------------------------- |
| Repos  | All, then each org and its repos with PR counts (unread/total). Below them: agent runs still going or not yet read.                 |
| Inbox  | PRs in the selected scope, newest first, grouped into stacks. Each shows its rules (`[review]`, `[mine]`), review state and checks. |
| PR     | Description, conversation, commits and review threads. One more tab per configured prompt.                                          |

- **Syncing.** The inbox syncs every `poll_interval_secs` (and on `Cmd-R`). The repo list syncs hourly. Details of new unread PRs are fetched ahead of time, so opening them is instant.
- **Unread.** Opening a PR marks it read. `u` toggles it.
- **Approving.** On the PR tab, `a` (or `approve` after the reviewers, or `approve PR` in the `:` palette) asks for an optional message, then `⌘↵` approves the PR on GitHub. It's offered on other people's open PRs until you've approved their latest commit.
- **Wide windows.** In a window wider than a full-size 14" MacBook Pro screen, the PR tab splits: description on the left, conversation on the right.

### Keys

Press `?` in the app for this list, and `:` for the command palette (every action, plus settings and themes).

| Key                 | Action                                                                             |
| ------------------- | ---------------------------------------------------------------------------------- |
| `j` / `k`           | next / previous (findings, in a prompt tab)                                        |
| `h` / `l`, `Tab`    | previous / next column (`Enter` too)                                               |
| `gg` / `G`          | top / bottom                                                                       |
| `Ctrl-d` / `Ctrl-u` | scroll the PR half a page                                                          |
| `/`, `Esc`          | filter the focused list, clear it                                                  |
| `o`                 | open the PR in the browser                                                         |
| `u`                 | toggle unread                                                                      |
| `a`                 | approve the PR, with an optional message (PR tab)                                  |
| `t`                 | show all rules in the inbox, then only each rule in turn                           |
| `Cmd-R`             | sync now                                                                           |
| `w` / `W`           | open the PR's worktree (or create it) / choose how to open                         |
| `[` / `]`           | previous / next PR tab (the PR, Jira, review bots, then each prompt)               |
| `{` / `}`           | previous / next file of the prompt tab (window 1920px+), or ticket of the Jira tab |
| `r`                 | run or re-run the tab's prompt                                                     |
| `c`                 | continue the tab's agent session in a terminal                                     |
| `f`                 | send the picked findings to the `[fix]` agent                                      |
| `p` / `x`           | pin / hide a repo in the sidebar                                                   |
| `H`, `e`, `Space`   | show hidden repos, hide repos without PRs, collapse an org                         |
| `Space`             | pick or drop a finding (prompt tab)                                                |

### Stacks

A PR whose base branch is another open PR's branch sits under it in the tree. If a PR in your inbox is stacked on PRs that aren't (no rule matches them, or your filters drop them), the app fetches those parents and shows them greyed out so you see the whole stack. A filter in the Inbox column does the same: matches stay bright, their parents come along greyed.

### Worktrees

The app looks for a local clone of each PR's repo among the direct children of your `clone_roots` folders, matching on the `origin` remote. The PR header then shows:

- the worktree folder, if the PR's branch is checked out somewhere. `w` opens it with your first `open_with` command, and `W` lists them all.
- `+ worktree`, if you have a clone and `wt` is installed. `w` runs `wt switch pr:<number>` in that clone.

### Agent prompts

Each `[[prompts]]` entry in the config adds a tab to the PR pane. `r` runs the prompt headless with its harness in the PR's worktree (created first if needed). The tab shows live progress, then the answer.

- **Files.** HTML, SVG and image files the agent saves in `{out}`, or mentions in its answer, show in the tab. In a window 1920px or wider, they sit right of the answer, one tab per file (`{` / `}`).
- **Results** are kept across restarts until you re-run. A run made on an older commit than the PR's current head is marked `*`.
- **Continuing.** `c` opens the agent's session in your `terminal` so you can carry on the conversation.
- **Quitting.** Runs stop if you quit the app. You can still continue them with `c`.

#### Posting a review

Put `{review_format}` in a prompt, and the agent ends its answer with its findings as structured data. The tab lists them above the answer, each exactly as it will be posted.

- **Picking.** Blockers and should-fix findings start ticked, nits don't. Click `[x]`, or step with `j` / `k` and press `Space`.
- **Posting.** `comment` or `approve` (also in the `:` palette) posts the picked findings as one review. A finding with a file and line becomes an inline comment there. The others are listed in the review body, after the agent's summary.
- **Lines outside the diff.** GitHub rejects the whole review if one inline comment points outside the diff. The app then posts again with every finding in the body, and says so.
- **Stale runs.** Comments go on the commit the agent read, so their lines stay right after new pushes.
- **Once.** A run's review is posted once. Re-run the prompt for a new one.

#### Review bots

A PR with open review threads from Greptile (`greptile-apps`) or CodeRabbit (`coderabbitai`) gets a `greptile` or `coderabbit` tab listing them as findings, with Greptile's priority (`P0`–`P2`; `P2` starts unticked). Resolved threads are left out, and outdated ones are marked. They're on GitHub already, so these tabs only have `fix`.

#### Fixing findings

With a `[fix]` section in the config, `fix` (or `f`) hands the picked findings to that agent in the PR's worktree, before or after posting them. It runs in a `fix` tab like a prompt: live progress, `c` to continue in a terminal. Each fix replaces the previous one; pick findings and press `f` again to re-run it.

Agents act with your permissions and read PR text written by other people. Allow each prompt only what it needs: `--allowedTools` in `args` for Claude, and `deny` rules for opencode in its own config.

## Configuration

Everything lives in `~/.config/ghreviewer/config.toml`. The app reloads it as soon as you save it. `:settings` edits the poll interval, theme, rules, filters and Jira in the same file and keeps your comments. A broken file shows an error in the status bar and the app keeps the last good config.

```toml
poll_interval_secs = 300 # minimum 30
theme = "dark" # a name under [themes]
clone_roots = ["~/Documents/dev"] # folders holding your clones
```

### Rules and filters

Each rule is a [GitHub search](https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests) (`is:pr` is added for you). A PR matching any rule lands in the inbox, tagged with the names of the rules it matched.

```toml
[[rules]]
name = "review"
query = "is:open review-requested:@me"

[[rules]]
name = "mine"
query = "is:open author:@me"

# Applied locally on top of the rules. `*` is a wildcard.
[filters]
exclude_authors = ["*[bot]"]
include_repos = ["acme/*"] # if set, only these repos
exclude_repos = ["acme/legacy-*"]
hide_drafts = false

# Sidebar only. Also set with `p` / `x`.
[repos]
pinned = ["acme/api"]
hidden = []
```

### Opening worktrees

`open_with` commands run through your login shell without shell parsing. In each argument, `{path}`, `{name}` (folder name), `{branch}`, `{repo}` and `{number}` are replaced.

```toml
[[open_with]]
name = "Cursor"
command = ["open", "-a", "Cursor", "{path}"]

[[open_with]]
name = "Ghostty"
command = ["open", "-a", "Ghostty", "{path}"]
```

`terminal` opens a new terminal running `{command}` (a shell command line). It's used to continue an agent session. It also gets `{path}` and `{name}`. The default uses Terminal.app, and the starter config has a commented example for Rex.

### Prompts

`harness` is `opencode` or `claude`, and `args` are extra flags for it (model, permissions, ...). In `prompt`, these are replaced:

| Placeholder       | Replaced with                                                           |
| ----------------- | ----------------------------------------------------------------------- |
| `{url}`           | the PR's URL                                                            |
| `{repo}`          | the repo                                                                |
| `{number}`        | the PR number                                                           |
| `{branch}`        | the PR's branch                                                         |
| `{base}`          | the branch it merges into                                               |
| `{path}`          | the worktree                                                            |
| `{out}`           | a folder kept per PR and prompt, emptied before each run                |
| `{review_format}` | instructions to return findings ([Posting a review](#posting-a-review)) |

```toml
[[prompts]]
name = "review"
harness = "opencode"
prompt = "Review PR {url}.\n\n{review_format}"

[[prompts]]
name = "diagram"
harness = "claude"
prompt = "Draw what PR {url} changes as {out}/diagram.html and mention that path."
args = ["--permission-mode", "acceptEdits", "--allowedTools", "Bash(git *)", "Bash(gh pr view *)"]
```

### Fix

`[fix]` takes the same `harness`, `prompt` and `args` as a prompt. In `prompt`, `{findings}` is the picked findings (title, severity, `path:line` and body), and the placeholders above work too.

```toml
[fix]
harness = "claude"
prompt = "Fix these review findings on {branch}, one commit per fix. Don't push.\n\n{findings}"
args = ["--permission-mode", "acceptEdits", "--allowedTools", "Bash(git *)"]
```

### Jira

Optional, Jira Cloud only. A PR whose title names a ticket, like `[TI-123] Fix login` or `TI-123: fix login` (several work too), gets a `Jira` tab: type, status, assignee, reporter, priority, parent and labels, with the description on the left and the comments on the right in a wide window. With several tickets, `{` / `}` switch between them. In the inbox, the key takes the color of its status: dim for to do, blue for in progress, green for done.

Tickets are fetched at each sync, after GitHub (the status bar turns blue for that step). A Jira change never marks a PR unread. Images in a ticket show as links to Jira.

```toml
[jira]
url = "https://acme.atlassian.net"
email = "you@acme.com"
prefixes = ["TI"] # your project keys
# The API token, either:
token_env = "JIRA_API_TOKEN" # a variable your shell exports (e.g. in ~/.zshrc)
# token = "..."
```

Create the token at <https://id.atlassian.com/manage-profile/security/api-tokens>. `:settings` edits all of these.

### Themes

`dark` and `light` come with the starter config. Add your own under `[themes.<name>]` with the same keys: `bg`, `bg_alt`, `fg`, `dim`, `border`, `sel`, `accent`, `unread`, `ok`, `fail`, `warn`, `font` and `font_size`. Then pick it with `theme =` or from the command palette.

## Data and privacy

- **Token.** The GitHub token comes from `gh auth token` and is kept in memory only. The Jira token, if you wrote it as `token`, is in the config file in clear: prefer `token_env`.
- **Cache.** The inbox, PR details, read state and agent results live in `~/Library/Application Support/ghreviewer/cache.db`. Agent output folders are in `runs/` next to it. Deleting them is safe: the app re-syncs, but read state and past results are lost.
- **Network.** The app only talks to `api.github.com` and, when set, your Jira site, and only reads, except when you approve a PR or post a review. Agents talk to whatever their harness uses.

## Development

The app is Tauri 2: a Rust backend in `src-tauri/` (GitHub GraphQL, sync loop, SQLite, worktrees, agents) and a React + TypeScript frontend in `src/` (TanStack Query, plain CSS).

```sh
mise run dev    # run with hot reload
mise run test   # cargo test + bun test
mise run lint   # clippy, oxlint, oxfmt --check, tsc
mise run fmt    # cargo fmt + oxfmt
```

`cd src-tauri && cargo test -- --ignored live --nocapture` runs a sync against GitHub with your `gh` login.
