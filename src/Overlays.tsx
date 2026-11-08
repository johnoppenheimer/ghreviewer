import { type ReactNode, useState } from "react";
import { type Config, type Rule, api } from "./api";
import { HELP } from "./keys";
import { step } from "./model";

function Overlay({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
}) {
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="panel">
        <div className="panel-head">{title}</div>
        {children}
      </div>
    </div>
  );
}

export function Help({ onClose }: { onClose: () => void }) {
  return (
    <Overlay title="Keys" onClose={onClose}>
      <div className="panel-body help-grid">
        {HELP.map(([k, v]) => (
          <div key={k} style={{ display: "contents" }}>
            <span className="accent">{k}</span>
            <span>{v}</span>
          </div>
        ))}
      </div>
    </Overlay>
  );
}

export type Command = { label: string; run: () => void };

export function Palette({ commands, onClose }: { commands: Command[]; onClose: () => void }) {
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const shown = commands.filter((c) => c.label.toLowerCase().includes(query.toLowerCase()));
  const sel = Math.min(index, shown.length - 1);
  const run = (c: Command | undefined) => {
    if (!c) return;
    onClose();
    c.run();
  };
  return (
    <Overlay title="Commands" onClose={onClose}>
      <div className="palette">
        <input
          autoFocus
          value={query}
          placeholder=":"
          onChange={(e) => {
            setQuery(e.target.value);
            setIndex(0);
          }}
          onKeyDown={(e) => {
            const down = e.key === "ArrowDown" || (e.ctrlKey && e.key === "n");
            const up = e.key === "ArrowUp" || (e.ctrlKey && e.key === "p");
            if (e.key === "Escape") onClose();
            else if (e.key === "Enter") run(shown[sel]);
            else if (down || up) {
              e.preventDefault();
              setIndex(step(shown.length, sel, down ? 1 : -1));
            }
          }}
        />
        {shown.map((c, i) => (
          <div
            key={c.label}
            className={`row ${i === sel ? "selected" : ""}`}
            onClick={() => run(c)}
          >
            {c.label}
          </div>
        ))}
        {shown.length === 0 && <div className="empty">No command.</div>}
      </div>
    </Overlay>
  );
}

const lines = (text: string) =>
  text
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter(Boolean);

export function Settings({ config, onClose }: { config: Config; onClose: () => void }) {
  const [poll, setPoll] = useState(String(config.poll_interval_secs));
  const [theme, setTheme] = useState(config.theme);
  const [rules, setRules] = useState<Rule[]>(config.rules);
  const [authors, setAuthors] = useState(config.filters.exclude_authors.join("\n"));
  const [repos, setRepos] = useState(config.filters.exclude_repos.join("\n"));
  const [hideDrafts, setHideDrafts] = useState(config.filters.hide_drafts);
  const [error, setError] = useState<string | null>(null);
  const themes = [...new Set([...Object.keys(config.themes), config.theme])];

  const setRule = (i: number, patch: Partial<Rule>) =>
    setRules(rules.map((r, j) => (j === i ? { ...r, ...patch } : r)));

  const save = async () => {
    const cleaned = rules.map((r) => ({ name: r.name.trim(), query: r.query.trim() }));
    const seconds = Number(poll);
    if (cleaned.some((r) => !r.name || !r.query))
      return setError("Every rule needs a name and a query.");
    if (new Set(cleaned.map((r) => r.name)).size !== cleaned.length)
      return setError("Rule names must be unique.");
    if (!Number.isInteger(seconds) || seconds < 30)
      return setError("Poll interval must be at least 30 seconds.");
    try {
      await api.saveSettings({
        poll_interval_secs: seconds,
        theme,
        rules: cleaned,
        filters: {
          exclude_authors: lines(authors),
          exclude_repos: lines(repos),
          hide_drafts: hideDrafts,
        },
      });
      onClose();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <Overlay title="Settings" onClose={onClose}>
      <div
        className="panel-body"
        onKeyDown={(e) => {
          if (e.key === "Escape") onClose();
          if (e.key === "Enter" && e.metaKey) save();
        }}
      >
        <div className="form">
          <span className="dim">rules</span>
          <div>
            {rules.map((r, i) => (
              <div key={i} className="rule">
                <input
                  autoFocus={i === 0}
                  value={r.name}
                  placeholder="name"
                  onChange={(e) => setRule(i, { name: e.target.value })}
                />
                <input
                  value={r.query}
                  placeholder="is:open review-requested:@me"
                  onChange={(e) => setRule(i, { query: e.target.value })}
                />
                <button onClick={() => setRules(rules.filter((_, j) => j !== i))}>x</button>
              </div>
            ))}
            <button onClick={() => setRules([...rules, { name: "", query: "" }])}>+ rule</button>
          </div>

          <span className="dim">exclude authors</span>
          <textarea
            value={authors}
            onChange={(e) => setAuthors(e.target.value)}
            placeholder="*[bot]"
          />

          <span className="dim">exclude repos</span>
          <textarea
            value={repos}
            onChange={(e) => setRepos(e.target.value)}
            placeholder="acme/legacy, other/*"
          />

          <span className="dim">hide drafts</span>
          <input
            type="checkbox"
            style={{ justifySelf: "start" }}
            checked={hideDrafts}
            onChange={(e) => setHideDrafts(e.target.checked)}
          />

          <span className="dim">poll (secs)</span>
          <input value={poll} onChange={(e) => setPoll(e.target.value)} style={{ width: "10ch" }} />

          <span className="dim">theme</span>
          <select
            value={theme}
            onChange={(e) => setTheme(e.target.value)}
            style={{ justifySelf: "start" }}
          >
            {themes.map((t) => (
              <option key={t}>{t}</option>
            ))}
          </select>
        </div>
        {error && <div className="error">{error}</div>}
        <div className="actions">
          <button onClick={() => api.openConfig().catch((e) => setError(String(e)))}>
            open config file
          </button>
          <span className="grow" />
          <button onClick={onClose}>cancel (esc)</button>
          <button onClick={save}>save (⌘↵)</button>
        </div>
      </div>
    </Overlay>
  );
}
