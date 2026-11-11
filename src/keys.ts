export type Action =
  | "down"
  | "up"
  | "left"
  | "right"
  | "top"
  | "bottom"
  | "pageDown"
  | "pageUp"
  | "enter"
  | "escape"
  | "filter"
  | "open"
  | "refresh"
  | "toggleUnread"
  | "help"
  | "palette"
  | "pin"
  | "hide"
  | "toggleHidden"
  | "toggleEmpty"
  | "cycleRule"
  | "worktree"
  | "worktreeMenu"
  | "prevTab"
  | "nextTab"
  | "prevFile"
  | "nextFile"
  | "runPrompt"
  | "continueRun"
  | "fix"
  | "approve"
  | "collapse";

type Key = { key: string; ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey: boolean };

const SINGLE: Record<string, Action> = {
  j: "down",
  ArrowDown: "down",
  k: "up",
  ArrowUp: "up",
  h: "left",
  ArrowLeft: "left",
  l: "right",
  ArrowRight: "right",
  G: "bottom",
  Enter: "enter",
  Escape: "escape",
  "/": "filter",
  o: "open",
  u: "toggleUnread",
  "?": "help",
  ":": "palette",
  p: "pin",
  x: "hide",
  H: "toggleHidden",
  e: "toggleEmpty",
  t: "cycleRule",
  w: "worktree",
  W: "worktreeMenu",
  "[": "prevTab",
  "]": "nextTab",
  "{": "prevFile",
  "}": "nextFile",
  r: "runPrompt",
  c: "continueRun",
  f: "fix",
  a: "approve",
  " ": "collapse",
};

/** Stateful key decoder (remembers a pending `g` for `gg`). */
export function createKeymap() {
  let pending = false;
  return (e: Key): Action | null => {
    if (e.metaKey) return e.key === "r" ? "refresh" : null;
    if (e.altKey) return null;
    const wasPending = pending;
    pending = false;
    if (e.ctrlKey) return e.key === "d" ? "pageDown" : e.key === "u" ? "pageUp" : null;
    if (e.key === "g") {
      pending = !wasPending;
      return wasPending ? "top" : null;
    }
    if (e.key === "Tab") return e.shiftKey ? "left" : "right";
    return SINGLE[e.key] ?? null;
  };
}

export const HELP: [string, string][] = [
  ["j / k", "next / previous (findings in a prompt tab)"],
  ["h / l, Tab / S-Tab", "focus previous / next column"],
  ["Enter", "focus next column"],
  ["gg / G", "top / bottom"],
  ["Ctrl-d / Ctrl-u", "scroll PR half a page"],
  ["/", "filter focused list"],
  ["Esc", "clear filter / close"],
  ["o", "open PR in browser"],
  ["Cmd-r", "refresh now"],
  ["u", "toggle unread"],
  ["a", "approve the PR, with an optional message (PR tab)"],
  ["t", "inbox: next rule (all, then each rule)"],
  ["w", "open PR worktree (first open_with), or create it (worktrunk)"],
  ["W", "open PR worktree with… (all open_with entries)"],
  ["[ / ]", "previous / next PR tab (the PR, its Jira tickets, then each prompt)"],
  ["{ / }", "previous / next file of the prompt tab (window 1920px+), or ticket of the Jira tab"],
  ["r", "run or re-run the tab's prompt (creates the worktree first)"],
  ["c", "continue the tab's agent session in a terminal"],
  ["f", "send the picked findings to the [fix] agent"],
  ["p / x", "pin / hide repo (sidebar)"],
  ["H", "show hidden repos"],
  ["e", "hide repos without PRs (sidebar)"],
  ["Space", "collapse org (sidebar) / pick finding (prompt tab)"],
  [":", "command palette"],
  ["?", "this help"],
];
