import { type CSSProperties, Fragment, useEffect, useRef } from "react";
import type { Run } from "./api";
import {
  type Graph,
  NEXT_CLASS,
  type PrRow,
  REVIEW_LABEL,
  type SideItem,
  canWait,
  checkGlyph,
  jiraClass,
  relTime,
  stackLeads,
  titleParts,
} from "./model";
import { PixelSpinner } from "./RunView";

type FilterProps = {
  filter: string;
  filterOpen: boolean;
  onFilter: (value: string) => void;
  onFilterDone: () => void;
};

function FilterInput({ filter, filterOpen, onFilter, onFilterDone }: FilterProps) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (filterOpen) ref.current?.focus();
  }, [filterOpen]);
  if (!filterOpen && !filter) return null;
  const done = () => {
    ref.current?.blur();
    onFilterDone();
  };
  return (
    <input
      ref={ref}
      className="col-filter"
      placeholder="filter…"
      value={filter}
      onChange={(e) => onFilter(e.target.value)}
      onBlur={onFilterDone}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          onFilter("");
          done();
        } else if (e.key === "Enter") done();
      }}
    />
  );
}

/** Scrolls the selected row into view when the selection changes, not on every re-render. */
function useScrollToSelected(selected: string | null) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.querySelector(".selected")?.scrollIntoView({ block: "nearest" });
  }, [selected]);
  return ref;
}

function Count({ total, unread }: { total: number; unread: number }) {
  if (!total) return null;
  return (
    <span className={`count ${unread ? "has-unread" : ""}`}>
      {unread ? `${unread}/${total}` : total}
    </span>
  );
}

function RunMark({ status }: { status: Run["status"] }) {
  if (status === "running") return <PixelSpinner />;
  if (status === "done") return <span className="ok">✓</span>;
  if (status === "error") return <span className="fail">✗</span>;
  return <span className="warn">○</span>;
}

export function Sidebar(
  props: FilterProps & {
    items: SideItem[];
    selected: string;
    focused: boolean;
    showHidden: boolean;
    onlyWithPrs: boolean;
    onSelect: (key: string) => void;
  },
) {
  const { items, selected, focused, showHidden, onlyWithPrs, onSelect } = props;
  const scrollRef = useScrollToSelected(selected);
  const runsRef = useScrollToSelected(selected);
  // Agent runs (going or unread) stay pinned under the scrolling repo list.
  const runs = items.flatMap((it) => (it.kind === "run" ? [it] : []));
  return (
    <div className={`col ${focused ? "focused" : ""}`}>
      <div className="col-head" data-tauri-drag-region>
        <span>Repos</span>
        <span>
          {onlyWithPrs && "with PRs "}
          {showHidden && "+hidden"}
        </span>
      </div>
      <FilterInput {...props} />
      <div className="scroll" ref={scrollRef}>
        {items.map((it) => {
          const sel = it.key === selected;
          if (it.kind === "run") return null;
          let cls = "side-all";
          let label = "All";
          if (it.kind === "org") {
            cls = "side-org";
            label = `${it.collapsed ? "▸" : "▾"} ${it.org}`;
          } else if (it.kind === "repo") {
            cls = `side-repo ${it.total ? "has-prs" : "no-prs"}`;
            label = `${it.name}${it.pinned ? " ★" : ""}${it.hidden ? " (hidden)" : ""}`;
          }
          return (
            <div
              key={it.key}
              className={`row ${cls} ${sel ? "selected" : ""}`}
              onClick={() => onSelect(it.key)}
            >
              <span className="grow">{label}</span>
              <Count total={it.total} unread={it.unread} />
            </div>
          );
        })}
      </div>
      {runs.length > 0 && (
        <div className="side-runs" ref={runsRef}>
          {runs.map(({ key, run }) => (
            <div
              key={key}
              className={`row side-run ${key === selected ? "selected" : ""}`}
              title={`${run.repo}#${run.number} · ${run.prompt} · ${run.status}`}
              onClick={() => onSelect(key)}
            >
              <span className="grow">
                {run.repo.split("/")[1]}#{run.number} <span className="tag">{run.prompt}</span>
              </span>
              <RunMark status={run.status} />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** Tree lines at the left of a stacked PR row; see `Graph`. */
function StackLines({ g }: { g: Graph }) {
  const at = (level: number) => ({ "--l": level }) as CSSProperties;
  return (
    <div className="stack-lines" aria-hidden>
      {g.through.map((l) => (
        <i key={l} className="through" style={at(l)} />
      ))}
      {g.depth > 0 && <i className="elbow" style={at(g.depth - 1)} />}
      {g.depth > 0 && !g.last && <i className="sibling" style={at(g.depth - 1)} />}
      {g.down && <i className={`down ${g.depth > 0 ? "joined" : ""}`} style={at(g.depth)} />}
    </div>
  );
}

export function Inbox(
  props: FilterProps & {
    title: string;
    rows: PrRow[];
    /** Smart inbox: rows come ranked (`byNext`); show each state and where "can wait" starts. */
    smart: boolean;
    selected: string | null;
    focused: boolean;
    empty: string;
    now: number;
    rules: string[];
    rule: string;
    onRule: (rule: string) => void;
    onSelect: (id: string) => void;
  },
) {
  const { title, rows, smart, selected, focused, empty, now, rules, rule } = props;
  const { onRule, onSelect } = props;
  const scrollRef = useScrollToSelected(selected);
  const leads = smart ? stackLeads(rows) : [];
  const waitAt = leads.findIndex((lead) => lead && canWait(lead));
  const waiting = waitAt < 0 ? 0 : rows.slice(waitAt).filter((r) => !r.dim).length;
  return (
    <div className={`col ${focused ? "focused" : ""}`}>
      <div className="col-head" data-tauri-drag-region>
        <span className="grow">{title}</span>
        <span>{rows.filter((r) => !r.dim).length}</span>
      </div>
      {rules.length > 1 && (
        <div className="tabs" title="t: next rule">
          {["", ...rules].map((r) => (
            <button
              key={r}
              className={`tab ${r === rule ? "active" : ""}`}
              onClick={() => onRule(r)}
            >
              {r || "all"}
            </button>
          ))}
        </div>
      )}
      <FilterInput {...props} />
      <div className="scroll" ref={scrollRef}>
        {rows.length === 0 && <div className="empty">{empty}</div>}
        {rows.map(({ pr, dim, graph }, i) => {
          const sel = pr.id === selected;
          const unread = pr.unread && !dim;
          const later = waitAt >= 0 && i >= waitAt;
          // Why this stack ranks here, when it's a PR further down it.
          const lead = leads[i] !== pr ? leads[i] : undefined;
          const [glyph, glyphCls] = checkGlyph(pr.checks);
          const review = pr.reviewDecision ? REVIEW_LABEL[pr.reviewDecision] : null;
          return (
            <Fragment key={pr.id}>
              {i === waitAt && <div className="rank-line">can wait · {waiting}</div>}
              <div
                className={`row pr-row ${sel ? "selected" : ""} ${unread ? "unread" : ""} ${dim ? "stack-parent" : ""} ${later ? "can-wait" : ""}`}
                style={{ "--depth": graph?.depth ?? 0 } as CSSProperties}
                title={dim ? "Stack parent, not in your inbox" : undefined}
                onClick={() => onSelect(pr.id)}
              >
                {graph && <StackLines g={graph} />}
                <div className="line">
                  <span className="dot">{unread ? "●" : ""}</span>
                  <span className="grow dim">
                    {pr.repo}#{pr.number}
                  </span>
                  {lead?.next && (
                    <span
                      className={NEXT_CLASS[lead.next]}
                      title={`Ranked here for #${lead.number}, further down this stack`}
                    >
                      {lead.next} ↓#{lead.number}
                    </span>
                  )}
                  <span className="dim">{relTime(pr.updatedAt, now)}</span>
                </div>
                <div className="line">
                  <span className="dot" />
                  <span className="title grow">
                    {titleParts(pr.title, pr.jira).map(({ text, ticket }, i) =>
                      ticket ? (
                        <span key={i} className={jiraClass(ticket.category)}>
                          {text}
                        </span>
                      ) : (
                        text
                      ),
                    )}
                  </span>
                </div>
                <div className="line">
                  <span className="dot" />
                  {smart && (
                    <span className={`state ${pr.next ? NEXT_CLASS[pr.next] : ""}`}>
                      {!dim && pr.next}
                    </span>
                  )}
                  <span className="grow">
                    <span className="dim">{pr.author}</span>
                    {pr.rules.map((r) => (
                      <span key={r} className="tag">
                        {" "}
                        [{r}]
                      </span>
                    ))}
                    {pr.isDraft && <span className="dim"> draft</span>}
                  </span>
                  {/* `ready` already says approved. */}
                  {review && !(smart && pr.next === "ready") && (
                    <span className={review[1]}>{review[0]}</span>
                  )}
                  {glyph && <span className={glyphCls}>{glyph}</span>}
                </div>
              </div>
            </Fragment>
          );
        })}
      </div>
    </div>
  );
}
