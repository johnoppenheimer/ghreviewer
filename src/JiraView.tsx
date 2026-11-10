import DOMPurify from "dompurify";
import { useMemo } from "react";
import type { Ticket } from "./api";
import { jiraClass, relTime } from "./model";

/** Jira's rendered HTML: links made absolute, and images (which need a Jira session) as links. */
function JiraHtml({ html, base }: { html: string; base: string }) {
  const clean = useMemo(() => {
    const dom = DOMPurify.sanitize(html, {
      RETURN_DOM: true as const,
      FORBID_ATTR: ["style"],
    }) as HTMLElement;
    const abs = (url: string | null) => {
      try {
        return new URL(url ?? "", base).href;
      } catch {
        return base;
      }
    };
    for (const a of dom.querySelectorAll("a[href]"))
      a.setAttribute("href", abs(a.getAttribute("href")));
    for (const img of dom.querySelectorAll("img")) {
      const a = document.createElement("a");
      a.href = abs(img.getAttribute("src"));
      a.textContent = `[image${img.alt ? `: ${img.alt}` : ""}]`;
      img.replaceWith(a);
    }
    return dom.innerHTML;
  }, [html, base]);
  return <div className="md" dangerouslySetInnerHTML={{ __html: clean }} />;
}

/** The Jira tab: one ticket named in the PR title, with a tab per ticket when there are several. */
export function JiraView(props: {
  tickets: Ticket[];
  ticket: Ticket;
  now: number;
  onTicket: (key: string) => void;
}) {
  const { tickets, ticket: t, now, onTicket } = props;
  const meta: [string, string | null][] = [
    ["assignee", t.assignee ?? "unassigned"],
    ["reporter", t.reporter],
    ["priority", t.priority],
  ];
  return (
    <div className="pr-pane">
      {tickets.length > 1 && (
        <div className="tabs jira-tickets" title="{ / }: previous / next ticket">
          {tickets.map((o) => (
            <button
              key={o.key}
              className={`tab ${o.key === t.key ? "active" : ""}`}
              onClick={() => onTicket(o.key)}
            >
              {o.key}
            </button>
          ))}
        </div>
      )}
      <h1 className="pr-title">
        {t.summary} <span className="dim">{t.key}</span>
      </h1>
      <div>
        <span className={`badge ${jiraClass(t.category)}`}>{t.status.toLowerCase()}</span>
        {t.kind} <span className="dim">·</span> <a href={t.url}>{t.key}</a>
      </div>
      <div className="meta">
        {meta.map(
          ([k, v]) =>
            v && (
              <span key={k} style={{ display: "contents" }}>
                <span className="k">{k}</span>
                <span>{v}</span>
              </span>
            ),
        )}
        {t.parent && (
          <>
            <span className="k">parent</span>
            <span>
              <a href={t.parent.url}>{t.parent.key}</a> {t.parent.summary}{" "}
              <span className="dim">{t.parent.kind}</span>
            </span>
          </>
        )}
        {t.labels.length > 0 && (
          <>
            <span className="k">labels</span>
            <span>
              {t.labels.map((l) => (
                <span key={l} className="label">
                  {l}
                </span>
              ))}
            </span>
          </>
        )}
      </div>

      <div className="pr-split">
        <div className="section">
          <div className="section-head">Description</div>
          {t.descriptionHtml ? (
            <JiraHtml html={t.descriptionHtml} base={t.url} />
          ) : (
            <div className="dim">No description.</div>
          )}
        </div>
        <div className="section">
          <div className="section-head">Comments · {t.comments.length}</div>
          {t.comments.map((c, i) => (
            <div key={i} className="item">
              <div className="item-head">
                <b>{c.author}</b> · {relTime(c.created, now)}
              </div>
              <JiraHtml html={c.bodyHtml} base={t.url} />
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
