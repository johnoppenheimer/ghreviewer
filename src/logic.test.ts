import { expect, test } from "bun:test";
import type { PrDetail, PrSummary } from "./api";
import { createKeymap } from "./keys";
import {
  canApprove,
  ghImageSrc,
  inScope,
  jiraClass,
  relTime,
  shouldMarkRead,
  sidebarItems,
  stackRows,
  step,
  titleParts,
} from "./model";

const key = (
  k: string,
  mods: Partial<{ ctrlKey: boolean; shiftKey: boolean; metaKey: boolean }> = {},
) => ({
  key: k,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  ...mods,
});

test("keymap decodes vim keys and gg", () => {
  const km = createKeymap();
  expect(km(key("j"))).toBe("down");
  expect(km(key("g"))).toBeNull();
  expect(km(key("g"))).toBe("top");
  expect(km(key("g"))).toBeNull();
  expect(km(key("j"))).toBe("down"); // pending g dropped
  expect(km(key("g"))).toBeNull();
  expect(km(key("G"))).toBe("bottom");
  expect(km(key("d", { ctrlKey: true }))).toBe("pageDown");
  expect(km(key("Tab", { shiftKey: true }))).toBe("left");
  expect(km(key("r", { metaKey: true }))).toBe("refresh");
  expect(km(key("r"))).toBe("runPrompt");
  expect(km(key("}"))).toBe("nextFile");
  expect(km(key("a"))).toBe("approve");
  expect(km(key("c", { metaKey: true }))).toBeNull();
});

test("titleParts cuts a title around its Jira keys", () => {
  const ti = { key: "TI-1", category: "done" };
  expect(titleParts("no ticket", [])).toEqual([{ text: "no ticket" }]);
  expect(titleParts("[ti-1][TI-12] fix", [ti])).toEqual([
    { text: "[", ticket: undefined },
    { text: "ti-1", ticket: ti },
    { text: "][TI-12] fix", ticket: undefined },
  ]);
  expect([jiraClass("new"), jiraClass("indeterminate"), jiraClass("done")]).toEqual([
    "dim",
    "accent",
    "ok",
  ]);
});

const pr = (repo: string, unread = false): PrSummary => ({
  id: `${repo}-${Math.random()}`,
  number: 1,
  title: "t",
  url: "",
  repo,
  author: "bob",
  isDraft: false,
  state: "OPEN",
  createdAt: "",
  updatedAt: "",
  reviewDecision: null,
  checks: null,
  lastOthersActivityAt: null,
  rules: [],
  unread,
  baseRef: "main",
  headRef: null,
  context: false,
  jira: [],
});

test("sidebar orders orgs and repos, counts, hides, filters", () => {
  const base = {
    repos: ["zeta/a", "acme/web", "acme/api", "acme/old", "acme/tools"],
    inbox: [pr("acme/web", true), pr("acme/web"), pr("ext/lib")],
    pinned: ["acme/tools"],
    hidden: ["acme/old"],
    collapsed: new Set<string>(),
    showHidden: false,
    onlyWithPrs: false,
    filter: "",
  };
  const keys = (opts = base) => sidebarItems(opts).map((i) => i.key);
  expect(keys()).toEqual([
    "all",
    "org:acme",
    "repo:acme/tools",
    "repo:acme/web",
    "repo:acme/api",
    "org:ext",
    "repo:ext/lib",
    "org:zeta",
    "repo:zeta/a",
  ]);
  const items = sidebarItems(base);
  expect(items[0]).toMatchObject({ total: 3, unread: 1 });
  expect(items[1]).toMatchObject({ total: 2, unread: 1 });
  expect(keys({ ...base, showHidden: true })).toContain("repo:acme/old");
  expect(keys({ ...base, onlyWithPrs: true })).toEqual([
    "all",
    "org:acme",
    "repo:acme/tools",
    "repo:acme/web",
    "org:ext",
    "repo:ext/lib",
  ]);
  expect(keys({ ...base, collapsed: new Set(["acme"]) }).slice(0, 3)).toEqual([
    "all",
    "org:acme",
    "org:ext",
  ]);
  expect(keys({ ...base, filter: "WEB", collapsed: new Set(["acme"]) })).toEqual([
    "all",
    "org:acme",
    "repo:acme/web",
  ]);
});

test("scope, step, relTime", () => {
  expect(inScope(pr("acme/web"), "org:acme")).toBe(true);
  expect(inScope(pr("acme/web"), "org:acm")).toBe(false);
  expect(inScope(pr("acme/web"), "repo:acme/web")).toBe(true);
  expect(inScope(pr("acme/web"), "all")).toBe(true);
  expect(step(5, 0, -1)).toBe(0);
  expect(step(5, 2, Infinity)).toBe(4);
  expect(step(0, 0, 1)).toBe(-1);
  const now = Date.parse("2026-01-10T00:00:00Z");
  expect(relTime("2026-01-09T23:59:30Z", now)).toBe("now");
  expect(relTime("2026-01-09T21:00:00Z", now)).toBe("3h");
  expect(relTime("2026-01-07T00:00:00Z", now)).toBe("3d");
});

test("ghImageSrc: repo files through ghimg://, other images as they are", () => {
  expect(ghImageSrc("https://github.com/acme/web/blob/afa42c65/docs/a%20b.png?raw=true")).toBe(
    "ghimg://localhost/acme/web/afa42c65/docs/a%20b.png",
  );
  expect(ghImageSrc("https://github.com/acme/web/raw/main/x.png")).toBe(
    "ghimg://localhost/acme/web/main/x.png",
  );
  expect(ghImageSrc("https://camo.githubusercontent.com/abc")).toBeUndefined();
  expect(ghImageSrc("https://github.com/user-attachments/assets/123")).toBeUndefined();
});

test("canApprove: open, not mine, head not approved by me yet", () => {
  const pr = (extra: Partial<PrDetail> = {}) =>
    ({ state: "OPEN", headRefOid: "h2", author: { login: "bob" }, ...extra }) as PrDetail;
  const mine = (state: string, oid: string) => ({
    latestOpinionatedReviews: { nodes: [{ author: { login: "me" }, state, commit: { oid } }] },
  });
  expect(canApprove(pr(), "me")).toBe(true);
  expect(canApprove(pr(), null)).toBe(false);
  expect(canApprove(pr({ author: { login: "me" } }), "me")).toBe(false);
  expect(canApprove(pr({ state: "MERGED" }), "me")).toBe(false);
  expect(canApprove(pr(mine("APPROVED", "h2")), "me")).toBe(false);
  // New commits since, or changes requested: approve again.
  expect(canApprove(pr(mine("APPROVED", "h1")), "me")).toBe(true);
  expect(canApprove(pr(mine("CHANGES_REQUESTED", "h2")), "me")).toBe(true);
});

test("stackRows nests stacks as a tree and dims ancestors that aren't shown", () => {
  const mk = (n: number, base: string, head: string | null, extra: Partial<PrSummary> = {}) =>
    ({ id: `${n}`, number: n, repo: "o/r", baseRef: base, headRef: head, ...extra }) as PrSummary;
  // Newest first: 4 is on 3, which is on 2 (not mine); 5 is alone; 6/7 form a cycle.
  const all = [
    mk(4, "b3", "b4"),
    mk(5, "main", "b5"),
    mk(3, "b2", "b3", { context: true }),
    mk(9, "b4", null, { context: true }),
    mk(2, "main", "b2", { context: true }),
    mk(6, "b7", "b6"),
    mk(7, "b6", "b7"),
    mk(8, "b4", "b8", { repo: "o/other" }),
    // A branching stack keeps its full nesting.
    mk(20, "main", "c20", { repo: "o/b" }),
    mk(21, "c20", "c21", { repo: "o/b" }),
    mk(22, "c20", "c22", { repo: "o/b" }),
    mk(23, "c21", "c23", { repo: "o/b" }),
  ];
  const rows = stackRows(all, (p) => !p.context);
  expect(rows.map((r) => [r.pr.number, r.graph?.depth ?? null, r.dim])).toEqual([
    [2, 0, true],
    [3, 1, true],
    [4, 2, false],
    [5, null, false],
    [7, 0, false],
    [6, 1, false],
    [8, null, false],
    [20, 0, false],
    [21, 1, false],
    [23, 2, false],
    [22, 1, false],
  ]);
  const g = (n: number) => rows.find((r) => r.pr.number === n)!.graph;
  expect(g(2)).toEqual({ depth: 0, last: true, down: true, through: [] });
  expect(g(4)).toEqual({ depth: 2, last: true, down: false, through: [] });
  expect(g(20)).toEqual({ depth: 0, last: true, down: true, through: [] });
  expect(g(21)).toEqual({ depth: 1, last: false, down: true, through: [] });
  // 21 has a sibling below (22), so level 0's line runs past 21's child.
  expect(g(23)).toEqual({ depth: 2, last: true, down: false, through: [0] });
  expect(g(22)).toEqual({ depth: 1, last: true, down: false, through: [] });
  expect(stackRows(all, (p) => p.number === 5).map((r) => r.pr.number)).toEqual([5]);
});

test("shouldMarkRead: only an unread PR you picked, and not while browsing repos", () => {
  const unread = { id: "a", unread: true } as PrSummary;
  expect(shouldMarkRead(unread, "a", 1)).toBe(true);
  expect(shouldMarkRead(unread, "a", 2)).toBe(true);
  expect(shouldMarkRead(unread, "a", 0)).toBe(false);
  // The first row shown because nothing is picked, or the picked PR left the inbox.
  expect(shouldMarkRead(unread, null, 1)).toBe(false);
  expect(shouldMarkRead(unread, "gone", 1)).toBe(false);
  expect(shouldMarkRead({ ...unread, unread: false }, "a", 1)).toBe(false);
  expect(shouldMarkRead(null, null, 1)).toBe(false);
});
