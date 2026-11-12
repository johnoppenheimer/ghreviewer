import { invoke } from "@tauri-apps/api/core";

export type Repo = { nameWithOwner: string };

export type PrSummary = {
  id: string;
  number: number;
  title: string;
  url: string;
  repo: string;
  author: string;
  isDraft: boolean;
  state: string;
  createdAt: string;
  updatedAt: string;
  reviewDecision: string | null;
  checks: string | null;
  lastOthersActivityAt: string | null;
  rules: string[];
  unread: boolean;
  baseRef: string;
  /** Null when the head branch is in a fork. */
  headRef: string | null;
  /** Not in the inbox itself: a stack parent of an inbox PR, or dropped by the local filters. */
  context: boolean;
  /** Tickets named in the title and found in Jira. */
  jira: TicketRef[];
  /** Who the PR waits on, for the smart inbox (`Next` in src-tauri/src/sync.rs). */
  next: Next | null;
};

export type Next =
  | "back"
  | "feedback"
  | "updated"
  | "failing"
  | "new"
  | "ready"
  | "waiting"
  | "done";

/** `category` is Jira's status category: `new`, `indeterminate` (in progress) or `done`. */
export type TicketRef = { key: string; category: string };
/** A Jira issue (src-tauri/src/jira.rs). The HTML is Jira's own rendering: sanitize it. */
export type Ticket = TicketRef & {
  url: string;
  summary: string;
  kind: string;
  status: string;
  assignee: string | null;
  reporter: string | null;
  priority: string | null;
  labels: string[];
  parent: { key: string; url: string; summary: string; kind: string } | null;
  descriptionHtml: string;
  comments: { author: string; created: string; bodyHtml: string }[];
};

export type Status = {
  viewer: string | null;
  syncing: boolean;
  syncingJira: boolean;
  lastSyncMs: number | null;
  rateRemaining: number | null;
  error: string | null;
  jiraError: string | null;
  configError: string | null;
};

export type Rule = { name: string; query: string };
export type Filters = {
  exclude_authors: string[];
  include_repos: string[];
  exclude_repos: string[];
  hide_drafts: boolean;
};
export type Config = {
  poll_interval_secs: number;
  theme: string;
  /** Adds the "Next" view above All. */
  smart_inbox: boolean;
  clone_roots: string[];
  open_with: { name: string; command: string[] }[];
  prompts: Prompt[];
  /** The agent picked findings go to; its runs are those of a prompt named "fix". */
  fix: Omit<Prompt, "name"> | null;
  /** `token` is never read back: set it to replace the one in the file. */
  jira: { url: string; email: string; prefixes: string[]; token_env: string; token?: string };
  rules: Rule[];
  filters: Filters;
  repos: { pinned: string[]; hidden: string[] };
  themes: Record<string, Record<string, string>>;
};
export type Prompt = {
  name: string;
  harness: "opencode" | "claude" | "greptile";
  prompt: string;
  args: string[];
};
/** Latest run of a prompt on a PR (src-tauri/src/agent.rs). */
export type Run = {
  prId: string;
  repo: string;
  number: number;
  prompt: string;
  harness: Prompt["harness"];
  /** Finished result looked at; until then the sidebar lists the run. */
  seen: boolean;
  status: "running" | "done" | "error" | "cancelled" | "interrupted";
  startedAt: number;
  finishedAt: number | null;
  headOid: string | null;
  path: string | null;
  sessionId: string | null;
  activity: string[];
  output: string;
  error: string | null;
  files: string[];
  review: Review | null;
};
/** Findings an agent returned in a ```ghreviewer block, to pick and post as a GitHub review. */
export type Review = {
  summary: string;
  findings: Finding[];
  /** Review URL once posted. */
  posted: string | null;
  /** GitHub rejected the inline comments, so every finding went into the review body. */
  allInBody: boolean;
};
export type Finding = {
  severity: string;
  title: string;
  body: string;
  path: string | null;
  line: number | null;
};
/** Local clone + worktree on the PR branch; `wt` is whether worktrunk is installed. */
export type Worktree = { clone: string | null; path: string | null; wt: boolean };
export type Settings = Pick<
  Config,
  "poll_interval_secs" | "theme" | "smart_inbox" | "rules" | "filters" | "jira"
>;

// PR detail, in the shape of the GraphQL query in src-tauri/src/sync.rs.
type Login = { login: string } | null;
export type Nodes<T> = { nodes: (T | null)[] } | null;
export type Reviewer = { login?: string; name?: string } | null;
export type Check = {
  __typename: "CheckRun" | "StatusContext";
  name?: string;
  status?: string;
  conclusion?: string | null;
  detailsUrl?: string | null;
  context?: string;
  state?: string;
  targetUrl?: string | null;
};
export type TimelineItem = {
  __typename: string;
  id: string;
  createdAt?: string;
  url?: string;
  bodyHTML?: string;
  state?: string;
  author?: Login;
  actor?: Login;
  comments?: { totalCount: number };
  commit?: {
    abbreviatedOid: string;
    messageHeadline: string;
    committedDate: string;
    url: string;
    author: { name: string | null; user: Login } | null;
  };
  label?: { name: string; color: string };
  requestedReviewer?: Reviewer;
};
export type ThreadComment = {
  id: string;
  createdAt: string;
  url: string;
  /** Markdown; missing from details cached before it was fetched. */
  body?: string;
  bodyHTML: string;
  diffHunk: string;
  author: Login;
};
export type Thread = {
  id: string;
  path: string;
  line: number | null;
  originalLine: number | null;
  isResolved: boolean;
  isOutdated: boolean;
  comments: Nodes<ThreadComment>;
};
export type PrDetail = {
  id: string;
  number: number;
  title: string;
  url: string;
  state: string;
  isDraft: boolean;
  createdAt: string;
  updatedAt: string;
  bodyHTML: string;
  baseRefName: string;
  headRefName: string;
  headRefOid: string;
  additions: number;
  deletions: number;
  changedFiles: number;
  mergeable: string;
  mergeStateStatus: string;
  reviewDecision: string | null;
  author: Login;
  repository: { nameWithOwner: string };
  labels: Nodes<{ name: string; color: string }>;
  assignees: Nodes<{ login: string }>;
  reviewRequests: Nodes<{ requestedReviewer: Reviewer }>;
  latestReviews: Nodes<{ author: Login; state: string; submittedAt: string | null }>;
  /** Each reviewer's latest approval or change request. Missing from details cached before it. */
  latestOpinionatedReviews?: Nodes<{
    author: Login;
    state: string;
    commit: { oid: string } | null;
  }>;
  commits: Nodes<{
    commit: { statusCheckRollup: { state: string; contexts: Nodes<Check> } | null };
  }>;
  timelineItems: Nodes<TimelineItem>;
  reviewThreads: Nodes<Thread>;
};

export const list = <T>(n: Nodes<T> | undefined): T[] =>
  (n?.nodes ?? []).filter((x): x is T => x != null);

/** The PR a run works on (`Target` in src-tauri/src/lib.rs). */
const target = (pr: PrDetail) => ({
  id: pr.id,
  repo: pr.repository.nameWithOwner,
  number: pr.number,
  url: pr.url,
  branch: pr.headRefName,
  base: pr.baseRefName,
  headOid: pr.headRefOid,
});

export const api = {
  status: () => invoke<Status>("get_status"),
  config: () => invoke<Config>("get_config"),
  repos: () => invoke<Repo[]>("list_repos"),
  inbox: () => invoke<PrSummary[]>("list_inbox"),
  tickets: (keys: string[]) => invoke<Ticket[]>("get_tickets", { keys }),
  pr: (id: string) => invoke<PrDetail>("get_pr", { id }),
  setUnread: (id: string, unread: boolean) => invoke<void>("set_unread", { id, unread }),
  refresh: () => invoke<void>("refresh"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  setRepoFlag: (repo: string, flag: "pinned" | "hidden", on: boolean) =>
    invoke<void>("set_repo_flag", { repo, flag, on }),
  openConfig: () => invoke<void>("open_config"),
  worktree: (repo: string, branch: string) => invoke<Worktree>("get_worktree", { repo, branch }),
  createWorktree: (repo: string, number: number) =>
    invoke<void>("create_worktree", { repo, number }),
  /** Runs the `opener`-th `open_with` command from the config on a worktree. */
  openWorktree: (opener: number, path: string, pr: PrSummary, branch: string) =>
    invoke<void>("open_worktree", { opener, path, repo: pr.repo, branch, number: pr.number }),
  runs: (prId: string) => invoke<Run[]>("get_runs", { prId }),
  activeRuns: () => invoke<Run[]>("list_active_runs"),
  markRunSeen: (prId: string, prompt: string) => invoke<void>("mark_run_seen", { prId, prompt }),
  runPrompt: (prompt: string, pr: PrDetail) =>
    invoke<void>("run_prompt", { prompt, pr: target(pr) }),
  fix: (pr: PrDetail, findings: Finding[]) =>
    invoke<void>("fix_findings", { pr: target(pr), findings }),
  cancelRun: (prId: string, prompt: string) => invoke<void>("cancel_run", { prId, prompt }),
  continueRun: (prId: string, prompt: string) => invoke<void>("continue_run", { prId, prompt }),
  postReview: (prId: string, prompt: string, picked: number[], approve: boolean) =>
    invoke<void>("post_review", { prId, prompt, picked, approve }),
  /** Approves the commit that was on screen, not whatever the head is by now. */
  approve: (pr: PrDetail, body: string) =>
    invoke<void>("approve_pr", { id: pr.id, headOid: pr.headRefOid, body }),
  openPath: (prId: string, path: string) => invoke<void>("open_path", { prId, path }),
};
