import { convertFileSrc } from "@tauri-apps/api/core";
import DOMPurify from "dompurify";
import { marked } from "marked";
import { useEffect, useMemo, useRef, useState } from "react";
import type { Prompt, Review, Run } from "./api";
import { type BotReview, relTime } from "./model";

// Clockwise path around the edge of a 3x3 grid (index 4, the center, stays empty).
const SPINNER_PATH = [0, 1, 2, 5, 8, 7, 6, 3];

/** A lit pixel with a fading tail circling a 3x3 square. */
export function PixelSpinner() {
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

/** Whether a finished run predates the PR's current head commit. */
export const isStale = (run: Run | undefined, headOid: string | undefined) =>
  !!(run?.headOid && headOid && run.headOid !== headOid && run.status !== "running");

/** Tab label: prompt name plus a mark for its run's state. */
export function TabLabel({ name, run, stale }: { name: string; run?: Run; stale: boolean }) {
  let mark = null;
  if (run?.status === "running") mark = <PixelSpinner />;
  else if (run?.status === "error") mark = <span className="fail">!</span>;
  else if (stale) mark = <span className="warn">*</span>;
  return (
    <span className="with-spinner">
      {name}
      {mark && <span className="tab-mark">{mark}</span>}
    </span>
  );
}

// Agent text gets nothing that loads a URL: a remote address could carry out what the agent read.
const AGENT_MD = {
  RETURN_DOM: true as const,
  FORBID_TAGS: ["style", "svg", "math", "picture", "source", "input", "video", "audio"],
  FORBID_ATTR: ["style", "srcset", "poster", "background"],
};

function Markdown({ text }: { text: string }) {
  const html = useMemo(() => {
    const md = marked.parse(text, { async: false, gfm: true });
    const dom = DOMPurify.sanitize(md, AGENT_MD) as HTMLElement;
    // Repo images (rewritten to ghimg:// by the hook in PrView) and inline ones stay.
    for (const img of dom.querySelectorAll("img"))
      if (!/^(ghimg|data):/.test(img.getAttribute("src") ?? "")) img.remove();
    return dom.innerHTML;
  }, [text]);
  return <div className="md" dangerouslySetInnerHTML={{ __html: html }} />;
}

const IMAGE = /\.(svg|png|jpe?g|gif|webp)$/i;

// Encoded per segment, so the page's relative links resolve to the files next to it.
const runFileSrc = (path: string) =>
  "runfile://localhost" + path.split("/").map(encodeURIComponent).join("/");

const fileName = (path: string) => path.split("/").pop();

function RunFile(props: { path: string; inactive: boolean; onOpen: () => void }) {
  const { path, inactive, onOpen } = props;
  const frame = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState<number>();
  useEffect(() => {
    // The runfile protocol adds a script that posts the page's height.
    const onMessage = (e: MessageEvent) => {
      const h = e.data?.ghreviewerHeight;
      if (e.source === frame.current?.contentWindow && typeof h === "number" && h > 0) setHeight(h);
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, []);
  return (
    <div className={`run-file${inactive ? " inactive" : ""}`}>
      <div className="item-head">
        <button className="head-btn" title="Open with its default app" onClick={onOpen}>
          {fileName(path)}
        </button>{" "}
        <span className="dim">{path}</span>
      </div>
      {IMAGE.test(path) ? (
        <img src={convertFileSrc(path)} alt="" loading="lazy" />
      ) : (
        // Agent-written HTML: scripts run (diagrams often need them) but stay in an opaque origin.
        // Keys go to the iframe while it has focus, so give it back on leaving it.
        <iframe
          ref={frame}
          src={runFileSrc(path)}
          style={height ? { height } : undefined}
          loading="lazy"
          sandbox="allow-scripts"
          title={path}
          onMouseLeave={(e) => {
            e.currentTarget.blur();
            window.focus();
          }}
        />
      )}
    </div>
  );
}

const duration = (ms: number) => {
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m${String(s % 60).padStart(2, "0")}s`;
};

function StatusLine({ run, stale, now }: { run: Run; stale: boolean; now: number }) {
  // The app clock ticks every 30s; a running timer needs seconds.
  const [tick, setTick] = useState(Date.now());
  useEffect(() => {
    if (run.status !== "running") return;
    const t = setInterval(() => setTick(Date.now()), 1000);
    return () => clearInterval(t);
  }, [run.status]);
  const took = run.finishedAt ? duration(run.finishedAt - run.startedAt) : null;
  const rel = run.finishedAt ? relTime(run.finishedAt, now) : null;
  const ago = rel && (rel === "now" ? "just now" : `${rel} ago`);
  const state = {
    running: (
      <span className="warn with-spinner">
        <PixelSpinner />
        running · {duration(Math.max(0, tick - run.startedAt))}
      </span>
    ),
    done: <span className="ok">done</span>,
    error: <span className="fail">failed</span>,
    cancelled: <span className="warn">cancelled</span>,
    interrupted: <span className="warn">interrupted (app quit)</span>,
  }[run.status];
  return (
    <div className="run-status">
      {state}
      {ago && <span className="dim"> · {ago}</span>}
      {took && <span className="dim"> · took {took}</span>}
      {stale && <span className="warn"> · stale: new commits since this run</span>}
    </div>
  );
}

const SEVERITY: Record<string, string> = {
  blocker: "fail",
  "should-fix": "warn",
  P0: "fail",
  P1: "warn",
};

export type Picker = {
  picked: boolean[];
  cursor: number;
  posting: boolean;
  onToggle: (i: number) => void;
  /** Unset where the findings are on GitHub already. */
  onPost?: (approve: boolean) => void;
  /** Hands the picked findings to the fix agent; unset without a [fix] config. */
  onFix?: () => void;
  onOpen: (url: string) => void;
};

/** The run's findings, each shown as it will be posted, to pick and post as one GitHub review. */
function ReviewPicker({ review, p }: { review: Review; p: Picker }) {
  const { posted } = review;
  const n = p.picked.filter(Boolean).length;
  const fix = p.onFix && (
    <button className="head-btn" disabled={!n} onClick={p.onFix}>
      fix ({n}) (f)
    </button>
  );
  const post = p.onPost;
  let actions;
  if (posted)
    actions = (
      <>
        <button className="head-btn" onClick={() => p.onOpen(posted)}>
          posted ↗
        </button>
        {review.allInBody && (
          <span className="warn"> · GitHub rejected the inline comments: all are in the body</span>
        )}
        {fix && <> · {fix}</>}
      </>
    );
  else if (p.posting)
    actions = (
      <span className="warn with-spinner">
        <PixelSpinner />
        posting…
      </span>
    );
  else
    actions = (
      <>
        {post && (
          <>
            <button className="head-btn" disabled={!n} onClick={() => post(false)}>
              comment ({n})
            </button>
            {" · "}
            <button className="head-btn" onClick={() => post(true)}>
              approve{n ? ` with ${n}` : ""}
            </button>
            {fix && " · "}
          </>
        )}
        {fix}
        <span className="dim">{(post || fix) && " · "}j/k Space pick</span>
      </>
    );
  return (
    <div className="review">
      <div className="section-head">
        Findings · {n}/{review.findings.length} picked
      </div>
      {review.summary && <Markdown text={review.summary} />}
      {review.findings.map((f, i) => (
        <div
          key={i}
          data-finding={i}
          className={`item finding${i === p.cursor ? " cursor" : ""}${p.picked[i] ? "" : " unpicked"}`}
        >
          <div className="item-head">
            <button
              className="head-btn"
              role="checkbox"
              aria-checked={p.picked[i]}
              disabled={!!posted && !p.onFix}
              onClick={() => p.onToggle(i)}
            >
              [{p.picked[i] ? "x" : " "}]
            </button>{" "}
            <span className={SEVERITY[f.severity.split(" ")[0]] ?? "dim"}>
              {f.severity || "note"}
            </span>{" "}
            <b>{f.title}</b>
            {f.path && (
              <span className="dim">
                {" "}
                {f.path}
                {f.line != null && `:${f.line}`}
              </span>
            )}
          </div>
          <Markdown text={f.body} />
        </div>
      ))}
      <div className="run-status">{actions}</div>
    </div>
  );
}

/** A review bot's open threads on the PR, to pick and hand to the fix agent. */
export function BotView({ review, picker }: { review: BotReview; picker: Picker }) {
  const findings = { summary: "", findings: review.findings, posted: null, allInBody: false };
  return (
    <div className="pr-pane">
      <h1 className="pr-title">
        {review.bot} <span className="dim">open review threads</span>
      </h1>
      <div className="dim run-keys">
        [ / ] tabs{!picker.onFix && " · add [fix] to the config to fix these"}
      </div>
      <ReviewPicker review={findings} p={picker} />
    </div>
  );
}

export function RunView(props: {
  prompt: Prompt;
  run: Run | undefined;
  stale: boolean;
  hasWorktree: boolean;
  now: number;
  picker: Picker;
  /** File tab shown beside the review (window 1920px+). */
  shownFile: string | undefined;
  onFile: (path: string) => void;
  /** Unset for the fix tab: it runs from picked findings. */
  onRun?: () => void;
  onOpenFile: (path: string) => void;
}) {
  const { prompt, run, stale, hasWorktree, now, picker, shownFile, onFile, onRun, onOpenFile } =
    props;
  const running = run?.status === "running";
  const files = run?.files ?? [];
  return (
    <div className="pr-pane">
      <h1 className="pr-title">
        {prompt.name} <span className="dim">{prompt.harness}</span>
      </h1>
      {run ? (
        <StatusLine run={run} stale={stale} now={now} />
      ) : (
        <div className="run-status">
          <button className="head-btn" onClick={onRun}>
            run (r)
          </button>
          {!hasWorktree && <span className="dim"> · creates the worktree first</span>}
        </div>
      )}
      <div className="dim run-keys">
        {run && onRun && "r re-run · "}
        {run?.sessionId && "c continue in terminal · "}
        {running && ": cancel · "}[ / ] tabs
        {files.length > 1 && <span className="run-file-keys"> · {"{ / }"} files</span>}
      </div>
      {!run && prompt.prompt && <pre className="run-prompt">{prompt.prompt}</pre>}
      {run?.error && <pre className="run-error fail">{run.error}</pre>}
      <div className={files.length ? "run-split" : undefined}>
        {files.length > 0 && (
          <div className="run-files">
            <div className="tabs run-file-tabs">
              {files.map((f) => (
                <button
                  key={f}
                  className={`tab ${f === shownFile ? "active" : ""}`}
                  title={f}
                  onClick={() => onFile(f)}
                >
                  {fileName(f)}
                </button>
              ))}
            </div>
            {files.map((f) => (
              <RunFile key={f} path={f} inactive={f !== shownFile} onOpen={() => onOpenFile(f)} />
            ))}
          </div>
        )}
        <div className="run-main">
          {run?.review && <ReviewPicker review={run.review} p={picker} />}
          {run?.output && <Markdown text={run.output} />}
          {run && run.activity.length > 0 && (
            <details className="run-activity" open={running}>
              <summary className="dim">
                {run.activity.length} step{run.activity.length === 1 ? "" : "s"}
                {run.sessionId && ` · session ${run.sessionId}`}
              </summary>
              {(running ? run.activity.slice(-15) : run.activity).map((a, i) => (
                <div key={i} className="dim">
                  {a}
                </div>
              ))}
            </details>
          )}
        </div>
      </div>
    </div>
  );
}
