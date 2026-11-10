use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use toml_edit::{Array, DocumentMut, Item, Table, value};

pub const STARTER: &str = r##"# ghreviewer config. Edit by hand or from the app (`:settings`); reloaded on save.

poll_interval_secs = 300
theme = "dark"

# Local clones: direct children of these folders are matched to GitHub repos by their
# `origin` remote. Used to show a PR's worktree and create one with worktrunk (`w`).
clone_roots = ["~/Documents/dev"]

# Opens a new terminal running {command} (a shell command line), to continue a prompt's
# agent session (`c`). Also gets {path} (the worktree) and {name} (its folder name).
terminal = ["osascript", "-e", "on run argv", "-e", 'tell application "Terminal" to do script (item 1 of argv)', "-e", 'tell application "Terminal" to activate', "-e", "end run", "{command}"]
# Rex instead (needs Rex's Remote Control on to switch to the new session):
# terminal = ["sh", "-c", 'rex="$HOME/Library/Application Support/com.superlogical.rex/RexServer/Versions/Current/Rex Server.app/Contents/MacOS/rex"; id=$("$rex" new "$1" -c "$2" --json -- zsh -lc "$3" | jq -r .session_id) && "$rex" do session.select session_id="$id"', "sh", "{name}", "{path}", "{command}"]

# Each rule is a GitHub search query (`is:pr` is added for you).
# PRs matching any rule land in the inbox, tagged with the rule name.
[[rules]]
name = "review"
query = "is:open review-requested:@me"

[[rules]]
name = "mine"
query = "is:open author:@me"

# Local filters applied on top of the rules. `*` is a wildcard.
[filters]
exclude_authors = ["*[bot]"]
exclude_repos = []
hide_drafts = false

# Sidebar only: pin to the top of the org, or hide (`p` / `x` in the app).
[repos]
pinned = []
hidden = []

# Ways to open a PR's worktree: `w` (or clicking the folder) uses the first one,
# `W` lists them all. `command` runs through your login shell without shell parsing;
# in each argument {path}, {name} (folder name), {branch}, {repo} and {number} are replaced.
[[open_with]]
name = "Cursor"
command = ["open", "-a", "Cursor", "{path}"]

# A new Rex session in the worktree, shown in the Rex app (needs Rex's Remote Control on).
# Rex's CLI is only on PATH inside Rex terminals, hence the full path.
# [[open_with]]
# name = "Rex"
# command = ["sh", "-c", 'rex="$HOME/Library/Application Support/com.superlogical.rex/RexServer/Versions/Current/Rex Server.app/Contents/MacOS/rex"; id=$("$rex" new "$1" -c "$2" --json | jq -r .session_id) && "$rex" do session.select session_id="$id"', "sh", "{name}", "{path}"]

# Prompts a coding agent runs headless in a PR's worktree (created first if needed).
# Each gets a tab in the PR pane: `[` / `]` switch tabs, `r` runs or re-runs the tab's prompt,
# `c` continues its session in `terminal`. harness is "opencode" or "claude"; `args` are
# extra flags for it (model, permissions…). In `prompt`, {url} {repo} {number} {branch} {base}
# {path} (the worktree) and {out} (a folder kept per PR and prompt) are replaced.
# .html/.svg/.png files in {out} (emptied on each run) or mentioned in the answer show in the tab.
# {review_format} asks the agent for findings you then pick and post as a GitHub review, e.g.
# prompt = "/pr-review {url}\n\n{review_format}"
# Agents read PR text written by others and act with your rights: allow only what a prompt needs
# (claude: --allowedTools as below; opencode: `deny` rules such as `gh pr review *` in its config).
# [[prompts]]
# name = "diagram"
# harness = "claude"
# prompt = "Use the diagram-design skill to draw what PR {url} changes and the author's intent. Save it as {out}/diagram.html and mention that path."
# args = ["--permission-mode", "acceptEdits", "--allowedTools", "Bash(git *)", "Bash(gh pr view *)"]

# Jira Cloud (optional). PR titles naming a ticket, like `[TI-123] Fix login` or `TI-123: fix login`,
# get a `Jira` tab with the ticket, and its key is colored by status in the inbox.
# `prefixes` are your project keys. The API token (https://id.atlassian.com/manage-profile/security/api-tokens)
# is either `token` or `token_env` (a variable your shell exports, e.g. in ~/.zshrc).
# [jira]
# url = "https://acme.atlassian.net"
# email = "you@acme.com"
# prefixes = ["TI"]
# token_env = "JIRA_API_TOKEN"

[themes.dark]
bg = "#0b0b0b"
bg_alt = "#151515"
fg = "#d4d4d4"
dim = "#6b6b6b"
border = "#262626"
sel = "#1f2a44"
accent = "#7aa2f7"
unread = "#e0af68"
ok = "#9ece6a"
fail = "#f7768e"
warn = "#e0af68"
font = "ui-monospace, 'SF Mono', Menlo, monospace"
font_size = "12px"

[themes.light]
bg = "#fbfbfa"
bg_alt = "#f0f0ee"
fg = "#1f1f1f"
dim = "#8a8a8a"
border = "#dcdcda"
sel = "#dbe5fb"
accent = "#2f5fd0"
unread = "#b26b00"
ok = "#2e7d32"
fail = "#c62828"
warn = "#b26b00"
font = "ui-monospace, 'SF Mono', Menlo, monospace"
font_size = "12px"
"##;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub poll_interval_secs: u64,
    pub theme: String,
    pub clone_roots: Vec<String>,
    pub rules: Vec<Rule>,
    pub filters: Filters,
    pub repos: RepoFlags,
    pub open_with: Vec<Opener>,
    pub terminal: Vec<String>,
    pub prompts: Vec<Prompt>,
    pub jira: Jira,
    pub themes: BTreeMap<String, BTreeMap<String, String>>,
}

/// Optional Jira Cloud link: tickets named in PR titles as `[TI-123]` get a tab in the PR pane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Jira {
    pub url: String,
    pub email: String,
    /// Project keys, e.g. `TI`.
    pub prefixes: Vec<String>,
    /// Never sent to the webview. From the settings view, empty means "keep the current one".
    #[serde(skip_serializing)]
    pub token: String,
    pub token_env: String,
}

impl Jira {
    pub fn enabled(&self) -> bool {
        !self.url.is_empty() && !self.prefixes.is_empty()
    }
}

/// A prompt a coding agent runs headless on a PR; its answer gets a tab in the PR pane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prompt {
    pub name: String,
    pub harness: Harness,
    pub prompt: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    #[default]
    Opencode,
    Claude,
}

/// A command that opens a worktree folder, e.g. an editor or a terminal session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Opener {
    pub name: String,
    pub command: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            poll_interval_secs: 300,
            theme: "dark".into(),
            clone_roots: vec!["~/Documents/dev".into()],
            open_with: vec![Opener {
                name: "Cursor".into(),
                command: ["open", "-a", "Cursor", "{path}"].map(Into::into).to_vec(),
            }],
            terminal: [
                "osascript",
                "-e",
                "on run argv",
                "-e",
                r#"tell application "Terminal" to do script (item 1 of argv)"#,
                "-e",
                r#"tell application "Terminal" to activate"#,
                "-e",
                "end run",
                "{command}",
            ]
            .map(Into::into)
            .to_vec(),
            prompts: vec![],
            jira: Jira::default(),
            rules: vec![],
            filters: Filters::default(),
            repos: RepoFlags::default(),
            themes: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Rule {
    pub name: String,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Filters {
    pub exclude_authors: Vec<String>,
    pub exclude_repos: Vec<String>,
    pub hide_drafts: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RepoFlags {
    pub pinned: Vec<String>,
    pub hidden: Vec<String>,
}

/// The part of the config the settings view edits.
#[derive(Debug, Deserialize)]
pub struct Settings {
    pub poll_interval_secs: u64,
    pub theme: String,
    pub rules: Vec<Rule>,
    pub filters: Filters,
    #[serde(default)]
    pub jira: Jira,
}

pub fn path() -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(".config/ghreviewer/config.toml")
}

/// Writes the starter config if there is none yet. Startup only, never on reload.
pub fn ensure(path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path.parent().context("bad config path")?)?;
        write(path, STARTER)?;
    }
    Ok(())
}

/// Reads the config. `None` when the file is missing or empty, e.g. while an editor saves it.
pub fn load(path: &Path) -> Result<Option<Config>> {
    let text = match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        res => res?,
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    toml::from_str(&text)
        .map(Some)
        .with_context(|| format!("invalid {}", path.display()))
}

/// Replaces the file in one step so the watcher never reads it half-written.
pub fn write(path: &Path, text: &str) -> Result<()> {
    // Into the file a symlink (e.g. from a dotfiles repo) points at, not over the link.
    let path = &std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn strings(items: &[String]) -> Array {
    items.iter().map(String::as_str).collect()
}

/// Writes settings into the TOML text, keeping the user's comments and layout where possible.
pub fn apply_settings(text: &str, s: &Settings) -> Result<String> {
    let mut doc: DocumentMut = text.parse()?;
    doc["poll_interval_secs"] = value(s.poll_interval_secs as i64);
    doc["theme"] = value(&s.theme);

    let rules = doc
        .entry("rules")
        .or_insert(Item::ArrayOfTables(Default::default()))
        .as_array_of_tables_mut()
        .context("`rules` must be written as [[rules]]")?;
    while rules.len() > s.rules.len() {
        rules.remove(rules.len() - 1);
    }
    for (i, rule) in s.rules.iter().enumerate() {
        if i == rules.len() {
            rules.push(Table::new());
        }
        let t = rules.get_mut(i).unwrap();
        t["name"] = value(&rule.name);
        t["query"] = value(&rule.query);
    }

    let f = &mut doc["filters"];
    f["exclude_authors"] = value(strings(&s.filters.exclude_authors));
    f["exclude_repos"] = value(strings(&s.filters.exclude_repos));
    f["hide_drafts"] = value(s.filters.hide_drafts);

    let j = &s.jira;
    let filled = !j.url.is_empty() || !j.email.is_empty() || !j.prefixes.is_empty();
    // No empty [jira] section for those who don't use it.
    if doc.contains_key("jira") || filled || !j.token.is_empty() || !j.token_env.is_empty() {
        let t = doc["jira"]
            .or_insert(toml_edit::table())
            .as_table_mut()
            .context("`jira` must be written as [jira]")?;
        t["url"] = value(&j.url);
        t["email"] = value(&j.email);
        t["prefixes"] = value(strings(&j.prefixes));
        if j.token_env.is_empty() {
            t.remove("token_env");
        } else {
            t["token_env"] = value(&j.token_env);
        }
        if !j.token.is_empty() {
            t["token"] = value(&j.token);
        }
    }

    let out = doc.to_string();
    toml::from_str::<Config>(&out).context("settings produced an invalid config")?;
    Ok(out)
}

/// Adds or removes `repo` from `[repos].pinned` / `[repos].hidden`.
pub fn set_repo_flag(text: &str, repo: &str, flag: &str, on: bool) -> Result<String> {
    if flag != "pinned" && flag != "hidden" {
        bail!("unknown repo flag {flag}");
    }
    let mut doc: DocumentMut = text.parse()?;
    let arr = doc["repos"][flag]
        .or_insert(value(Array::new()))
        .as_array_mut()
        .with_context(|| format!("`repos.{flag}` must be an array"))?;
    arr.retain(|v| v.as_str() != Some(repo));
    if on {
        arr.push(repo);
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_parses() {
        let c: Config = toml::from_str(STARTER).unwrap();
        assert_eq!(c.rules.len(), 2);
        assert_eq!(c.filters.exclude_authors, vec!["*[bot]"]);
        assert!(c.themes.contains_key("dark") && c.themes.contains_key("light"));
        assert_eq!(c.open_with, Config::default().open_with);
        // Configs written before these keys existed get the defaults.
        let old: Config = toml::from_str("theme = \"dark\"").unwrap();
        assert_eq!(old.clone_roots, vec!["~/Documents/dev"]);
        assert_eq!(old.open_with[0].name, "Cursor");
        // The commented Rex example must stay valid TOML once uncommented.
        let rex = STARTER
            .lines()
            .skip_while(|l| !l.starts_with("# [[open_with]]"))
            .take(3)
            .map(|l| l.trim_start_matches("# "))
            .collect::<Vec<_>>()
            .join("\n");
        let rex: Config = toml::from_str(&rex).unwrap();
        assert_eq!(rex.open_with[0].command[0], "sh");
        assert_eq!(c.terminal, Config::default().terminal);
        assert!(c.prompts.is_empty());
        let uncomment = |start: &str, n| {
            let text = STARTER
                .lines()
                .skip_while(|l| !l.starts_with(start))
                .take(n)
                .map(|l| l.trim_start_matches("# "))
                .collect::<Vec<_>>()
                .join("\n");
            toml::from_str::<Config>(&text).unwrap()
        };
        let p = uncomment("# [[prompts]]", 5).prompts.remove(0);
        assert_eq!((p.harness, p.args.len()), (Harness::Claude, 5));
        assert_eq!(uncomment("# terminal", 1).terminal.len(), 7);
        assert!(!c.jira.enabled());
        let j = uncomment("# [jira]", 5).jira;
        assert!(j.enabled() && j.token_env == "JIRA_API_TOKEN");
    }

    #[test]
    fn ensure_load_write() {
        let dir = std::env::temp_dir().join(format!("ghreviewer-test-{}", std::process::id()));
        let p = dir.join("sub/config.toml");
        assert!(load(&p).unwrap().is_none());
        ensure(&p).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), STARTER);
        assert_eq!(load(&p).unwrap().unwrap().theme, "dark");
        write(&p, "theme = \"light\"").unwrap();
        ensure(&p).unwrap();
        assert_eq!(load(&p).unwrap().unwrap().theme, "light");
        write(&p, "  \n").unwrap();
        assert!(load(&p).unwrap().is_none());
        assert!(!p.with_extension("toml.tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn settings_keep_comments() {
        let s = Settings {
            poll_interval_secs: 60,
            theme: "light".into(),
            rules: vec![Rule {
                name: "team".into(),
                query: "is:open team-review-requested:acme/x".into(),
            }],
            filters: Filters {
                exclude_authors: vec![],
                exclude_repos: vec!["a/b".into()],
                hide_drafts: true,
            },
            jira: Jira::default(),
        };
        let out = apply_settings(STARTER, &s).unwrap();
        assert!(out.contains("# Each rule is a GitHub search query"));
        assert!(out.contains("[themes.dark]"));
        let c: Config = toml::from_str(&out).unwrap();
        assert_eq!(c.poll_interval_secs, 60);
        assert_eq!(c.rules, s.rules);
        assert_eq!(c.filters, s.filters);
        assert!(!out.contains("\n[jira]"));

        // Jira: an empty token keeps the one in the file.
        let file = "[jira] # mine\ntoken = \"old\"\ntoken_env = \"X\"\n";
        let mut s = s;
        s.jira = Jira {
            url: "https://acme.atlassian.net".into(),
            email: "me@acme.com".into(),
            prefixes: vec!["TI".into()],
            ..Jira::default()
        };
        let out = apply_settings(file, &s).unwrap();
        assert!(out.contains("# mine"));
        let j = toml::from_str::<Config>(&out).unwrap().jira;
        assert_eq!((j.token.as_str(), j.token_env.as_str()), ("old", ""));
        assert_eq!(j.prefixes, ["TI"]);
        s.jira.token = "new".into();
        let out = apply_settings(&out, &s).unwrap();
        assert_eq!(toml::from_str::<Config>(&out).unwrap().jira.token, "new");
        // The webview never gets the token.
        assert!(!serde_json::to_string(&j).unwrap().contains("old"));
    }

    #[test]
    fn settings_on_empty_file() {
        let s = Settings {
            poll_interval_secs: 120,
            theme: "dark".into(),
            rules: vec![
                Rule {
                    name: "a".into(),
                    query: "b".into(),
                },
                Rule {
                    name: "c".into(),
                    query: "d".into(),
                },
            ],
            filters: Filters::default(),
            jira: Jira::default(),
        };
        let c: Config = toml::from_str(&apply_settings("", &s).unwrap()).unwrap();
        assert_eq!(c.rules.len(), 2);
    }

    #[test]
    fn repo_flags_toggle() {
        let on = set_repo_flag(STARTER, "o/r", "pinned", true).unwrap();
        let on = set_repo_flag(&on, "o/r", "pinned", true).unwrap();
        let c: Config = toml::from_str(&on).unwrap();
        assert_eq!(c.repos.pinned, vec!["o/r"]);
        let off = set_repo_flag(&on, "o/r", "pinned", false).unwrap();
        assert!(
            toml::from_str::<Config>(&off)
                .unwrap()
                .repos
                .pinned
                .is_empty()
        );
        assert!(
            set_repo_flag("", "o/r", "hidden", true)
                .unwrap()
                .contains("o/r")
        );
        assert!(set_repo_flag(STARTER, "o/r", "nope", true).is_err());
    }
}
