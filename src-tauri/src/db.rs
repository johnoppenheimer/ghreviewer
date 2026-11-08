use crate::{
    config::Filters,
    sync::{PrSummary, Repo},
};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::{collections::HashMap, path::Path, sync::Mutex};

pub struct Db(Mutex<Connection>);

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS repos (name TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS prs (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS pr_detail (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS pr_views (
  id TEXT PRIMARY KEY,
  seen_activity_at TEXT,
  forced_unread INTEGER NOT NULL DEFAULT 0
);";

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self(Mutex::new(conn)))
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.0.lock().unwrap()
    }

    pub fn replace_repos(&self, repos: &[Repo]) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM repos", [])?;
        for r in repos {
            tx.execute(
                "INSERT INTO repos (name, json) VALUES (?1, ?2)",
                params![r.name_with_owner, serde_json::to_string(r)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn repos(&self) -> Result<Vec<Repo>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT json FROM repos ORDER BY name")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|j| Ok(serde_json::from_str(&j?)?)).collect()
    }

    /// Replaces the inbox with a fresh sync and drops cached details of PRs that left it.
    pub fn replace_prs(&self, prs: &[PrSummary]) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM prs", [])?;
        for pr in prs {
            tx.execute(
                "INSERT INTO prs (id, json) VALUES (?1, ?2)",
                params![pr.id, serde_json::to_string(pr)?],
            )?;
        }
        tx.execute("DELETE FROM pr_detail WHERE id NOT IN (SELECT id FROM prs)", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Inbox after local filters, with unread computed, newest first.
    pub fn inbox(&self, filters: &Filters) -> Result<Vec<PrSummary>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id, seen_activity_at, forced_unread FROM pr_views")?;
        let views: HashMap<String, (Option<String>, bool)> = stmt
            .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = conn.prepare("SELECT json FROM prs")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut prs = Vec::new();
        for json in rows {
            let mut pr: PrSummary = serde_json::from_str(&json?)?;
            if !passes(filters, &pr) {
                continue;
            }
            let (seen, forced) = views.get(&pr.id).cloned().unwrap_or_default();
            pr.unread = is_unread(pr.last_others_activity_at.as_deref(), seen.as_deref(), forced);
            prs.push(pr);
        }
        prs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(prs)
    }

    pub fn detail(&self, id: &str) -> Result<Option<Value>> {
        let json: Option<String> = self
            .conn()
            .query_row("SELECT json FROM pr_detail WHERE id = ?1", [id], |r| r.get(0))
            .optional()?;
        Ok(json.map(|j| serde_json::from_str(&j)).transpose()?)
    }

    pub fn put_detail(&self, id: &str, detail: &Value) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO pr_detail (id, json) VALUES (?1, ?2)",
            params![id, detail.to_string()],
        )?;
        Ok(())
    }

    /// Unread: force it. Read: remember the latest activity seen so only newer activity flips it back.
    pub fn set_unread(&self, id: &str, unread: bool) -> Result<()> {
        let sql = if unread {
            "INSERT INTO pr_views (id, forced_unread) VALUES (?1, 1)
             ON CONFLICT(id) DO UPDATE SET forced_unread = 1"
        } else {
            "INSERT INTO pr_views (id, seen_activity_at, forced_unread)
             VALUES (?1, (SELECT json_extract(json, '$.lastOthersActivityAt') FROM prs WHERE id = ?1), 0)
             ON CONFLICT(id) DO UPDATE SET seen_activity_at = excluded.seen_activity_at, forced_unread = 0"
        };
        self.conn().execute(sql, [id])?;
        Ok(())
    }
}

/// GitHub timestamps are all UTC `...Z`, so string order is time order.
fn is_unread(last_others: Option<&str>, seen: Option<&str>, forced: bool) -> bool {
    forced
        || match (last_others, seen) {
            (Some(last), Some(seen)) => last > seen,
            (Some(_), None) => true,
            (None, _) => false,
        }
}

fn passes(f: &Filters, pr: &PrSummary) -> bool {
    !(f.hide_drafts && pr.is_draft)
        && !f.exclude_authors.iter().any(|p| glob(p, &pr.author))
        && !f.exclude_repos.iter().any(|p| glob(p, &pr.repo))
}

/// Case-insensitive match where `*` stands for any run of characters.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.to_lowercase(), text.to_lowercase());
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == t;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !t.starts_with(first) || !t[first.len()..].ends_with(last) {
        return false;
    }
    let mut rest = &t[first.len()..t.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(id: &str, author: &str, repo: &str, draft: bool, last: Option<&str>, updated: &str) -> PrSummary {
        PrSummary {
            id: id.into(),
            number: 1,
            title: "t".into(),
            url: "u".into(),
            repo: repo.into(),
            author: author.into(),
            is_draft: draft,
            state: "OPEN".into(),
            created_at: updated.into(),
            updated_at: updated.into(),
            review_decision: None,
            checks: None,
            last_others_activity_at: last.map(Into::into),
            rules: vec!["r".into()],
            unread: false,
        }
    }

    #[test]
    fn glob_matches() {
        assert!(glob("*[bot]", "dependabot[bot]"));
        assert!(glob("dependabot[bot]", "Dependabot[bot]"));
        assert!(!glob("*[bot]", "bob"));
        assert!(glob("acme/*", "acme/api"));
        assert!(!glob("acme/*", "other/api"));
        assert!(glob("a*b*c", "axxbyyc"));
        assert!(!glob("a*b*c", "axxc"));
        assert!(!glob("ab*ba", "aba"));
    }

    #[test]
    fn unread_rules() {
        assert!(is_unread(Some("2026-01-02T00:00:00Z"), Some("2026-01-01T00:00:00Z"), false));
        assert!(!is_unread(Some("2026-01-01T00:00:00Z"), Some("2026-01-01T00:00:00Z"), false));
        assert!(is_unread(Some("2026-01-01T00:00:00Z"), None, false));
        assert!(!is_unread(None, None, false));
        assert!(is_unread(None, None, true));
    }

    #[test]
    fn inbox_filters_and_unread() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let t1 = "2026-01-01T00:00:00Z";
        let t2 = "2026-01-02T00:00:00Z";
        db.replace_prs(&[
            pr("a", "bob", "o/r", false, Some(t1), t1),
            pr("b", "dependabot[bot]", "o/r", false, Some(t1), t2),
            pr("c", "bob", "o/r", true, None, t2),
        ])
        .unwrap();
        let filters = Filters { exclude_authors: vec!["*[bot]".into()], exclude_repos: vec![], hide_drafts: false };
        let ids = |v: Vec<PrSummary>| v.into_iter().map(|p| (p.id, p.unread)).collect::<Vec<_>>();
        assert_eq!(ids(db.inbox(&filters).unwrap()), vec![("c".into(), false), ("a".into(), true)]);

        db.set_unread("a", false).unwrap();
        assert!(!db.inbox(&filters).unwrap().iter().any(|p| p.unread));
        db.set_unread("a", true).unwrap();
        assert!(db.inbox(&filters).unwrap().iter().find(|p| p.id == "a").unwrap().unread);

        // New activity after being read flips it back to unread.
        db.set_unread("a", false).unwrap();
        db.replace_prs(&[pr("a", "bob", "o/r", false, Some(t2), t2)]).unwrap();
        assert!(db.inbox(&filters).unwrap()[0].unread);

        let drafts = Filters { hide_drafts: true, exclude_repos: vec!["o/*".into()], ..filters };
        assert!(db.inbox(&drafts).unwrap().is_empty());
    }

    #[test]
    fn details_pruned_with_inbox() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        db.replace_prs(&[pr("a", "bob", "o/r", false, None, "x")]).unwrap();
        db.put_detail("a", &serde_json::json!({ "id": "a" })).unwrap();
        assert!(db.detail("a").unwrap().is_some());
        db.replace_prs(&[]).unwrap();
        assert!(db.detail("a").unwrap().is_none());
    }
}
