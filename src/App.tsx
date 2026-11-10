import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useMemo, useRef, useState } from "react";
import { type PrDetail, type PrSummary, type Repo, type Run, api } from "./api";
import { type Action, createKeymap } from "./keys";
import { Inbox, Sidebar } from "./Lists";
import {
  canApprove,
  inScope,
  matchesFilter,
  relTime,
  runItems,
  sidebarItems,
  stackRows,
  step,
} from "./model";
import { type Command, Help, Palette, Settings } from "./Overlays";
import { PrView } from "./PrView";
import { PixelSpinner, RunView, TabLabel, isStale } from "./RunView";

const NO_REPOS: Repo[] = [];
const NO_PRS: PrSummary[] = [];
const NO_RUNS: Run[] = [];

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
const SYNC_ROWS = 5;

// Full-screen triangle; no vertex buffer needed.
const SYNC_VERT = `#version 300 es
void main() { gl_Position = vec4(gl_VertexID == 1 ? 3.0 : -1.0, gl_VertexID == 2 ? 3.0 : -1.0, 0.0, 1.0); }`;

// One cell per `cell` pixels, each with its own level and flicker phase, all on the GPU.
const SYNC_FRAG = `#version 300 es
precision highp float;
uniform vec2 res;
uniform float cell, time, seed;
uniform vec3 ok, bg;
out vec4 color;
float hash(vec2 p) { return fract(sin(dot(p, vec2(12.9898, 78.233)) + seed) * 43758.5453); }
void main() {
  vec2 c = floor(gl_FragCoord.xy / cell);
  float x = c.x / ceil(res.x / cell);
  // 0-4 like GitHub's levels, jittered so it reads as activity rather than a smooth ramp.
  float level = clamp(floor((1.0 - x) * 4.0 + (hash(c) - 0.5) * 1.6 + 0.5), 0.0, 4.0) / 4.0;
  // Like CSS \`1.4s steps(3) infinite alternate both\` from opacity 1 to 0.35, delayed per column.
  float t = max(0.0, time - x * 1500.0 - hash(c + 7.0) * 300.0) / 1400.0;
  float p = mod(floor(t), 2.0) == 0.0 ? fract(t) : 1.0 - fract(t);
  float a = 1.0 - 0.65 * floor(p * 3.0) / 3.0;
  color = vec4(mix(bg, ok, level) * a, a);
}`;

function SyncBlocks() {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    // Created here so each mount gets a fresh context; the cleanup loses the previous one.
    const canvas = ref.current!.appendChild(document.createElement("canvas"));
    const gl = canvas.getContext("webgl2", { antialias: false, depth: false });
    if (!gl) return () => canvas.remove();
    const program = gl.createProgram();
    for (const [type, src] of [
      [gl.VERTEX_SHADER, SYNC_VERT],
      [gl.FRAGMENT_SHADER, SYNC_FRAG],
    ] as const) {
      const shader = gl.createShader(type)!;
      gl.shaderSource(shader, src);
      gl.compileShader(shader);
      gl.attachShader(program, shader);
    }
    gl.linkProgram(program);
    gl.useProgram(program);
    const u = (name: string) => gl.getUniformLocation(program, name);
    gl.uniform1f(u("seed"), Math.random() * 100);
    // Theme colors through a 1px 2D canvas, so any CSS color syntax works.
    // shortcut: read on resize only, so a theme switch mid-sync shows from the next sync.
    const px = Object.assign(document.createElement("canvas"), { width: 1, height: 1 });
    const ctx = px.getContext("2d", { willReadFrequently: true })!;
    const rgb = (name: string) => {
      ctx.fillStyle = getComputedStyle(canvas).getPropertyValue(name);
      ctx.fillRect(0, 0, 1, 1);
      return Array.from(ctx.getImageData(0, 0, 1, 1).data.slice(0, 3), (v) => v / 255);
    };

    const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
    const start = performance.now();
    let frame = 0;
    const draw = () => {
      cancelAnimationFrame(frame);
      gl.uniform1f(u("time"), still ? 0 : performance.now() - start);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      if (!still) frame = requestAnimationFrame(draw);
    };
    // Square cells sized from the strip's measured height, so it follows the theme's font size.
    const measure = () => {
      canvas.width = Math.round(canvas.clientWidth * devicePixelRatio);
      canvas.height = Math.round(canvas.clientHeight * devicePixelRatio);
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.uniform2f(u("res"), canvas.width, canvas.height);
      gl.uniform1f(u("cell"), canvas.height / SYNC_ROWS);
      gl.uniform3fv(u("ok"), rgb("--ok"));
      gl.uniform3fv(u("bg"), rgb("--bg"));
      draw();
    };
    const observer = new ResizeObserver(measure);
    observer.observe(canvas);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      // Frees the GPU context now instead of whenever it's garbage collected.
      gl.getExtension("WEBGL_lose_context")?.loseContext();
      canvas.remove();
    };
  }, []);
  return <div ref={ref} className="sync-blocks" aria-hidden />;
}

export default function App() {
  const qc = useQueryClient();
  const now = useNow(30_000);
  const status = useQuery({ queryKey: ["status"], queryFn: api.status }).data;
  const config = useQuery({ queryKey: ["config"], queryFn: api.config }).data;
  const repos = useQuery({ queryKey: ["repos"], queryFn: api.repos }).data ?? NO_REPOS;
  const inbox = useQuery({ queryKey: ["inbox"], queryFn: api.inbox }).data ?? NO_PRS;
  const activeRuns =
    useQuery({ queryKey: ["activeRuns"], queryFn: api.activeRuns }).data ?? NO_RUNS;

  const [focus, setFocus] = useState(1);
  const [sideKey, setSideKey] = useState("all");
  const [prId, setPrId] = useState<string | null>(null);
  const [filters, setFilters] = usePersisted("filters", ["", ""]);
  // Inbox rule tab: "" for all rules, else a rule name.
  const [ruleName, setRuleName] = usePersisted("rule", "");
  const [filterOpen, setFilterOpen] = useState<number | null>(null);
  const [overlay, setOverlay] = useState<null | "help" | "palette" | "settings">(null);
  const [collapsed, setCollapsed] = useState(new Set<string>());
  const [showHidden, setShowHidden] = useState(false);
  const [onlyWithPrs, setOnlyWithPrs] = usePersisted("onlyWithPrs", false);
  const [uiError, setUiError] = useState<string | null>(null);
  // PR whose worktree was just created; its header flashes green for a moment.
  const [wtCreated, setWtCreated] = useState<string | null>(null);
  const [paletteQuery, setPaletteQuery] = useState("");
  // PR pane tab: "" for the PR itself, else a prompt name.
  const [tabName, setTabName] = usePersisted("prTab", "");
  const [picks, setPicks] = useState<Record<string, boolean[]>>({});
  const [cursor, setCursor] = useState(0);
  // File tab of the prompt tab (window 1920px+); falls back to the run's first file.
  const [tabFile, setTabFile] = useState<string>();
  const paneRef = useRef<HTMLDivElement>(null);
  const keymap = useMemo(createKeymap, []);

  const fire = (p: Promise<unknown>) => p.catch((e) => setUiError(String(e)));
  const setFilter = (col: number, value: string) =>
    setFilters((f) => f.map((v, i) => (i === col ? value : v)));

  useEffect(() => {
    const unlisten = [
      // Worktrees cost git/wt processes to look up, so only refetch them when one was created.
      listen("changed", () =>
        qc.invalidateQueries({
          predicate: (q) => !["pr", "worktree"].includes(String(q.queryKey[0])),
        }),
      ),
      listen("status", () => qc.invalidateQueries({ queryKey: ["status"] })),
      listen("worktree-created", () => qc.invalidateQueries({ queryKey: ["worktree"] })),
      listen<{ id: string; detail: PrDetail }>("pr-updated", (e) =>
        qc.setQueriesData({ queryKey: ["pr", e.payload.id] }, e.payload.detail),
      ),
      listen<string>("run-updated", (e) => {
        qc.invalidateQueries({ queryKey: ["runs", e.payload] });
        qc.invalidateQueries({ queryKey: ["activeRuns"] });
      }),
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

  const ruleNames = useMemo(() => config?.rules.map((r) => r.name) ?? [], [config]);
  // A renamed or removed rule falls back to all.
  const rule = ruleNames.includes(ruleName) ? ruleName : "";
  const cycleRule = () => {
    const all = ["", ...ruleNames];
    setRuleName(all[(all.indexOf(rule) + 1) % all.length]);
  };
  // The inbox proper; `context` PRs only show as greyed stack parents.
  const inRule = (pr: PrSummary) => !pr.context && (!rule || pr.rules.includes(rule));
  const mine = useMemo(() => inbox.filter(inRule), [inbox, rule]);
  const items = useMemo(
    () => [
      ...sidebarItems({
        repos: repos.map((r) => r.nameWithOwner),
        inbox: mine,
        pinned: config?.repos.pinned ?? [],
        hidden: config?.repos.hidden ?? [],
        collapsed,
        showHidden,
        onlyWithPrs,
        filter: filters[0],
      }),
      ...runItems(activeRuns),
    ],
    [activeRuns, repos, mine, config, collapsed, showHidden, onlyWithPrs, filters],
  );
  const sideIdx = Math.max(
    0,
    items.findIndex((i) => i.key === sideKey),
  );
  const side = items[sideIdx];
  const rows = useMemo(
    () =>
      stackRows(
        inbox,
        (pr) => inRule(pr) && inScope(pr, side.key) && matchesFilter(pr, filters[1]),
      ),
    [inbox, side.key, filters, rule],
  );
  const prs = useMemo(() => rows.map((r) => r.pr), [rows]);
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
    } else if (pickable && Number.isFinite(delta) && review?.findings[cursor + delta]) {
      // j/k step through findings; past either end they scroll the pane again.
      setCursor(cursor + delta);
      paneRef.current
        ?.querySelector(`[data-finding="${cursor + delta}"]`)
        ?.scrollIntoView({ block: "nearest" });
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

  // Prompt tabs and their latest runs on the selected PR.
  const prompts = config?.prompts ?? [];
  const pr = detail.data?.id === selected?.id ? detail.data : undefined;
  const runs = useQuery({
    queryKey: ["runs", selected?.id],
    queryFn: () => api.runs(selected!.id),
    enabled: !!selected,
  }).data;
  const runOf = (name: string) => runs?.find((r) => r.prompt === name);
  const tabNames = ["", ...prompts.map((p) => p.name)];
  const tab = Math.max(0, tabNames.indexOf(tabName));
  const tabPrompt = prompts[tab - 1];
  const switchTab = (i: number) => {
    setTabName(tabNames[(i + tabNames.length) % tabNames.length]);
    paneRef.current?.scrollTo({ top: 0 });
  };
  // Jumps from a sidebar run to its PR's tab, which marks it read.
  const openRun = (run: Run) => {
    const target = inbox.find((p) => p.id === run.prId);
    // PR left the inbox or the prompt left the config: nothing to show, just clear it.
    if (!target || !prompts.some((p) => p.name === run.prompt))
      return fire(api.markRunSeen(run.prId, run.prompt));
    if (!matchesFilter(target, filters[1])) setFilter(1, "");
    if (!inRule(target)) setRuleName("");
    setSideKey("all");
    setPrId(run.prId);
    setTabName(run.prompt);
    setFocus(2);
  };
  // A finished run counts as read once its tab is on screen.
  const tabRun = tabPrompt && runOf(tabPrompt.name);
  const readable = !!pr && !!tabRun && !tabRun.seen && tabRun.status !== "running";
  useEffect(() => {
    if (readable && tabRun) fire(api.markRunSeen(tabRun.prId, tabRun.prompt));
  }, [readable, tabRun?.prId, tabRun?.prompt, tabRun?.startedAt]);
  const runPrompt = (name: string) => pr && fire(api.runPrompt(name, pr));
  const files = tabRun?.files ?? [];
  const shownFile = tabFile && files.includes(tabFile) ? tabFile : files[0];
  const switchFile = (delta: number) =>
    files.length &&
    setTabFile(files[(files.indexOf(shownFile!) + delta + files.length) % files.length]);

  // Findings of the tab's run: which are picked (until relaunch) and the keyboard cursor.
  const review = tabRun?.review;
  const reviewKey = tabRun ? `${tabRun.prId}/${tabRun.prompt}/${tabRun.startedAt}` : "";
  const picked = picks[reviewKey] ?? review?.findings.map((f) => f.severity !== "nit") ?? [];
  const pickable = !!review?.findings.length && !review.posted;
  useEffect(() => setCursor(0), [reviewKey]);
  const togglePick = (i: number) =>
    setPicks((p) => ({ ...p, [reviewKey]: picked.map((v, j) => (i === j ? !v : v)) }));
  const post = useMutation({
    mutationFn: (v: { key: string; run: Run; picked: number[]; approve: boolean }) =>
      api.postReview(v.run.prId, v.run.prompt, v.picked, v.approve),
    onError: (e) => setUiError(String(e)),
  });
  const posting = post.isPending && post.variables?.key === reviewKey;
  const postReview = (approve: boolean) =>
    tabRun &&
    !post.isPending &&
    post.mutate({
      key: reviewKey,
      run: tabRun,
      picked: picked.flatMap((v, i) => (v ? [i] : [])),
      approve,
    });
  // Approving straight from the PR tab, without a prompt.
  const approval = useMutation({
    mutationFn: (id: string) => api.approve(id),
    onError: (e) => setUiError(String(e)),
  });
  const approvable = tab === 0 && !!pr && canApprove(pr, status?.viewer) && !approval.isPending;
  const approvePr = () => approvable && approval.mutate(pr.id);
  let approveBtn = null;
  if (approval.isPending && approval.variables === pr?.id)
    approveBtn = (
      <span className="warn with-spinner">
        <PixelSpinner />
        approving…
      </span>
    );
  else if (approvable)
    approveBtn = (
      <button className="head-btn" title="Approve on GitHub, with no comment" onClick={approvePr}>
        approve
      </button>
    );
  const tabs = tabNames.map((name) =>
    name ? (
      <TabLabel name={name} run={runOf(name)} stale={isStale(runOf(name), pr?.headRefOid)} />
    ) : (
      "PR"
    ),
  );
  const tabBody = tabPrompt && selected && (
    <RunView
      prompt={tabPrompt}
      run={runOf(tabPrompt.name)}
      stale={isStale(runOf(tabPrompt.name), pr?.headRefOid)}
      hasWorktree={!!wt?.path}
      now={now}
      picker={{
        picked,
        cursor,
        posting,
        onToggle: togglePick,
        onPost: postReview,
        onOpen: (url) => fire(openUrl(url)),
      }}
      shownFile={shownFile}
      onFile={setTabFile}
      onRun={() => runPrompt(tabPrompt.name)}
      onOpenFile={(path) => fire(api.openPath(selected.id, path))}
    />
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
        if (focus === 0 && side.kind === "run") return openRun(side.run);
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
      case "prevTab":
        return switchTab(tab - 1);
      case "nextTab":
        return switchTab(tab + 1);
      case "prevFile":
        return switchFile(-1);
      case "nextFile":
        return switchFile(1);
      case "runPrompt":
        return tabPrompt && runPrompt(tabPrompt.name);
      case "continueRun":
        return tabPrompt && selected && fire(api.continueRun(selected.id, tabPrompt.name));
      case "toggleEmpty":
        return focus === 0 && setOnlyWithPrs((v) => !v);
      case "cycleRule":
        return cycleRule();
      case "collapse":
        if (focus === 2 && pickable) return togglePick(cursor);
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
    ...(approvable ? [{ label: "approve PR", run: approvePr }] : []),
    ...(wt?.path
      ? openers.map((o, i) => ({ label: `open worktree in ${o.name}`, run: () => openWorktree(i) }))
      : []),
    ...(canCreateWt ? [{ label: "create worktree", run: worktreeAction }] : []),
    ...prompts.flatMap((p, i) => {
      const run = runOf(p.name);
      const show = () => switchTab(i + 1);
      return [
        ...(pr ? [{ label: `run ${p.name}`, run: () => (show(), runPrompt(p.name)) }] : []),
        ...(run?.status === "running" && selected
          ? [{ label: `cancel ${p.name}`, run: () => fire(api.cancelRun(selected.id, p.name)) }]
          : []),
        ...(run?.sessionId && selected
          ? [
              {
                label: `continue ${p.name} in terminal`,
                run: () => fire(api.continueRun(selected.id, p.name)),
              },
            ]
          : []),
      ];
    }),
    ...(review && !review.posted && !post.isPending
      ? [
          ...(picked.some(Boolean)
            ? [
                {
                  label: `post review: comment with ${picked.filter(Boolean).length} findings`,
                  run: () => postReview(false),
                },
              ]
            : []),
          { label: "post review: approve", run: () => postReview(true) },
        ]
      : []),
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
  if (status?.error && !mine.length) empty = status.error;
  else if (!config?.rules.length) empty = "No rules yet. Press : then settings.";
  else if (status?.syncing && !mine.length) empty = "Syncing…";

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
          const run = items.find((i) => i.key === key);
          if (run?.kind === "run") return openRun(run.run);
          setFocus(0);
          setSideKey(key);
        }}
      />
      <Inbox
        title={scopeTitle}
        rows={rows}
        selected={selected?.id ?? null}
        focused={focus === 1}
        empty={empty}
        now={now}
        rules={ruleNames}
        rule={rule}
        onRule={setRuleName}
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
        approve={approveBtn}
        tabs={tabs}
        tab={tab}
        tabBody={tabBody}
        onTab={switchTab}
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
