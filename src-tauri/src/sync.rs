use crate::{config::Rule, github::Github};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    pub name_with_owner: String,
    pub is_private: bool,
    pub pushed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrSummary {
    pub id: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub repo: String,
    pub author: String,
    pub is_draft: bool,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub review_decision: Option<String>,
    pub checks: Option<String>,
    pub last_others_activity_at: Option<String>,
    pub rules: Vec<String>,
    #[serde(default)]
    pub unread: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    page_info: PageInfo,
    nodes: Vec<Option<T>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<Option<T>>,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

pub async fn viewer(gh: &Github) -> Result<String> {
    #[derive(Deserialize)]
    struct Data {
        viewer: Login,
    }
    let d: Data = gh.query("query { viewer { login } }", json!({})).await?;
    Ok(d.viewer.login)
}

const REPOS_QUERY: &str = r#"
query($after: String) {
  viewer {
    repositories(first: 100, after: $after, isArchived: false,
      affiliations: [OWNER, COLLABORATOR, ORGANIZATION_MEMBER],
      ownerAffiliations: [OWNER, COLLABORATOR, ORGANIZATION_MEMBER]) {
      pageInfo { hasNextPage endCursor }
      nodes { nameWithOwner isPrivate pushedAt }
    }
  }
}"#;

pub async fn fetch_repos(gh: &Github) -> Result<Vec<Repo>> {
    #[derive(Deserialize)]
    struct Data {
        viewer: Viewer,
    }
    #[derive(Deserialize)]
    struct Viewer {
        repositories: Page<Repo>,
    }
    let mut repos = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let d: Data = gh.query(REPOS_QUERY, json!({ "after": after })).await?;
        let page = d.viewer.repositories;
        repos.extend(page.nodes.into_iter().flatten());
        if !page.page_info.has_next_page {
            return Ok(repos);
        }
        after = page.page_info.end_cursor;
    }
}

const ACTIVITY_TYPES: &str = "ISSUE_COMMENT, PULL_REQUEST_REVIEW, PULL_REQUEST_COMMIT, \
    REVIEW_REQUESTED_EVENT, READY_FOR_REVIEW_EVENT, HEAD_REF_FORCE_PUSHED_EVENT, \
    MERGED_EVENT, CLOSED_EVENT, REOPENED_EVENT";

fn search_query() -> String {
    format!(
        r#"
query($q: String!, $after: String) {{
  rateLimit {{ remaining }}
  search(type: ISSUE, query: $q, first: 50, after: $after) {{
    pageInfo {{ hasNextPage endCursor }}
    nodes {{
      ... on PullRequest {{
        id number title url isDraft state createdAt updatedAt reviewDecision
        author {{ login }}
        repository {{ nameWithOwner }}
        commits(last: 1) {{ nodes {{ commit {{ statusCheckRollup {{ state }} }} }} }}
        timelineItems(last: 10, itemTypes: [{ACTIVITY_TYPES}]) {{
          nodes {{
            ... on IssueComment {{ createdAt author {{ login }} }}
            ... on PullRequestReview {{ createdAt author {{ login }} }}
            ... on PullRequestCommit {{ commit {{ committedDate author {{ user {{ login }} }} }} }}
            ... on ReviewRequestedEvent {{ createdAt actor {{ login }} }}
            ... on ReadyForReviewEvent {{ createdAt actor {{ login }} }}
            ... on HeadRefForcePushedEvent {{ createdAt actor {{ login }} }}
            ... on MergedEvent {{ createdAt actor {{ login }} }}
            ... on ClosedEvent {{ createdAt actor {{ login }} }}
            ... on ReopenedEvent {{ createdAt actor {{ login }} }}
          }}
        }}
      }}
    }}
  }}
}}"#
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchPr {
    id: String,
    number: u64,
    title: String,
    url: String,
    is_draft: bool,
    state: String,
    created_at: String,
    updated_at: String,
    review_decision: Option<String>,
    author: Option<Login>,
    repository: SearchRepo,
    commits: Nodes<CommitNode>,
    timeline_items: Nodes<Activity>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchRepo {
    name_with_owner: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitNode {
    commit: CommitRollup,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitRollup {
    status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
struct Rollup {
    state: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Activity {
    created_at: Option<String>,
    author: Option<Login>,
    actor: Option<Login>,
    commit: Option<ActivityCommit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivityCommit {
    committed_date: String,
    author: Option<GitActor>,
}

#[derive(Deserialize)]
struct GitActor {
    user: Option<Login>,
}

impl Activity {
    fn who_when(&self) -> (Option<&str>, Option<&str>) {
        if let Some(c) = &self.commit {
            let who = c.author.as_ref().and_then(|a| a.user.as_ref()).map(|u| u.login.as_str());
            return (who, Some(&c.committed_date));
        }
        let who = self.author.as_ref().or(self.actor.as_ref()).map(|l| l.login.as_str());
        (who, self.created_at.as_deref())
    }
}

/// Latest activity not done by `viewer`. Unknown actors (deleted users, unlinked commits) count as others.
fn last_others_activity(pr: &SearchPr, viewer: &str) -> Option<String> {
    let created = (pr.author.as_ref().map(|a| a.login.as_str()), Some(pr.created_at.as_str()));
    let items = pr.timeline_items.nodes.iter().flatten().map(Activity::who_when);
    std::iter::once(created)
        .chain(items)
        .filter(|(who, _)| *who != Some(viewer))
        .filter_map(|(_, when)| when)
        .max()
        .map(str::to_string)
}

fn summarize(pr: SearchPr, viewer: &str) -> PrSummary {
    let last_others_activity_at = last_others_activity(&pr, viewer);
    let checks = pr
        .commits
        .nodes
        .into_iter()
        .flatten()
        .next()
        .and_then(|c| c.commit.status_check_rollup)
        .map(|r| r.state);
    PrSummary {
        id: pr.id,
        number: pr.number,
        title: pr.title,
        url: pr.url,
        repo: pr.repository.name_with_owner,
        author: pr.author.map_or_else(|| "ghost".into(), |a| a.login),
        is_draft: pr.is_draft,
        state: pr.state,
        created_at: pr.created_at,
        updated_at: pr.updated_at,
        review_decision: pr.review_decision,
        checks,
        last_others_activity_at,
        rules: vec![],
        unread: false,
    }
}

/// Adds `pr` to `acc`, deduping by id and recording every rule that matched it.
fn merge(acc: &mut HashMap<String, PrSummary>, pr: PrSummary, rule: &str) {
    let entry = acc.entry(pr.id.clone()).or_insert(pr);
    if !entry.rules.iter().any(|r| r == rule) {
        entry.rules.push(rule.to_string());
    }
}

/// Runs every rule's search and returns the deduped union, plus the remaining rate limit.
pub async fn fetch_inbox(gh: &Github, rules: &[Rule], viewer: &str) -> Result<(Vec<PrSummary>, Option<u64>)> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Data {
        rate_limit: Option<RateLimit>,
        search: Page<SearchPr>,
    }
    #[derive(Deserialize)]
    struct RateLimit {
        remaining: u64,
    }

    let query = search_query();
    let mut acc = HashMap::new();
    let mut remaining = None;
    for rule in rules.iter().filter(|r| !r.query.trim().is_empty()) {
        let q = format!("{} is:pr", rule.query);
        let mut after: Option<String> = None;
        loop {
            let d: Data = gh.query(&query, json!({ "q": q, "after": after })).await?;
            remaining = d.rate_limit.map(|r| r.remaining).or(remaining);
            for pr in d.search.nodes.into_iter().flatten() {
                merge(&mut acc, summarize(pr, viewer), &rule.name);
            }
            if !d.search.page_info.has_next_page {
                break;
            }
            after = d.search.page_info.end_cursor;
        }
    }
    Ok((acc.into_values().collect(), remaining))
}

const DETAIL_TYPES: &str = "ISSUE_COMMENT, PULL_REQUEST_REVIEW, PULL_REQUEST_COMMIT, \
    REVIEW_REQUESTED_EVENT, READY_FOR_REVIEW_EVENT, CONVERT_TO_DRAFT_EVENT, \
    HEAD_REF_FORCE_PUSHED_EVENT, MERGED_EVENT, CLOSED_EVENT, REOPENED_EVENT, LABELED_EVENT";

fn detail_query() -> String {
    format!(
        r#"
query($id: ID!, $after: String) {{
  node(id: $id) {{
    ... on PullRequest {{
      id number title url state isDraft createdAt updatedAt bodyHTML
      baseRefName headRefName additions deletions changedFiles
      mergeable mergeStateStatus reviewDecision
      author {{ login }}
      repository {{ nameWithOwner }}
      labels(first: 20) {{ nodes {{ name color }} }}
      assignees(first: 10) {{ nodes {{ login }} }}
      reviewRequests(first: 20) {{
        nodes {{ requestedReviewer {{ ... on User {{ login }} ... on Bot {{ login }} ... on Team {{ name }} }} }}
      }}
      latestReviews(first: 30) {{ nodes {{ author {{ login }} state submittedAt }} }}
      commits(last: 1) {{
        nodes {{ commit {{ statusCheckRollup {{ state
          contexts(first: 100) {{ nodes {{
            __typename
            ... on CheckRun {{ name status conclusion detailsUrl }}
            ... on StatusContext {{ context state targetUrl }}
          }} }}
        }} }} }}
      }}
      timelineItems(first: 100, after: $after, itemTypes: [{DETAIL_TYPES}]) {{
        pageInfo {{ hasNextPage endCursor }}
        nodes {{
          __typename
          ... on IssueComment {{ id createdAt url bodyHTML author {{ login }} }}
          ... on PullRequestReview {{ id createdAt url bodyHTML state author {{ login }} comments {{ totalCount }} }}
          ... on PullRequestCommit {{ id commit {{ abbreviatedOid messageHeadline committedDate url author {{ name user {{ login }} }} }} }}
          ... on ReviewRequestedEvent {{ id createdAt actor {{ login }}
            requestedReviewer {{ ... on User {{ login }} ... on Bot {{ login }} ... on Team {{ name }} }} }}
          ... on ReadyForReviewEvent {{ id createdAt actor {{ login }} }}
          ... on ConvertToDraftEvent {{ id createdAt actor {{ login }} }}
          ... on HeadRefForcePushedEvent {{ id createdAt actor {{ login }} }}
          ... on MergedEvent {{ id createdAt actor {{ login }} }}
          ... on ClosedEvent {{ id createdAt actor {{ login }} }}
          ... on ReopenedEvent {{ id createdAt actor {{ login }} }}
          ... on LabeledEvent {{ id createdAt actor {{ login }} label {{ name color }} }}
        }}
      }}
      reviewThreads(first: 100) {{
        nodes {{
          id path line originalLine isResolved isOutdated
          comments(first: 50) {{ nodes {{ id createdAt url bodyHTML diffHunk author {{ login }} }} }}
        }}
      }}
    }}
  }}
}}"#
    )
}

/// Full PR as GitHub returns it; timeline pages are concatenated.
// shortcut: review threads capped at 100 (50 comments each), paginate if real PRs exceed it.
pub async fn fetch_detail(gh: &Github, id: &str) -> Result<Value> {
    let query = detail_query();
    let mut pr: Value = gh.query::<Value>(&query, json!({ "id": id })).await?["node"].take();
    for _ in 0..10 {
        let info = &pr["timelineItems"]["pageInfo"];
        if info["hasNextPage"] != json!(true) {
            break;
        }
        let after = info["endCursor"].clone();
        let mut next: Value = gh.query(&query, json!({ "id": id, "after": after })).await?;
        let page = next["node"]["timelineItems"].take();
        if let (Some(nodes), Some(more)) = (
            pr["timelineItems"]["nodes"].as_array_mut(),
            page["nodes"].as_array(),
        ) {
            nodes.extend(more.iter().cloned());
        }
        pr["timelineItems"]["pageInfo"] = page["pageInfo"].clone();
    }
    Ok(pr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(author: &str, created: &str, timeline: Value) -> SearchPr {
        serde_json::from_value(json!({
            "id": "PR_1", "number": 1, "title": "t", "url": "u", "isDraft": false, "state": "OPEN",
            "createdAt": created, "updatedAt": created, "reviewDecision": null,
            "author": { "login": author }, "repository": { "nameWithOwner": "o/r" },
            "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "SUCCESS" } } }] },
            "timelineItems": { "nodes": timeline },
        }))
        .unwrap()
    }

    #[test]
    fn last_activity_ignores_viewer() {
        let p = pr(
            "me",
            "2026-01-01T00:00:00Z",
            json!([
                { "createdAt": "2026-01-02T00:00:00Z", "author": { "login": "bob" } },
                { "commit": { "committedDate": "2026-01-03T00:00:00Z", "author": { "user": { "login": "alice" } } } },
                { "createdAt": "2026-01-04T00:00:00Z", "author": { "login": "me" } },
                {},
            ]),
        );
        assert_eq!(last_others_activity(&p, "me").as_deref(), Some("2026-01-03T00:00:00Z"));
    }

    #[test]
    fn creation_by_others_counts() {
        let p = pr("bob", "2026-01-01T00:00:00Z", json!([]));
        assert_eq!(last_others_activity(&p, "me").as_deref(), Some("2026-01-01T00:00:00Z"));
        let mine = pr("me", "2026-01-01T00:00:00Z", json!([]));
        assert_eq!(last_others_activity(&mine, "me"), None);
    }

    /// Hits GitHub with the local `gh` login: `cargo test -- --ignored live --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live() {
        let gh = Github::new();
        let me = viewer(&gh).await.unwrap();
        let repos = fetch_repos(&gh).await.unwrap();
        let rules = [Rule { name: "any".into(), query: format!("involves:{me}") }];
        let (prs, rate) = fetch_inbox(&gh, &rules, &me).await.unwrap();
        println!("viewer={me} repos={} prs={} rate={rate:?}", repos.len(), prs.len());
        if let Some(pr) = prs.first() {
            let d = fetch_detail(&gh, &pr.id).await.unwrap();
            assert_eq!(d["id"], json!(pr.id));
            println!("{} timeline={}", pr.url, d["timelineItems"]["nodes"].as_array().unwrap().len());
        }
    }

    #[test]
    fn merge_dedupes_and_tags() {
        let mut acc = HashMap::new();
        let s = summarize(pr("bob", "2026-01-01T00:00:00Z", json!([])), "me");
        assert_eq!(s.checks.as_deref(), Some("SUCCESS"));
        merge(&mut acc, s.clone(), "review");
        merge(&mut acc, s.clone(), "mine");
        merge(&mut acc, s, "review");
        assert_eq!(acc.len(), 1);
        assert_eq!(acc["PR_1"].rules, vec!["review", "mine"]);
    }
}
