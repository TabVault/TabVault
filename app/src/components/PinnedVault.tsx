// Pinned Tabs Vault: every site ever pinned — with one-click restore of the
// exact pinned setup, a per-row restore, and two bulk scopes: "currently
// pinned" (your live pins) and "everything ever pinned" (full history).

import { useCallback, useEffect, useState } from 'react';
import { api, type PinnedSiteRow, type RestoreMode } from '../api';
import { Favicon, fmtDate } from './Shared';
import { notifyRestoreResult } from './restoreFeedback';

export default function PinnedVault({ notify }: { notify: (msg: string) => void }) {
  const [sites, setSites] = useState<PinnedSiteRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [skipOpen, setSkipOpen] = useState(true);
  const [newWindow, setNewWindow] = useState(true);

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

  const currentPins = sites.filter((s) => s.currentlyPinned);

  const mode: RestoreMode = { newWindow, skipOpen };

  const repinCurrent = () => {
    if (currentPins.length === 0) {
      notify('No sites are currently pinned in your browsers. Use "Re-pin everything ever pinned" to restore from history.');
      return;
    }
    void notifyRestoreResult(notify, api.repinAll(mode, 'current'), currentPins.length);
  };

  const repinAllEver = () => {
    if (sites.length === 0) return;
    void notifyRestoreResult(notify, api.repinAll(mode, 'all'), sites.length);
  };

  const restoreOne = (site: PinnedSiteRow) => {
    void notifyRestoreResult(notify, api.restorePinnedSite(site.id, mode), 1);
  };

  return (
    <div className="screen">
      <div className="screen-header" style={{ justifyContent: 'space-between' }}>
        <div>
          <h1>Pinned Tabs Vault</h1>
          <div className="screen-sub">
            {currentPins.length} currently pinned · {sites.length} site{sites.length === 1 ? '' : 's'} in your full pin history
          </div>
        </div>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', justifyContent: 'flex-end' }}>
          <label className="chip" title="Open the restored tabs in a new window">
            <input type="checkbox" checked={newWindow} onChange={(e) => setNewWindow(e.target.checked)} style={{ marginRight: 6 }} />
            new window
          </label>
          <label className="chip" title="Skip URLs that are already open">
            <input type="checkbox" checked={skipOpen} onChange={(e) => setSkipOpen(e.target.checked)} style={{ marginRight: 6 }} />
            skip already-open
          </label>
          <button
            className="btn primary"
            onClick={repinCurrent}
            disabled={currentPins.length === 0}
            title={currentPins.length === 0 ? 'Nothing is currently pinned in your browsers' : `Re-open your ${currentPins.length} currently-pinned site(s), pinned`}
          >
            📌 Re-pin currently pinned ({currentPins.length})
          </button>
          <button
            className="btn"
            onClick={repinAllEver}
            disabled={sites.length === 0}
            title="Re-open everything the vault has ever recorded as pinned, pinned, with original groups"
          >
            ⟲ Re-pin everything ever pinned ({sites.length})
          </button>
        </div>
      </div>

      {sites.length === 0 && !loading ? (
        <div className="empty">
          <h3>No pinned sites recorded yet</h3>
          <p>Pin a tab in any browser and it will appear here with its pin history — restore any of them pinned, at any time.</p>
        </div>
      ) : (
        <div
          className="list"
          style={{
            padding: 12,
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fill, minmax(340px, 1fr))',
            gap: 10,
            alignContent: 'start',
            overflow: 'auto',
          }}
        >
          {sites.map((site) => (
            <div key={site.id} className="card" style={{ display: 'flex', gap: 12, alignItems: 'center' }}>
              <Favicon url={site.url} title={site.title} />
              <div className="row-text" style={{ flex: 1 }}>
                <div className="row-title" title={site.url}>{site.title || site.url}</div>
                <div className="row-meta">
                  <span className="domain">{site.domain}</span>
                  <span className="badge">{site.browser}</span>
                  {site.currentlyPinned && <span className="badge pin">currently pinned</span>}
                  {site.groupName && <span className="badge group">● {site.groupName}</span>}
                </div>
                <div className="row-meta">
                  <span>first pinned {fmtDate(site.firstPinnedMs)}</span>
                  <span>last pinned {fmtDate(site.lastPinnedMs)}</span>
                  <span>{site.pinCount}×</span>
                </div>
              </div>
              <button className="btn small" title="Open this site, pinned" onClick={() => restoreOne(site)}>
                Restore
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
