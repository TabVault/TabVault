// Pinned Tabs Vault: every site ever pinned, one click to re-pin them all.

import { useCallback, useEffect, useState } from 'react';
import { api, type PinnedSiteRow, type RestoreMode } from '../api';
import { Favicon, fmtDate } from './Shared';

export default function PinnedVault({ notify }: { notify: (msg: string) => void }) {
  const [sites, setSites] = useState<PinnedSiteRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [skipOpen, setSkipOpen] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setSites(await api.pinnedVault());
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const repinAll = async () => {
    const mode: RestoreMode = { newWindow: true, skipOpen };
    try {
      await api.repinAll(mode);
      notify('Re-pinning everything into a new window…');
    } catch (e) {
      notify(`Re-pin failed: ${String(e)}`);
    }
  };

  return (
    <div className="screen">
      <div className="screen-header" style={{ justifyContent: 'space-between' }}>
        <div>
          <h1>Pinned Tabs Vault</h1>
          <div className="screen-sub">{sites.length} site{sites.length === 1 ? '' : 's'} pinned across your browsers — history preserved forever</div>
        </div>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <label className="chip" title="Skip URLs that are already open">
            <input type="checkbox" checked={skipOpen} onChange={(e) => setSkipOpen(e.target.checked)} style={{ marginRight: 6 }} />
            skip already-open
          </label>
          <button className="btn primary" onClick={() => void repinAll()} disabled={sites.length === 0}>
            📌 Re-pin all
          </button>
        </div>
      </div>

      {sites.length === 0 && !loading ? (
        <div className="empty">
          <h3>No pinned sites recorded yet</h3>
          <p>Pin a tab in any browser and it will appear here with its pin history. This is the fastest way back to "my usual setup".</p>
        </div>
      ) : (
        <div className="list" style={{ padding: 12, display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(300px, 1fr))', gap: 10, alignContent: 'start', overflow: 'auto' }}>
          {sites.map((site) => (
            <div key={site.id} className="card" style={{ display: 'flex', gap: 12, alignItems: 'center' }}>
              <Favicon url={site.url} title={site.title} />
              <div className="row-text" style={{ flex: 1 }}>
                <div className="row-title" title={site.url}>{site.title || site.url}</div>
                <div className="row-meta">
                  <span className="domain">{site.domain}</span>
                  <span className="badge">{site.browser}</span>
                  {site.currentlyPinned && <span className="badge pin">currently pinned</span>}
                </div>
                <div className="row-meta">
                  <span>first pinned {fmtDate(site.firstPinnedMs)}</span>
                  <span>last pinned {fmtDate(site.lastPinnedMs)}</span>
                  <span>{site.pinCount}×</span>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
