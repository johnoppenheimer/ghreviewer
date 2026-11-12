import { useQuery } from "@tanstack/react-query";
import DOMPurify from "dompurify";
import {
  type ReactNode,
  type RefObject,
  Suspense,
  createContext,
  lazy,
  useContext,
  useDeferredValue,
  useMemo,
  useState,
} from "react";
import {
  api,
  type Check,
  type PrDetail,
  type PrSummary,
  type Thread,
  type TimelineItem,
  list,
} from "./api";
import { REVIEW_LABEL, checkGlyph, ghImageSrc, recent, relTime, tailPatch } from "./model";

// Global hook, so it also covers the run markdown: off-screen images aren't fetched or decoded,
// and repo images load with the token (the webview has no GitHub session for private repos).
DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node.tagName !== "IMG") return;
  node.setAttribute("loading", "lazy");
  const src = ghImageSrc(node.getAttribute("src") ?? "");
  if (src) node.setAttribute("src", src);
});

// Sanitizing is most of a PR's render time, so going back to a recent PR skips it.
const RECENT_PRS = 20;
const cleanByPr = new Map<string, Map<string, string>>();
const CleanCache = createContext(new Map<string, string>());

function Html({ html }: { html: string | undefined }) {
  const cache = useContext(CleanCache);
  const clean = useMemo(() => {
    const raw = html ?? "";
    let out = cache.get(raw);
    if (out === undefined) cache.set(raw, (out = DOMPurify.sanitize(raw)));
    return out;
  }, [html, cache]);
  return <div className="md" dangerouslySetInnerHTML={{ __html: clean }} />;
}

function stateBadge(pr: PrDetail): [string, string] {
  if (pr.state === "MERGED") return ["merged", "accent"];
  if (pr.state === "CLOSED") return ["closed", "fail"];
  if (pr.isDraft) return ["draft", "dim"];
  return ["open", "ok"];
}

function Reviewers({ pr }: { pr: PrDetail }) {
  const reviewed = list(pr.latestReviews).map((r) => ({
    who: r.author?.login ?? "ghost",
    state: r.state,
  }));
  const pending = list(pr.reviewRequests).map((r) => ({
    who: r.requestedReviewer?.login ?? `@${r.requestedReviewer?.name ?? "?"}`,
    state: "PENDING",
  }));
  const all = [...pending, ...reviewed];
  if (!all.length) return <span className="dim">none</span>;
  return (
    <span>
      {all.map(({ who, state }) => {
        const [label, cls] = REVIEW_LABEL[state] ?? [state.toLowerCase(), "dim"];
        return (
          <span key={who + state} style={{ marginRight: 12 }}>
            {who} <span className={cls}>{label}</span>
          </span>
        );
      })}
    </span>
  );
}

const checkName = (c: Check) => c.name ?? c.context ?? "?";
const checkState = (c: Check) =>
  c.__typename === "CheckRun" ? (c.status === "COMPLETED" ? c.conclusion : "PENDING") : c.state;

function Checks({ pr }: { pr: PrDetail }) {
  const rollup = list(pr.commits)[0]?.commit.statusCheckRollup;
  if (!rollup) return <span className="dim">none</span>;
  const order = ["fail", "warn", "ok", "dim"];
  const severity = (c: Check) => order.indexOf(checkGlyph(checkState(c))[1]);
  const checks = list(rollup.contexts).sort((a, b) => severity(a) - severity(b));
  const tally = new Map<string, number>();
  for (const c of checks) {
    const [glyph] = checkGlyph(checkState(c));
    tally.set(glyph, (tally.get(glyph) ?? 0) + 1);
  }
  const failing = checks.filter((c) => severity(c) === 0).map(checkName);
  const [glyph, cls] = checkGlyph(rollup.state);
  return (
    <details>
      <summary>
        <span className={cls}>{glyph}</span> {rollup.state.toLowerCase()}{" "}
        <span className="dim">({[...tally].map(([g, n]) => `${n}${g}`).join(" ")})</span>
        {failing.length > 0 && <span className="fail"> {failing.join(", ")}</span>}
      </summary>
      {checks.map((c, i) => {
        const [g, gc] = checkGlyph(checkState(c));
        const url = c.detailsUrl ?? c.targetUrl;
        return (
          <div key={i} className="checks">
            <span className={gc}>{g}</span> {url ? <a href={url}>{checkName(c)}</a> : checkName(c)}
          </div>
        );
      })}
    </details>
  );
}

const EVENTS: Record<string, (it: TimelineItem) => string> = {
  ReviewRequestedEvent: (it) =>
    `requested review from ${it.requestedReviewer?.login ?? `@${it.requestedReviewer?.name ?? "?"}`}`,
  ReadyForReviewEvent: () => "marked ready for review",
  ConvertToDraftEvent: () => "converted to draft",
  HeadRefForcePushedEvent: () => "force-pushed",
  MergedEvent: () => "merged",
  ClosedEvent: () => "closed",
  ReopenedEvent: () => "reopened",
  LabeledEvent: (it) => `added label ${it.label?.name}`,
};

const REVIEW_VERB: Record<string, string> = {
  APPROVED: "approved",
  CHANGES_REQUESTED: "requested changes",
  COMMENTED: "reviewed",
  DISMISSED: "review dismissed",
  PENDING: "pending review",
};

function Entry({ it, now }: { it: TimelineItem; now: number }) {
  const who = it.author?.login ?? it.actor?.login ?? "ghost";
  const when = it.createdAt ? relTime(it.createdAt, now) : "";
  if (it.__typename === "PullRequestCommit" && it.commit) {
    const c = it.commit;
    return (
      <div className="event">
        ● <a href={c.url}>{c.abbreviatedOid}</a>{" "}
        <span style={{ color: "var(--fg)" }}>{c.messageHeadline}</span> ·{" "}
        {c.author?.user?.login ?? c.author?.name} · {relTime(c.committedDate, now)}
      </div>
    );
  }
  if (it.__typename === "IssueComment" || (it.__typename === "PullRequestReview" && it.bodyHTML)) {
    const review = it.__typename === "PullRequestReview";
    const [, cls] = REVIEW_LABEL[it.state ?? ""] ?? ["", "dim"];
    return (
      <div className="item">
        <div className="item-head">
          <b>{who}</b>{" "}
          {review ? (
            <span className={cls}>{REVIEW_VERB[it.state ?? ""] ?? "reviewed"}</span>
          ) : (
            "commented"
          )}{" "}
          · <a href={it.url}>{when}</a>
        </div>
        <Html html={it.bodyHTML} />
      </div>
    );
  }
  if (it.__typename === "PullRequestReview") {
    const [, cls] = REVIEW_LABEL[it.state ?? ""] ?? ["", "dim"];
    const n = it.comments?.totalCount ?? 0;
    return (
      <div className="event">
        <b>{who}</b> <span className={cls}>{REVIEW_VERB[it.state ?? ""] ?? "reviewed"}</span>
        {n > 0 && ` with ${n} inline comment${n > 1 ? "s" : ""}`} · {when}
      </div>
    );
  }
  const text = EVENTS[it.__typename]?.(it) ?? it.__typename;
  return (
    <div className="event">
      <b>{who}</b> {text} · {when}
    </div>
  );
}

// Diff renderer + Shiki core is most of the bundle; load it on the first inline comment.
const PatchDiff = lazy(() => import("@pierre/diffs/react").then((m) => ({ default: m.PatchDiff })));

function Hunk({ path, text }: { path: string; text: string }) {
  const theme = useQuery({ queryKey: ["config"], queryFn: api.config }).data?.theme;
  const patch = useMemo(() => tailPatch(path, text, 8), [path, text]);
  return (
    <Suspense fallback={null}>
      <PatchDiff
        className="hunk"
        patch={patch}
        options={{
          diffStyle: "unified",
          disableFileHeader: true,
          hunkSeparators: "simple",
          // shortcut: custom theme names get dark syntax colors; add a theme key if one needs light.
          themeType: theme === "light" ? "light" : "dark",
          unsafeCSS: "* { border-radius: 0 !important; }",
        }}
      />
    </Suspense>
  );
}

function ThreadView({ t, now }: { t: Thread; now: number }) {
  const comments = list(t.comments);
  const line = t.line ?? t.originalLine;
  // Highlighting a hunk blocks the paint, so collapsed threads only get one when opened.
  const [open, setOpen] = useState(!t.isResolved && !t.isOutdated);
  return (
    <details className="item" open={open} onToggle={(e) => setOpen(e.currentTarget.open)}>
      <summary>
        <b>
          {t.path}
          {line ? `:${line}` : ""}
        </b>
        {t.isResolved && <span className="ok"> resolved</span>}
        {t.isOutdated && <span className="dim"> outdated</span>}
        <span className="dim">
          {" "}
          · {comments.length} comment{comments.length === 1 ? "" : "s"}
        </span>
      </summary>
      {open && comments[0] && <Hunk path={t.path} text={comments[0].diffHunk} />}
      {comments.map((c) => (
        <div key={c.id} className="item">
          <div className="item-head">
            <b>{c.author?.login ?? "ghost"}</b> · <a href={c.url}>{relTime(c.createdAt, now)}</a>
          </div>
          <Html html={c.bodyHTML} />
        </div>
      ))}
    </details>
  );
}

function Detail({ pr, now, approve }: { pr: PrDetail; now: number; approve: ReactNode }) {
  const [badge, badgeCls] = stateBadge(pr);
  const labels = list(pr.labels);
  const assignees = list(pr.assignees);
  const timeline = list(pr.timelineItems);
  const threads = list(pr.reviewThreads);
  const open = threads.filter((t) => !t.isResolved).length;
  return (
    <div className="pr-pane">
      <h1 className="pr-title">
        {pr.title} <span className="dim">#{pr.number}</span>
      </h1>
      <div>
        <span className={`badge ${badgeCls}`}>{badge}</span>
        <b>{pr.author?.login ?? "ghost"}</b> <span className="dim">wants to merge</span>{" "}
        {pr.headRefName} <span className="dim">→</span> {pr.baseRefName}{" "}
        <span className="dim">·</span> <span className="ok">+{pr.additions}</span>{" "}
        <span className="fail">-{pr.deletions}</span>{" "}
        <span className="dim">· {pr.changedFiles} files ·</span>{" "}
        <a href={pr.url}>{pr.repository.nameWithOwner}</a>
      </div>
      <div className="meta">
        <span className="k">reviewers</span>
        <span>
          <Reviewers pr={pr} /> {approve}
        </span>
        {assignees.length > 0 && (
          <>
            <span className="k">assignees</span>
            <span>{assignees.map((a) => a.login).join(" ")}</span>
          </>
        )}
        {labels.length > 0 && (
          <>
            <span className="k">labels</span>
            <span>
              {labels.map((l) => (
                <span key={l.name} className="label" style={{ borderLeftColor: `#${l.color}` }}>
                  {l.name}
                </span>
              ))}
            </span>
          </>
        )}
        <span className="k">checks</span>
        <Checks pr={pr} />
        <span className="k">merge</span>
        <span
          className={
            pr.mergeable === "CONFLICTING" ? "fail" : pr.mergeStateStatus === "CLEAN" ? "ok" : ""
          }
        >
          {pr.mergeable.toLowerCase()} · {pr.mergeStateStatus.toLowerCase()}
        </span>
        <span className="k">opened</span>
        <span>{relTime(pr.createdAt, now)} ago</span>
      </div>

      <div className="pr-split">
        <div className="section">
          <div className="section-head">Description</div>
          {pr.bodyHTML ? <Html html={pr.bodyHTML} /> : <div className="dim">No description.</div>}
        </div>

        <div>
          <div className="section">
            <div className="section-head">Conversation · {timeline.length}</div>
            {timeline.map((it) => (
              <Entry key={it.id} it={it} now={now} />
            ))}
          </div>

          {threads.length > 0 && (
            <div className="section">
              <div className="section-head">
                Review threads · {open} open / {threads.length}
              </div>
              {threads.map((t) => (
                <ThreadView key={t.id} t={t} now={now} />
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export function PrView(props: {
  summary: PrSummary | null;
  detail: PrDetail | undefined;
  error: string | null;
  focused: boolean;
  now: number;
  paneRef: RefObject<HTMLDivElement | null>;
  /** Right side of the header (worktree folder / create button). */
  headRight: ReactNode;
  /** Header highlight: "busy" while creating a worktree, "done" right after. */
  headState: "" | "busy" | "done";
  /** Approve button after the reviewers (PR tab). */
  approve: ReactNode;
  /** Tab labels (the PR, then prompts); no bar when there's only the PR. */
  tabs: ReactNode[];
  /** Index of the first prompt tab; prompt tabs sit on the right. */
  firstPromptTab: number;
  tab: number;
  /** Content of a prompt tab, shown instead of the PR. */
  tabBody: ReactNode;
  onTab: (i: number) => void;
  onFocus: () => void;
  onLink: (url: string) => void;
}) {
  const { summary, detail, error, focused, now, paneRef, headRight, headState, approve } = props;
  const { tabs, firstPromptTab, tab, tabBody, onTab, onFocus, onLink } = props;
  // A PR renders in the background, so holding j/k drops the PRs it passes and only shows titles.
  const shown = useDeferredValue(detail);
  const body =
    detail &&
    (tabBody ??
      (shown?.id === detail.id && (
        <CleanCache value={recent(cleanByPr, shown.id, RECENT_PRS, () => new Map())}>
          <Detail key={shown.id} pr={shown} now={now} approve={approve} />
        </CleanCache>
      )));
  return (
    <div className={`col ${focused ? "focused" : ""}`} onMouseDown={onFocus}>
      <div className={`col-head ${headState}`} data-tauri-drag-region>
        <span className="grow">{summary ? `${summary.repo}#${summary.number}` : "PR"}</span>
        {summary && !detail && !error ? <span>loading…</span> : headRight}
      </div>
      {summary && tabs.length > 1 && (
        <div className="tabs">
          {tabs.map((label, i) => (
            <button
              key={i}
              className={`tab ${i >= firstPromptTab ? "prompt" : ""} ${i === tab ? "active" : ""}`}
              onClick={() => onTab(i)}
            >
              {label}
            </button>
          ))}
        </div>
      )}
      <div
        className="scroll"
        ref={paneRef}
        onClick={(e) => {
          const a = (e.target as HTMLElement).closest("a");
          if (!a) return;
          // Never navigate the app webview itself.
          e.preventDefault();
          if (a.href.startsWith("http")) onLink(a.href);
        }}
      >
        {error && <div className="empty error">{error}</div>}
        {!summary && <div className="empty">No PR selected.</div>}
        {summary && !body && (
          <div className="pr-pane">
            <h1 className="pr-title">
              {summary.title} <span className="dim">#{summary.number}</span>
            </h1>
            <span className="dim">{summary.author}</span>
          </div>
        )}
        {body}
      </div>
    </div>
  );
}
