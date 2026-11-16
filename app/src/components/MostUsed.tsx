// Most Used: your top sites by visits and active time, across windows.

import { useCallback, useEffect, useMemo, useState } from 'react';
import { api, type SiteStatRow } from '../api';
import { Favicon, fmtMs } from './Shared';

type Window = 'day' | 'week' | 'month' | 'all';
type Metric = 'count' | 'active';

const WINDOWS: { key: Window; label: string }[] = [
  { key: 'day', label: 'Today' },
  { key: 'week', label: 'This week' },
  { key: 'month', label: 'This month' },
  { key: 'all', label: 'All time' },
];

export default function MostUsed() {
  const [sites, setSites] = useState<SiteStatRow[]>([]);
  const [window, setWindow] = useState<Window>('week');
  const [metric, setMetric] = useState<Metric>('count');
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setSites(await api.topSites(window, metric, 40));
    } finally {
      setLoading(false);
    }
  }, [window, metric]);

  useEffect(() => {
    void load();
  }, [load]);

  const max = useMemo(
    () => sites.reduce((acc, s) => Math.max(acc, metric === 'count' ? s.visitCount : s.activeMs), 1),
    [sites, metric],
  );

  return (
    <div className="screen">
      <div className="screen-header" style={{ justifyContent: 'space-between' }}>
        <div>
          <h1>Most Used</h1>
          <div className="screen-sub">where your attention actually goes — seeded from history, kept forever</div>
        </div>
        <div style={{ display: 'flex', gap: 8 }}>
          {WINDOWS.map((w) => (
            <button key={w.key} className={`chip ${window === w.key ? 'active' : ''}`} onClick={() => setWindow(w.key)}>
              {w.label}
            </button>
          ))}
          <div style={{ width: 1 }} />
          <button className={`chip ${metric === 'count' ? 'active' : ''}`} onClick={() => setMetric('count')}>Visits</button>
          <button className={`chip ${metric === 'active' ? 'active' : ''}`} onClick={() => setMetric('active')}>Active time</button>
        </div>
      </div>

      {sites.length === 0 && !loading ? (
        <div className="empty">
          <h3>No statistics yet</h3>
          <p>Use your browser and the numbers fill in. You can also seed them from your browser's history via “Recover from the past”.</p>
        </div>
      ) : (
        <div className="list" style={{ padding: 10, display: 'flex', flexDirection: 'column', gap: 8, overflow: 'auto' }}>
          {sites.map((site, i) => {
            const value = metric === 'count' ? site.visitCount : site.activeMs;
            return (
              <div key={site.id} className="site-card">
                <span className="muted" style={{ width: 26, textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>{i + 1}</span>
                <Favicon url={site.url} title={site.title} />
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div className="row-title" title={site.url}>{site.title || site.domain}</div>
                  <div className="row-meta">
                    <span className="domain">{site.domain}</span>
                    {site.pinned && <span className="badge pin">pinned</span>}
                  </div>
                  <div className="bar" aria-hidden>
                    <div style={{ width: `${(value / max) * 100}%` }} />
                  </div>
                </div>
                <div style={{ textAlign: 'right', minWidth: 92 }}>
                  <div style={{ fontWeight: 700, fontVariantNumeric: 'tabular-nums' }}>
                    {metric === 'count' ? site.visitCount.toLocaleString() : fmtMs(site.activeMs)}
                  </div>
                  <div className="muted" style={{ fontSize: 11 }}>{metric === 'count' ? 'visits' : 'active'}</div>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
