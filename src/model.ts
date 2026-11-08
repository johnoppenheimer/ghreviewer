import type { PrSummary, Run } from "./api";

export type SideItem =
  | { key: string; kind: "run"; run: Run }
  | { key: "all"; kind: "all"; total: number; unread: number }
  | { key: string; kind: "org"; org: string; total: number; unread: number; collapsed: boolean }
  | {
      key: string;
      kind: "repo";
      repo: string;
      name: string;
      total: number;
      unread: number;
      pinned: boolean;
      hidden: boolean;
    };

type Counts = { total: number; unread: number };

function count(map: Map<string, Counts>, key: string, pr: PrSummary) {
  const c = map.get(key) ?? { total: 0, unread: 0 };
  c.total++;
  if (pr.unread) c.unread++;
  map.set(key, c);
}

const orgOf = (repo: string) => repo.split("/")[0];

/** Flat sidebar: All, then orgs (with PRs first) and their repos (pinned, then with PRs, then the rest). */
export function sidebarItems(opts: {
  repos: string[];
  inbox: PrSummary[];
  pinned: string[];
  hidden: string[];
  collapsed: Set<string>;
  showHidden: boolean;
  /** Hide repos with no PRs in the inbox (pinned repos stay). */
  onlyWithPrs?: boolean;
  filter: string;
}): SideItem[] {
  const { inbox, collapsed, showHidden, onlyWithPrs } = opts;
  const filter = opts.filter.toLowerCase();
  const pinned = new Set(opts.pinned);
  const hidden = new Set(opts.hidden);
  const byRepo = new Map<string, Counts>();
  const byOrg = new Map<string, Counts>();
  for (const pr of inbox) {
    count(byRepo, pr.repo, pr);
    count(byOrg, orgOf(pr.repo), pr);
  }

  const orgs = new Map<string, string[]>();
  for (const repo of new Set([...opts.repos, ...byRepo.keys()])) {
    if (hidden.has(repo) && !showHidden) continue;
    if (onlyWithPrs && !byRepo.has(repo) && !pinned.has(repo)) continue;
    if (filter && !repo.toLowerCase().includes(filter)) continue;
    const org = orgOf(repo);
    orgs.set(org, [...(orgs.get(org) ?? []), repo]);
  }

  const rank = (has: boolean, a: string, hasB: boolean, b: string) =>
    Number(hasB) - Number(has) || a.localeCompare(b, undefined, { sensitivity: "base" });
  const orgNames = [...orgs.keys()].sort((a, b) => rank(byOrg.has(a), a, byOrg.has(b), b));

  const items: SideItem[] = [
    { key: "all", kind: "all", total: inbox.length, unread: inbox.filter((p) => p.unread).length },
  ];
  for (const org of orgNames) {
    const isCollapsed = collapsed.has(org) && !filter;
    items.push({
      key: `org:${org}`,
      kind: "org",
      org,
      collapsed: isCollapsed,
      ...(byOrg.get(org) ?? { total: 0, unread: 0 }),
    });
    if (isCollapsed) continue;
    const repos = orgs
      .get(org)!
      .sort(
        (a, b) =>
          Number(pinned.has(b)) - Number(pinned.has(a)) || rank(byRepo.has(a), a, byRepo.has(b), b),
      );
    for (const repo of repos) {
      items.push({
        key: `repo:${repo}`,
        kind: "repo",
        repo,
        name: repo.slice(org.length + 1),
        pinned: pinned.has(repo),
        hidden: hidden.has(repo),
        ...(byRepo.get(repo) ?? { total: 0, unread: 0 }),
      });
    }
  }
  return items;
}

/** Sidebar rows for agent runs that are going or not looked at yet; above All. */
export const runItems = (runs: Run[]): SideItem[] =>
  runs.map((run) => ({ key: `run:${run.prId}:${run.prompt}`, kind: "run", run }));

/** Whether a PR belongs to the sidebar selection (`all`, `org:<org>`, `repo:<owner/name>`). */
export function inScope(pr: PrSummary, key: string): boolean {
  if (key.startsWith("org:")) return orgOf(pr.repo) === key.slice(4);
  if (key.startsWith("repo:")) return pr.repo === key.slice(5);
  return true;
}

export function matchesFilter(pr: PrSummary, filter: string): boolean {
  const f = filter.toLowerCase();
  return !f || `${pr.repo}#${pr.number} ${pr.title} ${pr.author}`.toLowerCase().includes(f);
}

/** Index after moving `delta` from `current` (clamped), or jumping to an end with ±Infinity. */
export function step(len: number, current: number, delta: number): number {
  if (len === 0) return -1;
  return Math.max(0, Math.min(len - 1, current + delta));
}

export function relTime(iso: string | number, now = Date.now()): string {
  const s = Math.max(0, (now - new Date(iso).getTime()) / 1000);
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)}d`;
  if (s < 86400 * 365) return `${Math.floor(s / (86400 * 30))}mo`;
  return `${Math.floor(s / (86400 * 365))}y`;
}

/** Glyph + color class for a check rollup, CheckRun conclusion or status state. */
export function checkGlyph(state: string | null | undefined): [string, string] {
  switch (state) {
    case "SUCCESS":
      return ["✓", "ok"];
    case "FAILURE":
    case "ERROR":
    case "TIMED_OUT":
    case "CANCELLED":
    case "ACTION_REQUIRED":
    case "STARTUP_FAILURE":
      return ["✗", "fail"];
    case "NEUTRAL":
    case "SKIPPED":
    case "STALE":
      return ["○", "dim"];
    case null:
    case undefined:
      return ["", "dim"];
    default:
      return ["●", "warn"];
  }
}

export const REVIEW_LABEL: Record<string, [string, string]> = {
  APPROVED: ["approved", "ok"],
  CHANGES_REQUESTED: ["changes", "fail"],
  REVIEW_REQUIRED: ["review", "dim"],
  COMMENTED: ["commented", "dim"],
  DISMISSED: ["dismissed", "dim"],
  PENDING: ["pending", "dim"],
};
