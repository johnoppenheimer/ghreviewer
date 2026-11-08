import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { type CSSProperties, useEffect, useMemo, useRef, useState } from "react";
import { type PrDetail, type PrSummary, type Repo, api } from "./api";
import { type Action, createKeymap } from "./keys";
import { Inbox, Sidebar } from "./Lists";
import { inScope, matchesFilter, relTime, sidebarItems, step } from "./model";
import { type Command, Help, Palette, Settings } from "./Overlays";
import { PrView } from "./PrView";

const NO_REPOS: Repo[] = [];
const NO_PRS: PrSummary[] = [];

/** useState that survives relaunches (webview localStorage). */
function usePersisted<T>(key: string, initial: T) {
  const [value, setValue] = useState<T>(() => {
    try {
      const saved = localStorage.getItem(key);
      return saved === null ? initial : (JSON.parse(saved) as T);
    } catch {
      return initial;
    }
  });
  useEffect(() => localStorage.setItem(key, JSON.stringify(value)), [key, value]);
  return [value, setValue] as const;
}

function useNow(ms: number) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [ms]);
  return now;
}

/** GitHub contribution-graph style blocks, 2 rows, fading from full green to bg across 1/3 of the window. */
// Clockwise path around the edge of a 3x3 grid (index 4, the center, stays empty).
const SPINNER_PATH = [0, 1, 2, 5, 8, 7, 6, 3];

/** A lit pixel with a fading tail circling a 3x3 square. */
function PixelSpinner() {
  return (
    <span className="pixel-spinner" aria-hidden>
      {Array.from({ length: 9 }, (_, i) => {
        const step = SPINNER_PATH.indexOf(i);
        return (
          <i
            key={i}
            style={
              step < 0
                ? { animation: "none", opacity: 0 }
                : { animationDelay: `${(step - 8) * 0.1}s` }
            }
          />
        );
      })}
    </span>
  );
}

const SYNC_ROWS = 5;

function SyncBlocks() {
  // Square cells sized from the strip's measured height, so it follows the theme's font size.
  const ref = useRef<HTMLDivElement>(null);
  const [grid, setGrid] = useState({ cols: 0, cell: 0 });
  useEffect(() => {
    const el = ref.current!;
    const measure = () => {
      const cell = el.clientHeight / SYNC_ROWS;
      setGrid({ cols: Math.ceil(el.clientWidth / cell), cell });
    };
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  const { cols, cell } = grid;
  const cells = useMemo(
    () =>
      Array.from({ length: cols * SYNC_ROWS }, (_, i) => {
        const col = Math.floor(i / SYNC_ROWS);
        // 0-4 like GitHub's levels, jittered so it reads as activity rather than a smooth ramp.
        const level = Math.round((1 - col / cols) * 4 + (Math.random() - 0.5) * 1.6);
        return {
          level: Math.max(0, Math.min(4, level)) / 4,
          delay: (col / cols) * 1500 + Math.random() * 300,
        };
      }),
    [cols],
  );
  return (
    <div
      ref={ref}
      className="sync-blocks"
      style={{ gridAutoColumns: `${cell}px`, gridTemplateRows: `repeat(${SYNC_ROWS}, 1fr)` }}
      aria-hidden
    >
      {cells.map((c, i) => (
        <i key={i} style={{ "--l": c.level, animationDelay: `${c.delay}ms` } as CSSProperties} />
      ))}
    </div>
  );
}

export default function App() {
  const qc = useQueryClient();
  const now = useNow(30_000);
  const status = useQuery({ queryKey: ["status"], queryFn: api.status }).data;
  const config = useQuery({ queryKey: ["config"], queryFn: api.config }).data;
  const repos = useQuery({ queryKey: ["repos"], queryFn: api.repos }).data ?? NO_REPOS;
  const inbox = useQuery({ queryKey: ["inbox"], queryFn: api.inbox }).data ?? NO_PRS;

  const [focus, setFocus] = useState(1);
  const [sideKey, setSideKey] = useState("all");
  const [prId, setPrId] = useState<string | null>(null);
  const [filters, setFilters] = usePersisted("filters", ["", ""]);
  const [filterOpen, setFilterOpen] = useState<number | null>(null);
  const [overlay, setOverlay] = useState<null | "help" | "palette" | "settings">(null);
  const [collapsed, setCollapsed] = useState(new Set<string>());
  const [showHidden, setShowHidden] = useState(false);
  const [onlyWithPrs, setOnlyWithPrs] = usePersisted("onlyWithPrs", false);
  const [uiError, setUiError] = useState<string | null>(null);
  // PR whose worktree was just created; its header flashes green for a moment.
  const [wtCreated, setWtCreated] = useState<string | null>(null);
  const [paletteQuery, setPaletteQuery] = useState("");
  const paneRef = useRef<HTMLDivElement>(null);
  const keymap = useMemo(createKeymap, []);

  const fire = (p: Promise<unknown>) => p.catch((e) => setUiError(String(e)));
  const setFilter = (col: number, value: string) =>
    setFilters((f) => f.map((v, i) => (i === col ? value : v)));

  useEffect(() => {
    const unlisten = [
      listen("changed", () => qc.invalidateQueries({ predicate: (q) => q.queryKey[0] !== "pr" })),
      listen<{ id: string; detail: PrDetail }>("pr-updated", (e) =>
        qc.setQueriesData({ queryKey: ["pr", e.payload.id] }, e.payload.detail),
      ),
    ];
    return () => unlisten.forEach((p) => p.then((f) => f()));
  }, [qc]);

  useEffect(() => {
    if (!config) return;
    const style = document.documentElement.style;
    for (const theme of Object.values(config.themes))
      for (const k of Object.keys(theme)) style.removeProperty(`--${k}`);
    for (const [k, v] of Object.entries(config.themes[config.theme] ?? {}))
      style.setProperty(`--${k}`, v);
  }, [config]);

  const items = useMemo(
    () =>
      sidebarItems({
        repos: repos.map((r) => r.nameWithOwner),
        inbox,
        pinned: config?.repos.pinned ?? [],
        hidden: config?.repos.hidden ?? [],
        collapsed,
        showHidden,
        onlyWithPrs,
        filter: filters[0],
      }),
    [repos, inbox, config, collapsed, showHidden, onlyWithPrs, filters],
  );
  const sideIdx = Math.max(
    0,
    items.findIndex((i) => i.key === sideKey),
  );
  const side = items[sideIdx];
  const prs = useMemo(
    () => inbox.filter((pr) => inScope(pr, side.key) && matchesFilter(pr, filters[1])),
    [inbox, side.key, filters],
  );
  const prIdx = Math.max(
    0,
    prs.findIndex((p) => p.id === prId),
  );
  const selected = prs[prIdx] ?? null;

  // Keyed by what the inbox knows changed, so a PR is only re-fetched when it moved.
  const detail = useQuery({
    queryKey: ["pr", selected?.id, selected?.updatedAt, selected?.checks],
    queryFn: () => api.pr(selected!.id),
    enabled: !!selected,
  });

  const browsingRepos = focus === 0;
  useEffect(() => {
    if (selected?.unread && !browsingRepos) fire(api.setUnread(selected.id, false));
    // Only on opening a PR or on new activity while it's open, so `u` sticks.
    // Not while moving through repos: that only previews each repo's first PR.
  }, [selected?.id, selected?.lastOthersActivityAt, browsingRepos]);

  useEffect(() => paneRef.current?.scrollTo({ top: 0 }), [selected?.id]);

  const move = (delta: number) => {
    if (focus === 0) {
      const i = step(items.length, sideIdx, delta);
      if (i >= 0) setSideKey(items[i].key);
    } else if (focus === 1) {
      const i = step(prs.length, prIdx, delta);
      if (i >= 0) setPrId(prs[i].id);
    } else if (Number.isFinite(delta)) {
      paneRef.current?.scrollBy({ top: delta * 60 });
    } else {
      paneRef.current?.scrollTo({ top: delta > 0 ? paneRef.current.scrollHeight : 0 });
    }
  };

  const toggleUnread = () => selected && fire(api.setUnread(selected.id, !selected.unread));
  const openSelected = () => selected && fire(openUrl(selected.url));

  // Local worktree of the PR branch; cheap (`git worktree list`), so re-checked on every visit.
  const branch = detail.data?.id === selected?.id ? detail.data?.headRefName : undefined;
  const wt = useQuery({
    queryKey: ["worktree", selected?.repo, branch],
    queryFn: () => api.worktree(selected!.repo, branch!),
    enabled: !!selected && !!branch,
    staleTime: 0,
    refetchOnWindowFocus: true,
  }).data;
  const createWt = useMutation({
    mutationFn: (pr: PrSummary) => api.createWorktree(pr.repo, pr.number),
    onSuccess: (_, pr) => {
      qc.invalidateQueries({ queryKey: ["worktree"] });
      setWtCreated(pr.id);
      setTimeout(() => setWtCreated((id) => (id === pr.id ? null : id)), 2000);
    },
    onError: (e) => setUiError(String(e)),
  });
  const creatingWt = createWt.isPending && createWt.variables?.id === selected?.id;
  const headState = creatingWt ? "busy" : wtCreated && wtCreated === selected?.id ? "done" : "";
  const canCreateWt = !!(selected && wt?.clone && wt.wt && !wt.path && !createWt.isPending);
  const openers = config?.open_with ?? [];
  const openWorktree = (opener: number) =>
    wt?.path && selected && branch && fire(api.openWorktree(opener, wt.path, selected, branch));
  const worktreeAction = () => {
    if (wt?.path) openWorktree(0);
    else if (canCreateWt) createWt.mutate(selected!);
  };
  let wtHead = null;
  if (wt?.path)
    wtHead = (
      <button
        className="head-btn"
        title={`${wt.path}\nOpen in ${openers[0]?.name ?? "?"} (w) · all options (W)`}
        onClick={worktreeAction}
      >
        {wt.path.split("/").pop()}
      </button>
    );
  else if (creatingWt)
    wtHead = (
      <span className="with-spinner">
        <PixelSpinner />
        creating worktree…
      </span>
    );
  else if (canCreateWt)
    wtHead = (
      <button
        className="head-btn"
        title={`wt switch pr:${selected?.number} in ${wt?.clone}`}
        onClick={worktreeAction}
      >
        + worktree (w)
      </button>
    );

  const run = (action: Action) => {
    const pane = paneRef.current;
    switch (action) {
      case "down":
        return move(1);
      case "up":
        return move(-1);
      case "top":
        return move(-Infinity);
      case "bottom":
        return move(Infinity);
      case "left":
        return setFocus((f) => Math.max(0, f - 1));
      case "right":
      case "enter":
        return setFocus((f) => Math.min(2, f + 1));
      case "pageDown":
      case "pageUp":
        return pane?.scrollBy({ top: ((action === "pageDown" ? 1 : -1) * pane.clientHeight) / 2 });
      case "filter":
        return focus < 2 && setFilterOpen(focus);
      case "escape":
        setUiError(null);
        return focus < 2 && setFilter(focus, "");
      case "open":
        return openSelected();
      case "refresh":
        return fire(api.refresh());
      case "toggleUnread":
        return toggleUnread();
      case "help":
        return setOverlay(action);
      case "palette":
        setPaletteQuery("");
        return setOverlay("palette");
      case "worktreeMenu":
        if (!wt?.path) return;
        setPaletteQuery("open worktree in ");
        return setOverlay("palette");
      case "pin":
      case "hide":
        if (focus !== 0 || side.kind !== "repo") return;
        return fire(
          action === "pin"
            ? api.setRepoFlag(side.repo, "pinned", !side.pinned)
            : api.setRepoFlag(side.repo, "hidden", !side.hidden),
        );
      case "toggleHidden":
        return setShowHidden((v) => !v);
      case "worktree":
        return worktreeAction();
      case "toggleEmpty":
        return focus === 0 && setOnlyWithPrs((v) => !v);
      case "collapse":
        if (focus !== 0 || side.kind !== "org") return;
        return setCollapsed((c) => {
          const next = new Set(c);
          if (!next.delete(side.org)) next.add(side.org);
          return next;
        });
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Cmd shortcuts work everywhere (and Cmd-R must never reload the webview).
      if (e.metaKey) {
        const action = keymap(e);
        if (action) {
          e.preventDefault();
          run(action);
        }
        return;
      }
      if (overlay) {
        if (e.key === "Escape" || overlay === "help") {
          e.preventDefault();
          setOverlay(null);
        }
        return;
      }
      if ((e.target as HTMLElement).closest("input, textarea, select")) return;
      const action = keymap(e);
      if (!action) return;
      e.preventDefault();
      run(action);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const commands: Command[] = [
    { label: "refresh", run: () => fire(api.refresh()) },
    { label: "settings", run: () => setOverlay("settings") },
    { label: "open config file", run: () => fire(api.openConfig()) },
    { label: "open PR in browser", run: openSelected },
    { label: "toggle unread", run: toggleUnread },
    ...(wt?.path
      ? openers.map((o, i) => ({ label: `open worktree in ${o.name}`, run: () => openWorktree(i) }))
      : []),
    ...(canCreateWt ? [{ label: "create worktree", run: worktreeAction }] : []),
    {
      label: showHidden ? "hide hidden repos" : "show hidden repos",
      run: () => setShowHidden((v) => !v),
    },
    {
      label: onlyWithPrs ? "show repos without PRs" : "hide repos without PRs",
      run: () => setOnlyWithPrs((v) => !v),
    },
    ...Object.keys(config?.themes ?? {}).map((theme) => ({
      label: `theme ${theme}`,
      run: () => config && fire(api.saveSettings({ ...config, theme })),
    })),
    { label: "help", run: () => setOverlay("help") },
  ];

  const scopeTitle = side.kind === "org" ? side.org : side.kind === "repo" ? side.repo : "Inbox";
  let empty = "Nothing here.";
  if (status?.error && !inbox.length) empty = status.error;
  else if (!config?.rules.length) empty = "No rules yet. Press : then settings.";
  else if (status?.syncing && !inbox.length) empty = "Syncing…";

  const error = uiError ?? status?.configError ?? status?.error;
  const synced = status?.lastSyncMs ? relTime(status.lastSyncMs, now) : null;

  return (
    <div className="app">
      <Sidebar
        items={items}
        selected={side.key}
        focused={focus === 0}
        showHidden={showHidden}
        onlyWithPrs={onlyWithPrs}
        filter={filters[0]}
        filterOpen={filterOpen === 0}
        onFilter={(v) => setFilter(0, v)}
        onFilterDone={() => setFilterOpen(null)}
        onSelect={(key) => {
          setFocus(0);
          setSideKey(key);
        }}
      />
      <Inbox
        title={scopeTitle}
        prs={prs}
        selected={selected?.id ?? null}
        focused={focus === 1}
        empty={empty}
        now={now}
        filter={filters[1]}
        filterOpen={filterOpen === 1}
        onFilter={(v) => setFilter(1, v)}
        onFilterDone={() => setFilterOpen(null)}
        onSelect={(id) => {
          setFocus(1);
          setPrId(id);
        }}
      />
      <PrView
        summary={selected}
        detail={detail.data}
        error={detail.error ? String(detail.error) : null}
        focused={focus === 2}
        now={now}
        paneRef={paneRef}
        headRight={wtHead}
        headState={headState}
        onFocus={() => setFocus(2)}
        onLink={(url) => fire(openUrl(url))}
      />
      <div className={`status ${status?.syncing ? "syncing" : ""}`}>
        {status?.syncing && <SyncBlocks />}
        <span>{status?.viewer ? `@${status.viewer}` : "…"}</span>
        <span className={status?.syncing ? "ok" : ""}>
          {status?.syncing
            ? "syncing…"
            : synced
              ? `synced ${synced === "now" ? "just now" : `${synced} ago`}`
              : "not synced"}
        </span>
        {status?.rateRemaining != null && <span>api {status.rateRemaining}</span>}
        {error && <span className="fail">{error}</span>}
        <span className="grow">? help · : commands</span>
      </div>
      {overlay === "help" && <Help onClose={() => setOverlay(null)} />}
      {overlay === "palette" && (
        <Palette commands={commands} initialQuery={paletteQuery} onClose={() => setOverlay(null)} />
      )}
      {overlay === "settings" && config && (
        <Settings config={config} onClose={() => setOverlay(null)} />
      )}
    </div>
  );
}
