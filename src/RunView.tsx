import { convertFileSrc } from "@tauri-apps/api/core";
import DOMPurify from "dompurify";
import { marked } from "marked";
import { useEffect, useMemo, useState } from "react";
import type { Prompt, Run } from "./api";
import { relTime } from "./model";

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

function Markdown({ text }: { text: string }) {
  const html = useMemo(
    () => DOMPurify.sanitize(marked.parse(text, { async: false, gfm: true })),
    [text],
  );
  return <div className="md" dangerouslySetInnerHTML={{ __html: html }} />;
}

const IMAGE = /\.(svg|png|jpe?g|gif|webp)$/i;

function RunFile({ path, onOpen }: { path: string; onOpen: () => void }) {
  const src = convertFileSrc(path);
  return (
    <div className="run-file">
      <div className="item-head">
        <button className="head-btn" title="Open with its default app" onClick={onOpen}>
          {path.split("/").pop()}
        </button>{" "}
        <span className="dim">{path}</span>
      </div>
      {IMAGE.test(path) ? (
        <img src={src} alt="" />
      ) : (
        // Agent-written HTML: scripts run (diagrams often need them) but stay in an opaque origin.
        <iframe src={src} sandbox="allow-scripts" title={path} />
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

export function RunView(props: {
  prompt: Prompt;
  run: Run | undefined;
  stale: boolean;
  hasWorktree: boolean;
  now: number;
  onRun: () => void;
  onOpenFile: (path: string) => void;
}) {
  const { prompt, run, stale, hasWorktree, now, onRun, onOpenFile } = props;
  const running = run?.status === "running";
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
        {run && "r re-run · "}
        {run?.sessionId && "c continue in terminal · "}
        {running && ": cancel · "}[ / ] tabs
      </div>
      {!run && <pre className="run-prompt">{prompt.prompt}</pre>}
      {run?.error && <pre className="run-error fail">{run.error}</pre>}
      {run?.files.map((f) => (
        <RunFile key={f} path={f} onOpen={() => onOpenFile(f)} />
      ))}
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
  );
}
