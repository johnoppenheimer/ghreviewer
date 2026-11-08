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
    pub rules: Vec<Rule>,
    pub filters: Filters,
    pub repos: RepoFlags,
    pub themes: BTreeMap<String, BTreeMap<String, String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            poll_interval_secs: 300,
            theme: "dark".into(),
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
}

pub fn path() -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(".config/ghreviewer/config.toml")
}

/// Reads the config, writing the starter one first if the file doesn't exist.
pub fn load(path: &Path) -> Result<Config> {
    if !path.exists() {
        std::fs::create_dir_all(path.parent().context("bad config path")?)?;
        std::fs::write(path, STARTER)?;
    }
    let text = std::fs::read_to_string(path)?;
    toml::from_str(&text).with_context(|| format!("invalid {}", path.display()))
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
    }

    #[test]
    fn load_writes_starter() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/config.toml");
        assert_eq!(load(&p).unwrap().theme, "dark");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), STARTER);
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
        };
        let out = apply_settings(STARTER, &s).unwrap();
        assert!(out.contains("# Each rule is a GitHub search query"));
        assert!(out.contains("[themes.dark]"));
        let c: Config = toml::from_str(&out).unwrap();
        assert_eq!(c.poll_interval_secs, 60);
        assert_eq!(c.rules, s.rules);
        assert_eq!(c.filters, s.filters);
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
