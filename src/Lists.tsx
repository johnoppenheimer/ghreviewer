import { useEffect, useRef } from "react";
import type { PrSummary } from "./api";
import { REVIEW_LABEL, type SideItem, checkGlyph, relTime } from "./model";

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
  return (
    <div className={`col ${focused ? "focused" : ""}`}>
      <div className="col-head">
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
    </div>
  );
}

export function Inbox(
  props: FilterProps & {
    title: string;
    prs: PrSummary[];
    selected: string | null;
    focused: boolean;
    empty: string;
    now: number;
    onSelect: (id: string) => void;
  },
) {
  const { title, prs, selected, focused, empty, now, onSelect } = props;
  const scrollRef = useScrollToSelected(selected);
  return (
    <div className={`col ${focused ? "focused" : ""}`}>
      <div className="col-head">
        <span className="grow">{title}</span>
        <span>{prs.length}</span>
      </div>
      <FilterInput {...props} />
      <div className="scroll" ref={scrollRef}>
        {prs.length === 0 && <div className="empty">{empty}</div>}
        {prs.map((pr) => {
          const sel = pr.id === selected;
          const [glyph, glyphCls] = checkGlyph(pr.checks);
          const review = pr.reviewDecision ? REVIEW_LABEL[pr.reviewDecision] : null;
          return (
            <div
              key={pr.id}
              className={`row pr-row ${sel ? "selected" : ""} ${pr.unread ? "unread" : ""}`}
              onClick={() => onSelect(pr.id)}
            >
              <div className="line">
                <span className="dot">{pr.unread ? "●" : ""}</span>
                <span className="grow dim">
                  {pr.repo}#{pr.number}
                </span>
                <span className="dim">{relTime(pr.updatedAt, now)}</span>
              </div>
              <div className="line">
                <span className="dot" />
                <span className="title grow">{pr.title}</span>
              </div>
              <div className="line">
                <span className="dot" />
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
                {review && <span className={review[1]}>{review[0]}</span>}
                {glyph && <span className={glyphCls}>{glyph}</span>}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
