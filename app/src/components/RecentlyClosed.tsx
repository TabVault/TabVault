// Recently Closed: searchable, filterable, virtualized list of every tab
// ever seen closing. The home screen of TabVault.
//
// "Group by source file" view: the spec asks that recovery results are
// presented grouped by source file/time, so this mode interleaves file banner
// rows (file name, source, imported at, item counts) with the tabs imported
// from each file. Rows captured live sit under their own "Live capture"
// banner.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { api, type ClosedTabRow, type ImportBatchRow, type RestoreMode } from '../api';
import { Favicon, fmtAge, fmtDateTime, fmtMs, sourceLabel, groupColor } from './Shared';
import { notifyRestoreResult } from './restoreFeedback';

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

function batchLabel(batch: ImportBatchRow): { title: string; subtitle: string } {
  const path = batch.sourcePath?.split(/[\\/]/).pop() ?? 'unknown file';
  const when = batch.startedMs ? fmtDateTime(batch.startedMs) : '';
  const counts = `${batch.importedTabs} tab${batch.importedTabs === 1 ? '' : 's'}`;
  return {
    title: path,
    subtitle: `${sourceLabel(batch.source)} · imported ${when} · ${counts} found in file`,
  };
}

type ListItem =
  | { type: 'banner'; key: string; title: string; subtitle: string; batchId: number | null }
  | { type: 'row'; key: string; row: ClosedTabRow };

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
  const [grouped, setGrouped] = useState(false);
  const [batches, setBatches] = useState<ImportBatchRow[]>([]);
  const [focusedIndex, setFocusedIndex] = useState<number | null>(null);
  const parentRef = useRef<HTMLDivElement>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [page, batchList] = await Promise.all([
        api.recentlyClosed({
          q: query.trim() || undefined,
          pinnedOnly,
          browser,
          source: source || undefined,
          dateFrom: rangeToFrom(range),
          limit: 1000,
          offset: 0,
        }),
        api.importBatches().catch(() => [] as ImportBatchRow[]),
      ]);
      setRows(page.rows);
      setTotal(page.total);
      setBatches(batchList);
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
  }, [load]);

  const items = useMemo<ListItem[]>(() => {
    if (!grouped) {
      return rows.map((row) => ({ type: 'row', key: `r-${row.id}`, row }));
    }
    const batchById = new Map<number, ImportBatchRow>();
    for (const b of batches) batchById.set(b.id, b);

    const groups = new Map<number | null, ClosedTabRow[]>();
    for (const row of rows) {
      const key = row.sourceBatchId ?? null;
      const bucket = groups.get(key);
      if (bucket) bucket.push(row);
      else groups.set(key, [row]);
    }

    const out: ListItem[] = [];
    // Live capture first (that is the common case), then batches newest-first.
    const liveRows = groups.get(null);
    if (liveRows && liveRows.length > 0) {
      out.push({
        type: 'banner',
        key: 'banner-live',
        title: 'Live capture',
        subtitle: `${liveRows.length} tab${liveRows.length === 1 ? '' : 's'} recorded while you browsed`,
        batchId: null,
      });
      for (const row of liveRows) out.push({ type: 'row', key: `r-${row.id}`, row });
    }

    const batchIds = [...new Set(rows.map((r) => r.sourceBatchId).filter((v): v is number => v != null))]
      .sort((a, b) => (batchById.get(b)?.startedMs ?? 0) - (batchById.get(a)?.startedMs ?? 0));

    for (const id of batchIds) {
      const rowsInBatch = groups.get(id);
      if (!rowsInBatch) continue;
      const batch = batchById.get(id);
      const { title, subtitle } = batch
        ? batchLabel(batch)
        : { title: `Import #${id}`, subtitle: 'batch details unavailable' };
      out.push({
        type: 'banner',
        key: `banner-${id}`,
        title,
        subtitle,
        batchId: id,
      });
      for (const row of rowsInBatch) out.push({ type: 'row', key: `r-${row.id}`, row });
    }
    return out;
  }, [grouped, rows, batches]);

  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => parentRef.current,
    estimateSize: (index) => (items[index]?.type === 'banner' ? 38 : 54),
    overscan: 12,
  });

  // Keep the focused row visible when jumping with the keyboard.
  useEffect(() => {
    if (focusedIndex !== null) {
      virtualizer.scrollToIndex(focusedIndex, { align: 'auto' });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusedIndex]);

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

  const restoreSelected = (newWindow: boolean) => {
    if (selected.size === 0) return;
    void notifyRestoreResult(notify, api.restoreTabs([...selected], restoreMode(newWindow)));
  };

  const restoreOne = (row: ClosedTabRow, newWindow: boolean) => {
    void notifyRestoreResult(notify, api.restoreTabs([row.id], restoreMode(newWindow)));
  };

  const restoreWindowOf = (row: ClosedTabRow) => {
    if (row.windowEventId == null) return;
    void notifyRestoreResult(notify, api.restoreWindow(row.windowEventId, restoreMode(true)));
  };

  const onListKeyDown = (e: React.KeyboardEvent) => {
    if (items.length === 0) return;
    const current = focusedIndex ?? 0;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const step = e.key === 'ArrowDown' ? 1 : -1;
      let next = current;
      for (let i = 0; i < items.length; i++) {
        next = (next + step + items.length) % items.length;
        if (items[next].type === 'row') break;
      }
      setFocusedIndex(next);
    } else if (e.key === 'Home') {
      e.preventDefault();
      setFocusedIndex(items.findIndex((i) => i.type === 'row'));
    } else if (e.key === 'End') {
      e.preventDefault();
      for (let i = items.length - 1; i >= 0; i--) {
        if (items[i].type === 'row') {
          setFocusedIndex(i);
          break;
        }
      }
    } else if ((e.key === 'Enter' || e.key === ' ') && focusedIndex !== null) {
      const item = items[focusedIndex];
      if (item?.type === 'row') {
        e.preventDefault();
        toggle(item.row.id);
      }
    } else if (e.key === 'Escape') {
      setSelected(new Set());
    }
  };

  const vitems = virtualizer.getVirtualItems();

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
          <button
            className={`chip ${grouped ? 'active' : ''}`}
            aria-pressed={grouped}
            title="Group recovered rows by the session/history file they came from"
            onClick={() => setGrouped((v) => !v)}
          >
            ⧉ Group by source file
          </button>
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

      <div
        className="list"
        ref={parentRef}
        role="list"
        aria-label="Recently closed tabs"
        tabIndex={0}
        onKeyDown={onListKeyDown}
      >
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
            {vitems.map((vi) => {
              const item = items[vi.index];
              if (item.type === 'banner') {
                return (
                  <div
                    key={item.key}
                    role="presentation"
                    className="group-banner"
                    style={{ position: 'absolute', top: 0, left: 0, right: 0, height: 36, transform: `translateY(${vi.start}px)` }}
                  >
                    <span className="badge source">{item.batchId === null ? 'live' : `import #${item.batchId}`}</span>
                    <span style={{ fontWeight: 650 }}>{item.title}</span>
                    <span className="muted">{item.subtitle}</span>
                  </div>
                );
              }
              const row = item.row;
              const rowIndex = vi.index;
              const focused = focusedIndex === rowIndex;
              const isSelected = selected.has(row.id);
              // Highlight the first tab of a window batch (same window_event_id
              // as the tab before it).
              let prevRow: ClosedTabRow | undefined;
              for (let i = vi.index - 1; i >= 0; i--) {
                const it = items[i];
                if (it?.type === 'row') {
                  prevRow = it.row;
                  break;
                }
                if (it?.type === 'banner') break;
              }
              const isWindowStart = row.partOfClosedWindow && prevRow?.windowEventId !== row.windowEventId;
              return (
                <div
                  key={row.id}
                  role="listitem"
                  aria-selected={isSelected}
                  data-focused={focused || undefined}
                  className={`row ${isWindowStart ? 'window-start' : ''}`}
                  onClick={() => setFocusedIndex(rowIndex)}
                  style={{
                    position: 'absolute', top: 0, left: 0, right: 0, transform: `translateY(${vi.start}px)`,
                    background: focused ? 'var(--accent-soft)' : undefined,
                  }}
                >
                  <input
                    type="checkbox"
                    checked={isSelected}
                    onChange={() => toggle(row.id)}
                    aria-label={`Select ${row.title}`}
                    tabIndex={-1}
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
