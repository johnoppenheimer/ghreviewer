import { expect, test } from "bun:test";
import type { PrSummary } from "./api";
import { createKeymap } from "./keys";
import { inScope, relTime, sidebarItems, step } from "./model";

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
  expect(km(key("r"))).toBeNull();
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
