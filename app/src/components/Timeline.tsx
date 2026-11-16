// Timeline: browse snapshots by date and preview exactly what existed then.

import { useCallback, useEffect, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { useRef } from 'react';
import { api, type BrowserSnapshot, type RestoreMode, type SnapshotRow } from '../api';
import { Favicon, fmtDateTime, groupColor } from './Shared';

export default function Timeline({ notify }: { notify: (msg: string) => void }) {
  const [snapshots, setSnapshots] = useState<SnapshotRow[]>([]);
  const [selected, setSelected] = useState<SnapshotRow | null>(null);
  const [detail, setDetail] = useState<BrowserSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [restoring, setRestoring] = useState(false);
  const [skipOpen, setSkipOpen] = useState(true);

  const parentRef = useRef<HTMLDivElement>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const rows = await api.timeline(undefined, undefined, undefined, 400, 0);
      setSnapshots(rows);
      setSelected((cur) => cur ?? rows[0] ?? null);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!selected) {
      setDetail(null);
      return;
    }
    void api.snapshotDetail(selected.id).then(setDetail).catch(() => setDetail(null));
  }, [selected]);

  const virtualizer = useVirtualizer({
    count: snapshots.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 54,
    overscan: 12,
  });

  const restoreMode = (): RestoreMode => ({ newWindow: true, skipOpen });

  const restoreAll = async () => {
    if (!selected) return;
    setRestoring(true);
    try {
      await api.restoreSnapshot(selected.id, restoreMode());
      notify('Restoring the whole snapshot in a new window…');
    } catch (e) {
      notify(`Restore failed: ${String(e)}`);
    } finally {
      setRestoring(false);
    }
  };

  const reasonLabel = (row: SnapshotRow) => {
    switch (row.reason) {
      case 'mass_close': return 'RECOVERY POINT';
      case 'tab_closed': return 'tab closed';
      case 'startup': return 'browser start';
      case 'manual': return 'manual';
      case 'imported': return 'imported';
      default: return row.reason;
    }
  };

  return (
    <div className="screen">
      <div className="screen-header" style={{ justifyContent: 'space-between' }}>
        <div>
          <h1>Timeline</h1>
          <div className="screen-sub">{snapshots.length} snapshots recorded — every point in time is restorable</div>
        </div>
        <div style={{ display: 'flex', gap: 8 }}>
          <label className="chip" title="Skip URLs that are already open">
            <input type="checkbox" checked={skipOpen} onChange={(e) => setSkipOpen(e.target.checked)} style={{ marginRight: 6 }} />
            skip already-open
          </label>
          <button className="btn primary" disabled={!selected || restoring} onClick={() => void restoreAll()}>
            {restoring ? 'Restoring…' : 'Restore this snapshot'}
          </button>
        </div>
      </div>

      <div className="timeline-layout">
        <div className="timeline-list" ref={parentRef} role="list" aria-label="Snapshots">
          {snapshots.length === 0 && !loading ? (
            <div className="empty">
              <h3>No snapshots yet</h3>
              <p>Snapshots arrive as you browse — every minute and every close — fully offline.</p>
            </div>
          ) : (
            <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
              {virtualizer.getVirtualItems().map((vi) => {
                const row = snapshots[vi.index];
                return (
                  <button
                    key={row.id}
                    role="listitem"
                    className={`snap-row ${selected?.id === row.id ? 'selected' : ''}`}
                    style={{ position: 'absolute', top: 0, left: 0, right: 0, transform: `translateY(${vi.start}px)` }}
                    onClick={() => setSelected(row)}
                  >
                    <span className="snap-time">{fmtDateTime(row.takenMs)}</span>
                    <span className="snap-meta">
                      {row.recoveryPoint && <span className="badge" style={{ color: '#f5c56b', borderColor: '#f5c56b' }}>recovery point</span>}
                      {' '}{row.tabCount} tab{row.tabCount === 1 ? '' : 's'} · {row.windowCount} win
                      <br />{reasonLabel(row)}
                    </span>
                  </button>
                );
              })}
            </div>
          )}
        </div>

        <div className="timeline-detail card" aria-label="Snapshot detail">
          {!detail ? (
            <div className="muted" style={{ padding: 20 }}>{loading ? 'Loading…' : 'Select a snapshot on the left to preview it.'}</div>
          ) : (
            <>
              <div className="screen-sub" style={{ marginBottom: 12 }}>
                {fmtDateTime(selected?.takenMs)} · {detail.browser} · {detail.windows.length} window{detail.windows.length === 1 ? '' : 's'}
                {detail.windows.flatMap((w) => w.tabs).length} tabs
              </div>
              {detail.windows.map((window) => (
                <div key={window.windowId} style={{ marginBottom: 18 }}>
                  <div className="group-banner" style={{ borderRadius: 8, marginBottom: 6 }}>
                    Window #{window.windowId}{window.focused ? ' · focused' : ''}{window.incognito ? ' · incognito (excluded from capture)' : ''}
                    <span style={{ marginLeft: 'auto' }}>{window.tabs.length} tabs</span>
                  </div>
                  {window.tabs.map((tab, i) => (
                    <div key={`${window.windowId}-${i}`} className="row" style={{ minHeight: 42, padding: '6px 12px' }}>
                      <Favicon url={tab.url} title={tab.title} />
                      <div className="row-text">
                        <div className="row-title" title={tab.url}>{tab.title || tab.url}</div>
                        <div className="row-meta">
                          <span className="domain">{tab.groupName ? <span style={{ color: groupColor(tab.groupColor) }}>● {tab.groupName} · </span> : null}{(() => { try { return new URL(tab.url).hostname; } catch { return tab.url; } })()}</span>
                          {tab.pinned && <span className="badge pin">pinned</span>}
                        </div>
                      </div>
                    </div>
                  ))}
                </div>
              ))}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
