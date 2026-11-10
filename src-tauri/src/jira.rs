use crate::{config, local::login_command};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::Mutex;

const FIELDS: &str =
    "summary,status,issuetype,assignee,reporter,priority,labels,parent,description,comment";

/// The part of a Jira issue the Jira tab shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Ticket {
    pub key: String,
    pub url: String,
    pub summary: String,
    /// Issue type name: Story, Bug, Epic…
    pub kind: String,
    pub status: String,
    /// Jira's status category: `new`, `indeterminate` (in progress) or `done`.
    pub category: String,
    pub assignee: Option<String>,
    pub reporter: Option<String>,
    pub priority: Option<String>,
    pub labels: Vec<String>,
    pub parent: Option<Parent>,
    /// Rendered by Jira; untrusted, sanitized by the frontend.
    pub description_html: String,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Parent {
    pub key: String,
    pub url: String,
    pub summary: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Comment {
    pub author: String,
    pub created: String,
    pub body_html: String,
}

/// A ticket found in Jira for a PR, as the inbox shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TicketRef {
    pub key: String,
    pub category: String,
}

/// Ticket keys named in a PR title, in brackets or not: `[TI-12] fix`, `TI-1, TI-2: fix`.
pub fn keys(title: &str, prefixes: &[String]) -> Vec<String> {
    let prefixes: Vec<String> = prefixes
        .iter()
        .map(|p| p.trim().trim_end_matches('-').to_ascii_uppercase())
        .filter(|p| !p.is_empty())
        .collect();
    let mut keys = Vec::new();
    // Only these characters reach the request URL.
    for word in title.split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_') {
        let word = word.to_ascii_uppercase();
        let known = prefixes.iter().any(|p| {
            let number = word.strip_prefix(p).and_then(|r| r.strip_prefix('-'));
            number.is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        });
        if known && !keys.contains(&word) {
            keys.push(word);
        }
    }
    keys
}

/// The title's tickets that were found in Jira (`categories`: key to status category).
pub fn refs(
    title: &str,
    prefixes: &[String],
    categories: &HashMap<String, String>,
) -> Vec<TicketRef> {
    keys(title, prefixes)
        .into_iter()
        .filter_map(|key| {
            let category = categories.get(&key)?.clone();
            Some(TicketRef { key, category })
        })
        .collect()
}

fn parse(base: &str, v: &Value) -> Ticket {
    let text = |v: &Value, p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_string);
    let s = |p: &str| text(v, p);
    let list = |p: &str| v.pointer(p).and_then(Value::as_array);
    let key = s("/key").unwrap_or_default();
    // Jira renders `created` too ("2 days ago"), so dates come from the raw fields.
    let rendered = list("/renderedFields/comment/comments");
    let comments = list("/fields/comment/comments")
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, c)| Comment {
            author: text(c, "/author/displayName").unwrap_or_else(|| "?".into()),
            created: text(c, "/created").unwrap_or_default(),
            body_html: rendered
                .and_then(|r| text(r.get(i)?, "/body"))
                .unwrap_or_default(),
        })
        .collect();
    let parent = s("/fields/parent/key").map(|key| Parent {
        url: format!("{base}/browse/{key}"),
        key,
        summary: s("/fields/parent/fields/summary").unwrap_or_default(),
        kind: s("/fields/parent/fields/issuetype/name").unwrap_or_default(),
    });
    let labels = list("/fields/labels").into_iter().flatten();
    Ticket {
        url: format!("{base}/browse/{key}"),
        key,
        summary: s("/fields/summary").unwrap_or_default(),
        kind: s("/fields/issuetype/name").unwrap_or_default(),
        status: s("/fields/status/name").unwrap_or_default(),
        category: s("/fields/status/statusCategory/key").unwrap_or_default(),
        assignee: s("/fields/assignee/displayName"),
        reporter: s("/fields/reporter/displayName"),
        priority: s("/fields/priority/name"),
        labels: labels
            .filter_map(|l| l.as_str().map(str::to_string))
            .collect(),
        parent,
        description_html: s("/renderedFields/description").unwrap_or_default(),
        comments,
    }
}

/// The API token: `token`, else `$token_env` or `token_command`, both run in the login shell
/// (a GUI app doesn't get the shell's environment).
async fn resolve_token(cfg: &config::Jira) -> Result<String> {
    if !cfg.token.is_empty() {
        return Ok(cfg.token.clone());
    }
    let (args, what) = if cfg.token_env.is_empty() {
        (cfg.token_command.clone(), "`token_command`".to_string())
    } else {
        (
            vec!["printenv".into(), cfg.token_env.clone()],
            format!("${} in your login shell", cfg.token_env),
        )
    };
    ensure!(
        !args.is_empty(),
        "Jira: set `token`, `token_env` or `token_command` under [jira]"
    );
    let out = login_command(&args)
        .output()
        .await
        .with_context(|| format!("Jira: could not read {what}"))?;
    // The last line: a login file may print before the command runs.
    let out = String::from_utf8(out.stdout)?;
    let token = out.lines().last().unwrap_or_default().trim().to_string();
    ensure!(!token.is_empty(), "Jira: no token from {what}");
    Ok(token)
}

/// Jira Cloud REST client (email + API token).
pub struct Jira {
    http: reqwest::Client,
    /// The token that authenticated, with the config it came from.
    token: Mutex<Option<(config::Jira, String)>>,
}

impl Jira {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("ghreviewer")
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("http client"),
            token: Mutex::new(None),
        }
    }

    fn base(cfg: &config::Jira) -> Result<&str> {
        let base = cfg.url.trim_end_matches('/');
        // Anything else would send the token in clear.
        ensure!(
            base.starts_with("https://"),
            "Jira: `url` must start with https://"
        );
        Ok(base)
    }

    async fn get(&self, cfg: &config::Jira, token: &str, path: &str) -> Result<reqwest::Response> {
        let url = format!("{}/rest/api/3/{path}", Self::base(cfg)?);
        let req = self.http.get(url).basic_auth(&cfg.email, Some(token));
        Ok(req.send().await?)
    }

    /// Resolved once per config, and checked: with a bad token Jira answers some requests as an
    /// anonymous user, which would make every ticket look missing.
    async fn token(&self, cfg: &config::Jira) -> Result<String> {
        let mut cached = self.token.lock().await;
        if let Some((_, token)) = cached.as_ref().filter(|(c, _)| c == cfg) {
            return Ok(token.clone());
        }
        *cached = None;
        let token = resolve_token(cfg).await?;
        let status = self.get(cfg, &token, "myself").await?.status();
        ensure!(
            status.is_success(),
            "Jira: sign-in failed ({status}): check `email` and the API token"
        );
        *cached = Some((cfg.clone(), token.clone()));
        Ok(token)
    }

    /// One issue, `None` when it doesn't exist or you can't see it.
    pub async fn ticket(&self, cfg: &config::Jira, key: &str) -> Result<Option<Ticket>> {
        let token = self.token(cfg).await?;
        let path = format!("issue/{key}?fields={FIELDS}&expand=renderedFields");
        let res = self.get(cfg, &token, &path).await?;
        let status = res.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            *self.token.lock().await = None; // e.g. a rotated token: resolve it again next sync
        }
        if !status.is_success() {
            let text = res.text().await.unwrap_or_default();
            bail!(
                "Jira {status} on {key}: {}",
                text.chars().take(200).collect::<String>()
            );
        }
        let v: Value = res.json().await.context("unexpected Jira response")?;
        Ok(Some(parse(Self::base(cfg)?, &v)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_from_title() {
        let p = ["TI-".to_string(), "ops".to_string()];
        assert_eq!(keys("[TI-12] fix login", &p), ["TI-12"]);
        assert_eq!(keys("[ti-1, OPS-2][WIP] x [TI-1]", &p), ["TI-1", "OPS-2"]);
        assert_eq!(keys("TI-12: fix (ops-3)", &p), ["TI-12", "OPS-3"]);
        // Another project, no number, more than a number, or part of a longer word.
        assert!(keys("[ABC-1] TI- [TI-1a] XTI-3 TI-4-fix", &p).is_empty());
        assert!(keys("[TI-12]", &[]).is_empty());
        assert!(keys("[-12]", &["-".to_string()]).is_empty());

        let found = HashMap::from([("TI-1".to_string(), "done".to_string())]);
        let got = refs("[TI-1][TI-2]", &p, &found);
        assert_eq!(got.len(), 1);
        assert_eq!(
            (got[0].key.as_str(), got[0].category.as_str()),
            ("TI-1", "done")
        );
    }

    #[test]
    fn parses_an_issue() {
        let v = json!({
            "key": "TI-12",
            "fields": {
                "summary": "Fix login",
                "issuetype": { "name": "Bug" },
                "status": { "name": "In Review", "statusCategory": { "key": "indeterminate" } },
                "assignee": { "displayName": "Jane" },
                "reporter": null,
                "labels": ["auth"],
                "parent": { "key": "TI-1", "fields": { "summary": "Auth", "issuetype": { "name": "Epic" } } },
                "comment": { "comments": [
                    { "author": { "displayName": "Bob" }, "created": "2026-01-01T10:00:00.000+0100", "body": {} }
                ] }
            },
            "renderedFields": {
                "description": "<p>desc</p>",
                "comment": { "comments": [{ "created": "2 days ago", "body": "<p>hi</p>" }] }
            }
        });
        let t = parse("https://acme.atlassian.net", &v);
        assert_eq!(t.url, "https://acme.atlassian.net/browse/TI-12");
        assert_eq!((t.kind.as_str(), t.status.as_str()), ("Bug", "In Review"));
        assert_eq!(t.category, "indeterminate");
        assert_eq!((t.assignee.as_deref(), t.reporter), (Some("Jane"), None));
        assert_eq!(t.labels, ["auth"]);
        assert_eq!(t.parent.unwrap().kind, "Epic");
        assert_eq!(t.description_html, "<p>desc</p>");
        let c = &t.comments[0];
        assert_eq!(
            (c.author.as_str(), c.created.as_str(), c.body_html.as_str()),
            ("Bob", "2026-01-01T10:00:00.000+0100", "<p>hi</p>")
        );
        // An issue with nothing filled in.
        assert_eq!(parse("https://x", &json!({ "key": "A-1" })).key, "A-1");
    }

    #[tokio::test]
    async fn token_sources() {
        let mut cfg = config::Jira::default();
        assert!(resolve_token(&cfg).await.is_err());
        cfg.token_command = ["echo", "from-command"].map(String::from).to_vec();
        assert_eq!(resolve_token(&cfg).await.unwrap(), "from-command");
        cfg.token_env = "GHREVIEWER_NO_SUCH_VAR".into();
        assert!(resolve_token(&cfg).await.is_err());
        cfg.token = "inline".into();
        assert_eq!(resolve_token(&cfg).await.unwrap(), "inline");
        cfg.url = "http://acme.example".into();
        assert!(Jira::base(&cfg).is_err());
    }
}
