import { expect, test } from "bun:test";
import type { PrSummary } from "./api";
import { createKeymap } from "./keys";
import { inScope, relTime, sidebarItems, stackRows, step } from "./model";

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
  expect(km(key("c", { metaKey: true }))).toBeNull();
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

test("stackRows lays stacks out as a graph and dims ancestors that aren't shown", () => {
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
  expect(rows.map((r) => [r.pr.number, r.graph?.col ?? null, r.dim])).toEqual([
    [2, 0, true],
    [3, 0, true],
    [4, 0, false],
    [5, null, false],
    [7, 0, false],
    [6, 0, false],
    [8, null, false],
    [20, 0, false],
    [21, 0, false],
    [23, 0, false],
    [22, 1, false],
  ]);
  const g = (n: number) => rows.find((r) => r.pr.number === n)!.graph;
  expect(g(2)).toEqual({ col: 0, cols: 1, up: false, down: true, through: [], branches: [] });
  expect(g(4)).toEqual({ col: 0, cols: 1, up: true, down: false, through: [], branches: [] });
  expect(g(20)).toEqual({ col: 0, cols: 2, up: false, down: true, through: [], branches: [1] });
  expect(g(21)).toEqual({ col: 0, cols: 2, up: true, down: true, through: [1], branches: [] });
  expect(g(23)).toEqual({ col: 0, cols: 2, up: true, down: false, through: [1], branches: [] });
  expect(g(22)).toEqual({ col: 1, cols: 2, up: true, down: false, through: [], branches: [] });
  expect(stackRows(all, (p) => p.number === 5).map((r) => r.pr.number)).toEqual([5]);
});
