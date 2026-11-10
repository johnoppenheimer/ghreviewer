use crate::agent::Review;
use crate::{
    config::Rule,
    github::{Github, Rejected},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    pub name_with_owner: String,
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
    /// Branch the PR merges into.
    #[serde(default)]
    pub base_ref: String,
    /// Head branch, `None` when it lives in a fork (so it can't be a stack parent).
    #[serde(default)]
    pub head_ref: Option<String>,
    /// Not in the inbox itself: a stack parent of an inbox PR, or dropped by the local filters.
    #[serde(default)]
    pub context: bool,
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
      nodes { nameWithOwner }
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

fn pr_fields() -> String {
    format!(
        r#"
fragment PrFields on PullRequest {{
  id number title url isDraft state createdAt updatedAt reviewDecision
  baseRefName headRefName isCrossRepository
  author {{ login }}
  repository {{ nameWithOwner defaultBranchRef {{ name }} }}
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
}}"#
    )
}

fn search_query() -> String {
    r#"
query($q: String!, $after: String) {
  rateLimit { remaining }
  search(type: ISSUE, query: $q, first: 50, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes { ...PrFields }
  }
}"#
    .to_string()
        + &pr_fields()
}

/// Open PRs whose head is each `(repo, branch)`, aliased `p0`, `p1`, ...
fn heads_query(n: usize) -> String {
    let vars: Vec<String> = (0..n)
        .map(|i| format!("$o{i}: String!, $n{i}: String!, $h{i}: String!"))
        .collect();
    let body: Vec<String> = (0..n)
        .map(|i| {
            format!(
                "p{i}: repository(owner: $o{i}, name: $n{i}) {{ \
                 pullRequests(headRefName: $h{i}, states: [OPEN], first: 5) {{ nodes {{ ...PrFields }} }} }}"
            )
        })
        .collect();
    format!("query({}) {{\n{}\n}}", vars.join(", "), body.join("\n")) + &pr_fields()
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
    base_ref_name: String,
    head_ref_name: String,
    is_cross_repository: bool,
    author: Option<Login>,
    repository: SearchRepo,
    commits: Nodes<CommitNode>,
    timeline_items: Nodes<Activity>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchRepo {
    name_with_owner: String,
    default_branch_ref: Option<Name>,
}

#[derive(Deserialize)]
struct Name {
    name: String,
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
            let who = c
                .author
                .as_ref()
                .and_then(|a| a.user.as_ref())
                .map(|u| u.login.as_str());
            return (who, Some(&c.committed_date));
        }
        let who = self
            .author
            .as_ref()
            .or(self.actor.as_ref())
            .map(|l| l.login.as_str());
        (who, self.created_at.as_deref())
    }
}

/// Latest activity not done by `viewer`. Unknown actors (deleted users, unlinked commits) count as others.
fn last_others_activity(pr: &SearchPr, viewer: &str) -> Option<String> {
    let created = (
        pr.author.as_ref().map(|a| a.login.as_str()),
        Some(pr.created_at.as_str()),
    );
    let items = pr
        .timeline_items
        .nodes
        .iter()
        .flatten()
        .map(Activity::who_when);
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
        base_ref: pr.base_ref_name,
        head_ref: (!pr.is_cross_repository).then_some(pr.head_ref_name),
        context: false,
    }
}

/// `(repo, base)` of PRs whose base is another branch than the default one and no known PR's head:
/// the stack parents still to look up.
fn missing_parents(
    prs: &HashMap<String, PrSummary>,
    defaults: &HashMap<String, String>,
    looked_up: &HashSet<(String, String)>,
) -> Vec<(String, String)> {
    let heads: HashSet<(&str, &str)> = prs
        .values()
        .filter_map(|p| Some((p.repo.as_str(), p.head_ref.as_deref()?)))
        .collect();
    let mut missing: Vec<(String, String)> = prs
        .values()
        .filter(|p| defaults.get(&p.repo) != Some(&p.base_ref))
        .filter(|p| !heads.contains(&(p.repo.as_str(), p.base_ref.as_str())))
        .map(|p| (p.repo.clone(), p.base_ref.clone()))
        .filter(|k| !looked_up.contains(k))
        .collect();
    missing.sort();
    missing.dedup();
    missing
}

const STACK_DEPTH: usize = 5;

/// Adds the open PRs that inbox PRs are stacked on, as `context`, up to `STACK_DEPTH` levels up.
// shortcut: one aliased query per level with every missing parent, chunk it if stacks get huge.
async fn fetch_parents(
    gh: &Github,
    viewer: &str,
    acc: &mut HashMap<String, PrSummary>,
    defaults: &mut HashMap<String, String>,
) -> Result<()> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RepoPrs {
        pull_requests: Nodes<SearchPr>,
    }
    let mut looked_up = HashSet::new();
    for _ in 0..STACK_DEPTH {
        let missing = missing_parents(acc, defaults, &looked_up);
        if missing.is_empty() {
            break;
        }
        let mut vars = serde_json::Map::new();
        for (i, (repo, base)) in missing.iter().enumerate() {
            let (owner, name) = repo.split_once('/').unwrap_or_default();
            vars.insert(format!("o{i}"), json!(owner));
            vars.insert(format!("n{i}"), json!(name));
            vars.insert(format!("h{i}"), json!(base));
        }
        let d: HashMap<String, Option<RepoPrs>> = gh
            .query(&heads_query(missing.len()), Value::Object(vars))
            .await?;
        for repo in d.into_values().flatten() {
            let parent = repo.pull_requests.nodes.into_iter().flatten();
            for pr in parent.filter(|p| !p.is_cross_repository).take(1) {
                if let Some(b) = &pr.repository.default_branch_ref {
                    defaults.insert(pr.repository.name_with_owner.clone(), b.name.clone());
                }
                let pr = PrSummary {
                    context: true,
                    ..summarize(pr, viewer)
                };
                acc.entry(pr.id.clone()).or_insert(pr);
            }
        }
        looked_up.extend(missing);
    }
    Ok(())
}

/// Adds `pr` to `acc`, deduping by id and recording every rule that matched it.
fn merge(acc: &mut HashMap<String, PrSummary>, pr: PrSummary, rule: &str) {
    let entry = acc.entry(pr.id.clone()).or_insert(pr);
    if !entry.rules.iter().any(|r| r == rule) {
        entry.rules.push(rule.to_string());
    }
}

/// Runs every rule's search and returns the deduped union, plus the remaining rate limit.
pub async fn fetch_inbox(
    gh: &Github,
    rules: &[Rule],
    viewer: &str,
) -> Result<(Vec<PrSummary>, Option<u64>)> {
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
    let mut defaults = HashMap::new();
    let mut remaining = None;
    for rule in rules.iter().filter(|r| !r.query.trim().is_empty()) {
        let q = format!("{} is:pr", rule.query);
        let mut after: Option<String> = None;
        loop {
            let d: Data = gh.query(&query, json!({ "q": q, "after": after })).await?;
            remaining = d.rate_limit.map(|r| r.remaining).or(remaining);
            for pr in d.search.nodes.into_iter().flatten() {
                if let Some(b) = &pr.repository.default_branch_ref {
                    defaults.insert(pr.repository.name_with_owner.clone(), b.name.clone());
                }
                merge(&mut acc, summarize(pr, viewer), &rule.name);
            }
            if !d.search.page_info.has_next_page {
                break;
            }
            after = d.search.page_info.end_cursor;
        }
    }
    // Best effort: a failed stack lookup must not cost the inbox its sync.
    if let Err(e) = fetch_parents(gh, viewer, &mut acc, &mut defaults).await {
        eprintln!("stack parents: {e:#}");
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
      baseRefName headRefName headRefOid additions deletions changedFiles
      mergeable mergeStateStatus reviewDecision
      author {{ login }}
      repository {{ nameWithOwner }}
      labels(first: 20) {{ nodes {{ name color }} }}
      assignees(first: 10) {{ nodes {{ login }} }}
      reviewRequests(first: 20) {{
        nodes {{ requestedReviewer {{ ... on User {{ login }} ... on Bot {{ login }} ... on Team {{ name }} }} }}
      }}
      latestReviews(first: 30) {{ nodes {{ author {{ login }} state submittedAt }} }}
      latestOpinionatedReviews(first: 30) {{ nodes {{ author {{ login }} state commit {{ oid }} }} }}
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
    if pr.is_null() {
        bail!("PR not found, or no access to it");
    }
    for _ in 0..10 {
        let info = &pr["timelineItems"]["pageInfo"];
        if info["hasNextPage"] != json!(true) {
            break;
        }
        let after = info["endCursor"].clone();
        let mut next: Value = gh
            .query(&query, json!({ "id": id, "after": after }))
            .await?;
        append_page(&mut pr, next["node"]["timelineItems"].take());
    }
    Ok(pr)
}

/// Adds a further timeline page to `pr` and takes over its cursor.
fn append_page(pr: &mut Value, mut page: Value) {
    let more = page["nodes"].take();
    if let (Some(nodes), Value::Array(more)) = (pr["timelineItems"]["nodes"].as_array_mut(), more) {
        nodes.extend(more);
    }
    pr["timelineItems"]["pageInfo"] = page["pageInfo"].take();
}

/// `AddPullRequestReviewInput` for the picked findings: inline comments where they have a
/// line (if `inline`), the rest listed in the body after the summary.
fn review_input(
    pr_id: &str,
    head_oid: Option<&str>,
    review: &Review,
    picked: &[usize],
    approve: bool,
    inline: bool,
) -> Value {
    let mut body: Vec<String> = Some(review.summary.trim().to_string())
        .filter(|s| !s.is_empty())
        .into_iter()
        .collect();
    let mut threads = Vec::new();
    for f in picked.iter().filter_map(|&i| review.findings.get(i)) {
        match (&f.path, f.line) {
            (Some(path), Some(line)) if inline => threads.push(json!({
                "path": path, "line": line, "side": "RIGHT",
                "body": format!("**{}**\n\n{}", f.title, f.body),
            })),
            (Some(path), line) => {
                let at = line.map(|l| format!(":{l}")).unwrap_or_default();
                body.push(format!("**{}** · `{path}{at}`\n\n{}", f.title, f.body));
            }
            _ => body.push(format!("**{}**\n\n{}", f.title, f.body)),
        }
    }
    json!({
        "pullRequestId": pr_id,
        // Inline lines and approvals go on the commit that was read. A plain comment doesn't
        // need it, and it may be gone after a force-push.
        "commitOID": if threads.is_empty() && !approve { None } else { head_oid },
        "event": if approve { "APPROVE" } else { "COMMENT" },
        "body": body.join("\n\n"),
        "threads": threads,
    })
}

const REVIEW_MUTATION: &str = r#"
mutation($input: AddPullRequestReviewInput!) {
  addPullRequestReview(input: $input) { pullRequestReview { url } }
}"#;

/// Posts the picked findings as one review on the commit the agent read. Returns its URL, and
/// whether GitHub rejected the inline comments so every finding went into the body instead.
pub async fn post_review(
    gh: &Github,
    pr_id: &str,
    head_oid: Option<&str>,
    review: &Review,
    picked: &[usize],
    approve: bool,
) -> Result<(String, bool)> {
    // A failed mutation comes back as a null field next to its errors, so read it untyped.
    let send = |input: Value| async move {
        let d: Value = gh
            .mutate(REVIEW_MUTATION, json!({ "input": input }))
            .await?;
        let url = d.pointer("/addPullRequestReview/pullRequestReview/url");
        url.and_then(Value::as_str)
            .map(Into::into)
            .context("GitHub returned no review URL")
    };
    let input = review_input(pr_id, head_oid, review, picked, approve, true);
    let has_inline = input["threads"].as_array().is_some_and(|t| !t.is_empty());
    match send(input).await {
        Ok(url) => Ok((url, false)),
        // GitHub rejected it, so no review was made: usually a line outside the diff, which fails
        // the whole review. Not on timeouts or 5xx: the review may exist despite them.
        Err(e) if has_inline && e.is::<Rejected>() => {
            let input = review_input(pr_id, head_oid, review, picked, approve, false);
            Ok((send(input).await?, true))
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Finding;

    #[test]
    fn review_input_splits_inline_and_body() {
        let finding = |title: &str, path: Option<&str>, line| Finding {
            title: title.into(),
            body: "b".into(),
            path: path.map(Into::into),
            line,
            ..Default::default()
        };
        let review = Review {
            summary: " Thanks! ".into(),
            findings: vec![
                finding("inline", Some("a.rs"), Some(3)),
                finding("file", Some("b.rs"), None),
                finding("general", None, None),
                finding("unpicked", None, None),
            ],
            ..Default::default()
        };
        let input = review_input("PR_1", Some("abc"), &review, &[0, 1, 2, 9], false, true);
        assert_eq!(input["event"], "COMMENT");
        assert_eq!(input["commitOID"], "abc");
        assert_eq!(
            input["threads"],
            json!([{ "path": "a.rs", "line": 3, "side": "RIGHT", "body": "**inline**\n\nb" }])
        );
        assert_eq!(
            input["body"],
            "Thanks!\n\n**file** · `b.rs`\n\nb\n\n**general**\n\nb"
        );
        let all = review_input("PR_1", Some("abc"), &review, &[0], true, false);
        assert_eq!(all["event"], "APPROVE");
        assert_eq!(all["threads"], json!([]));
        // An approval stays on the commit that was read, even with nothing inline.
        assert_eq!(all["commitOID"], "abc");
        let body_only = review_input("PR_1", Some("abc"), &review, &[1], false, true);
        assert_eq!(body_only["commitOID"], Value::Null);
        assert_eq!(all["body"], "Thanks!\n\n**inline** · `a.rs:3`\n\nb");
        // `approve_pr` with no message: a plain approval of the current head.
        assert_eq!(
            review_input("PR_1", None, &Review::default(), &[], true, true),
            json!({ "pullRequestId": "PR_1", "commitOID": null, "event": "APPROVE", "body": "", "threads": [] })
        );
    }

    fn pr(author: &str, created: &str, timeline: Value) -> SearchPr {
        serde_json::from_value(json!({
            "id": "PR_1", "number": 1, "title": "t", "url": "u", "isDraft": false, "state": "OPEN",
            "createdAt": created, "updatedAt": created, "reviewDecision": null,
            "baseRefName": "main", "headRefName": "feat", "isCrossRepository": false,
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
        assert_eq!(
            last_others_activity(&p, "me").as_deref(),
            Some("2026-01-03T00:00:00Z")
        );
    }

    #[test]
    fn creation_by_others_counts() {
        let p = pr("bob", "2026-01-01T00:00:00Z", json!([]));
        assert_eq!(
            last_others_activity(&p, "me").as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
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
        let rules = [Rule {
            name: "any".into(),
            query: format!("involves:{me}"),
        }];
        let (prs, rate) = fetch_inbox(&gh, &rules, &me).await.unwrap();
        println!(
            "viewer={me} repos={} prs={} parents={} rate={rate:?}",
            repos.len(),
            prs.len(),
            prs.iter().filter(|p| p.context).count()
        );
        if let Some(pr) = prs.first() {
            let d = fetch_detail(&gh, &pr.id).await.unwrap();
            assert_eq!(d["id"], json!(pr.id));
            println!(
                "{} timeline={}",
                pr.url,
                d["timelineItems"]["nodes"].as_array().unwrap().len()
            );
        }
    }

    #[test]
    fn timeline_pages_append() {
        let mut pr = json!({ "id": "PR_1", "timelineItems": {
            "pageInfo": { "hasNextPage": true, "endCursor": "c1" },
            "nodes": [{ "id": "a" }],
        }});
        append_page(
            &mut pr,
            json!({
                "pageInfo": { "hasNextPage": false, "endCursor": "c2" },
                "nodes": [{ "id": "b" }, { "id": "c" }],
            }),
        );
        let ids: Vec<_> = pr["timelineItems"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(pr["timelineItems"]["pageInfo"]["hasNextPage"], json!(false));
        assert_eq!(pr["id"], json!("PR_1"));
    }

    #[test]
    fn missing_parents_skips_default_known_and_looked_up() {
        let mk = |id: &str, repo: &str, base: &str, head: Option<&str>| {
            let mut p = summarize(pr("bob", "2026-01-01T00:00:00Z", json!([])), "me");
            (p.id, p.repo, p.base_ref, p.head_ref) =
                (id.into(), repo.into(), base.into(), head.map(Into::into));
            (id.to_string(), p)
        };
        let prs = HashMap::from([
            mk("1", "o/r", "main", Some("a")),
            mk("2", "o/r", "a", Some("b")),
            mk("3", "o/r", "x", Some("c")),
            mk("4", "o/r", "y", Some("d")),
            mk("5", "o/q", "x", None),
        ]);
        let defaults = HashMap::from([("o/r".to_string(), "main".to_string())]);
        let looked_up = HashSet::from([("o/r".to_string(), "y".to_string())]);
        assert_eq!(
            missing_parents(&prs, &defaults, &looked_up),
            vec![("o/q".into(), "x".into()), ("o/r".into(), "x".into())]
        );
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
