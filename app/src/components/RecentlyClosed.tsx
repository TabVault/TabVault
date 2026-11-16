// Recently Closed: searchable, filterable, virtualized list of every tab
// ever seen closing. The home screen of TabVault.

import { useCallback, useEffect, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { api, type ClosedTabRow, type RestoreMode } from '../api';
import { Favicon, fmtAge, fmtMs, sourceLabel, groupColor } from './Shared';

type Range = 'all' | '24h' | '7d' | '30d';
type SourceFilter = '' | 'extension' | 'chromium_snss' | 'firefox_sessionstore' | 'chromium_history' | 'firefox_places' | 'import';

const RANGE_LABEL: Record<Range, string> = {
  all: 'All time',
  '24h': 'Last 24 h',
  '7d': 'Last 7 days',
  '30d': 'Last 30 days',
};

const SOURCE_OPTIONS: { value: SourceFilter; label: string }[] = [
  { value: '', label: 'All sources' },
  { value: 'extension', label: 'Live capture' },
  { value: 'chromium_snss', label: 'Chrome/Edge files' },
  { value: 'firefox_sessionstore', label: 'Firefox files' },
  { value: 'chromium_history', label: 'Chrome/Edge history' },
  { value: 'firefox_places', label: 'Firefox history' },
  { value: 'import', label: 'Imported' },
];

function rangeToFrom(range: Range): number | undefined {
  const now = Date.now();
  switch (range) {
    case '24h': return now - 86_400_000;
    case '7d': return now - 7 * 86_400_000;
    case '30d': return now - 30 * 86_400_000;
    default: return undefined;
  }
}

export default function RecentlyClosed({ notify }: { notify: (msg: string) => void }) {
  const [query, setQuery] = useState('');
  const [pinnedOnly, setPinnedOnly] = useState(false);
  const [browser, setBrowser] = useState<string | undefined>(undefined);
  const [source, setSource] = useState<SourceFilter>('');
  const [range, setRange] = useState<Range>('all');
  const [rows, setRows] = useState<ClosedTabRow[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [skipOpen, setSkipOpen] = useState(false);
  const parentRef = useRef<HTMLDivElement>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const page = await api.recentlyClosed({
        q: query.trim() || undefined,
        pinnedOnly,
        browser,
        source: source || undefined,
        dateFrom: rangeToFrom(range),
        limit: 500,
        offset: 0,
      });
      setRows(page.rows);
      setTotal(page.total);
      setSelected((prev) => {
        const next = new Set<number>();
        for (const row of page.rows) if (prev.has(row.id)) next.add(row.id);
        return next;
      });
    } finally {
      setLoading(false);
    }
  }, [query, pinnedOnly, browser, source, range]);

  useEffect(() => {
    const t = setTimeout(() => void load(), 180);
    return () => clearTimeout(t);
  }, [load]); // load depends on [query, pinnedOnly, browser, source, range]

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 54,
    overscan: 12,
  });

  const toggleAll = () => {
    if (selected.size === rows.length) {
      setSelected(new Set());
    } else {
      setSelected(new Set(rows.map((r) => r.id)));
    }
  };

  const toggle = (id: number) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const restoreMode = (newWindow: boolean): RestoreMode => ({ newWindow, skipOpen });

  const restoreSelected = async (newWindow: boolean) => {
    if (selected.size === 0) return;
    try {
      await api.restoreTabs([...selected], restoreMode(newWindow));
      notify(`Sent ${selected.size} tab${selected.size === 1 ? '' : 's'} to the browser — it will open them in a moment.`);
    } catch (e) {
      notify(`Restore failed: ${String(e)}`);
    }
  };

  const restoreOne = async (row: ClosedTabRow, newWindow: boolean) => {
    try {
      await api.restoreTabs([row.id], restoreMode(newWindow));
      notify('Restoring 1 tab…');
    } catch (e) {
      notify(`Restore failed: ${String(e)}`);
    }
  };

  const restoreWindowOf = async (row: ClosedTabRow) => {
    if (row.windowEventId == null) return;
    try {
      await api.restoreWindow(row.windowEventId, restoreMode(true));
      notify('Restoring the whole window…');
    } catch (e) {
      notify(`Restore failed: ${String(e)}`);
    }
  };

  const items = virtualizer.getVirtualItems();

  return (
    <div className="screen">
      <div className="screen-header" style={{ justifyContent: 'space-between' }}>
        <div>
          <h1>Recently Closed</h1>
          <div className="screen-sub">{total.toLocaleString()} closed tab{total === 1 ? '' : 's'} recorded{loading ? ' · loading…' : ''}</div>
        </div>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <label className="chip" title="Skip URLs that are already open">
            <input type="checkbox" checked={skipOpen} onChange={(e) => setSkipOpen(e.target.checked)} style={{ marginRight: 6 }} />
            skip already-open
          </label>
          {selected.size > 0 && (
            <>
              <button className="btn primary" onClick={() => void restoreSelected(true)}>
                Restore {selected.size} in new window
              </button>
              <button className="btn" onClick={() => void restoreSelected(false)}>Restore here</button>
            </>
          )}
        </div>
      </div>

      <div className="screen-header" style={{ marginTop: -4 }}>
        <input
          type="search"
          placeholder="Search titles, URLs, domains…  (Ctrl+K everywhere)"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          aria-label="Search recently closed tabs"
        />
        <button className={`chip ${pinnedOnly ? 'active' : ''}`} onClick={() => setPinnedOnly((v) => !v)} aria-pressed={pinnedOnly}>
          📌 Pinned only
        </button>
        {(['all', '24h', '7d', '30d'] as Range[]).map((r) => (
          <button key={r} className={`chip ${range === r ? 'active' : ''}`} onClick={() => setRange(r)}>
            {RANGE_LABEL[r]}
          </button>
        ))}
        <select value={browser ?? ''} onChange={(e) => setBrowser(e.target.value || undefined)} aria-label="Browser">
          <option value="">All browsers</option>
          <option value="chrome">Chrome</option>
          <option value="edge">Edge</option>
          <option value="firefox">Firefox</option>
        </select>
        <select value={source} onChange={(e) => setSource(e.target.value as SourceFilter)} aria-label="Source">
          {SOURCE_OPTIONS.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
        </select>
        {rows.length > 0 && (
          <button className="btn small" onClick={toggleAll}>
            {selected.size === rows.length ? 'Clear selection' : 'Select all'}
          </button>
        )}
      </div>

      <div className="list" ref={parentRef} role="list" aria-label="Recently closed tabs">
        {rows.length === 0 && !loading ? (
          <div className="empty">
            <h3>Nothing here yet</h3>
            <p>
              Once the TabVault extension is running, every closed tab lands here — URL, title, pinned state,
              groups and all. <span className="kbd">Ctrl+K</span> searches your entire history.
            </p>
          </div>
        ) : (
          <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
            {items.map((vi) => {
              const row = rows[vi.index];
              const isWindowStart = row.partOfClosedWindow && vi.index >= 0
                ? rows[vi.index - 1]?.windowEventId !== row.windowEventId
                : false;
              return (
                <div
                  key={row.id}
                  role="listitem"
                  className={`row ${isWindowStart ? 'window-start' : ''}`}
                  style={{ position: 'absolute', top: 0, left: 0, right: 0, transform: `translateY(${vi.start}px)` }}
                >
                  <input
                    type="checkbox"
                    checked={selected.has(row.id)}
                    onChange={() => toggle(row.id)}
                    aria-label={`Select ${row.title}`}
                  />
                  <div className="row-main">
                    <Favicon url={row.url} title={row.title} />
                    <div className="row-text">
                      <div className="row-title" title={row.url}>{row.title || row.domain}</div>
                      <div className="row-meta">
                        <span className="domain">{row.domain}</span>
                        <span>{fmtAge(row.closedMs)}</span>
                        {row.activeMs > 0 && <span>{fmtMs(row.activeMs)} active</span>}
                        {row.pinned && <span className="badge pin">pinned</span>}
                        {row.groupName && (
                          <span className="badge group" style={{ color: groupColor(row.groupColor) }}>
                            ● {row.groupName}
                          </span>
                        )}
                        {row.partOfClosedWindow && <span className="badge">window</span>}
                        <span className="badge source">{sourceLabel(row.source)}</span>
                        {row.confidence === 'low' && <span className="badge low">low confidence</span>}
                        <span className="badge">{row.browser}</span>
                      </div>
                    </div>
                  </div>
                  <div className="row-actions">
                    {row.partOfClosedWindow && row.windowEventId != null && (
                      <button className="btn small" title="Re-open the whole window these tabs closed with" onClick={() => void restoreWindowOf(row)}>
                        Restore window
                      </button>
                    )}
                    <button className="btn small primary" onClick={() => void restoreOne(row, false)}>Restore</button>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
