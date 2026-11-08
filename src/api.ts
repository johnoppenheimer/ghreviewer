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
};

export type Status = {
  viewer: string | null;
  syncing: boolean;
  lastSyncMs: number | null;
  rateRemaining: number | null;
  error: string | null;
  configError: string | null;
};

export type Rule = { name: string; query: string };
export type Filters = { exclude_authors: string[]; exclude_repos: string[]; hide_drafts: boolean };
export type Config = {
  poll_interval_secs: number;
  theme: string;
  clone_roots: string[];
  open_with: { name: string; command: string[] }[];
  prompts: Prompt[];
  rules: Rule[];
  filters: Filters;
  repos: { pinned: string[]; hidden: string[] };
  themes: Record<string, Record<string, string>>;
};
export type Prompt = {
  name: string;
  harness: "opencode" | "claude";
  prompt: string;
  args: string[];
};
/** Latest run of a prompt on a PR (src-tauri/src/agent.rs). */
export type Run = {
  prId: string;
  prompt: string;
  harness: Prompt["harness"];
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
};
/** Local clone + worktree on the PR branch; `wt` is whether worktrunk is installed. */
export type Worktree = { clone: string | null; path: string | null; wt: boolean };
export type Settings = Pick<Config, "poll_interval_secs" | "theme" | "rules" | "filters">;

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
  commits: Nodes<{
    commit: { statusCheckRollup: { state: string; contexts: Nodes<Check> } | null };
  }>;
  timelineItems: Nodes<TimelineItem>;
  reviewThreads: Nodes<Thread>;
};

export const list = <T>(n: Nodes<T> | undefined): T[] =>
  (n?.nodes ?? []).filter((x): x is T => x != null);

export const api = {
  status: () => invoke<Status>("get_status"),
  config: () => invoke<Config>("get_config"),
  repos: () => invoke<Repo[]>("list_repos"),
  inbox: () => invoke<PrSummary[]>("list_inbox"),
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
  runPrompt: (prompt: string, pr: PrDetail) =>
    invoke<void>("run_prompt", {
      prompt,
      pr: {
        id: pr.id,
        repo: pr.repository.nameWithOwner,
        number: pr.number,
        url: pr.url,
        branch: pr.headRefName,
        base: pr.baseRefName,
        headOid: pr.headRefOid,
      },
    }),
  cancelRun: (prId: string, prompt: string) => invoke<void>("cancel_run", { prId, prompt }),
  continueRun: (prId: string, prompt: string) => invoke<void>("continue_run", { prId, prompt }),
  openPath: (prId: string, path: string) => invoke<void>("open_path", { prId, path }),
};
