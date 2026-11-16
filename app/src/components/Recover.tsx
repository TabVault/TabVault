// Recover from the past: auto-detect every browser profile on this machine and
// let the user import what they already lost — no browser extension required.

import { useEffect, useState } from 'react';
import { api, type DetectedProfileInfo, type ImportOutcome, type SessionFileInfo } from '../api';
import { fmtDateTime } from './Shared';

function kindLabel(kind: string): string {
  switch (kind) {
    case 'session_tabs': return 'Tabs (recently closed)';
    case 'session_session': return 'Session (full state)';
    case 'firefox_sessionstore': return 'Firefox sessionstore';
    case 'chromium_history': return 'Chrome/Edge history';
    case 'firefox_places': return 'Firefox history';
    default: return kind;
  }
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export default function Recover({ notify }: { notify: (msg: string) => void }) {
  const [profiles, setProfiles] = useState<DetectedProfileInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [busyFile, setBusyFile] = useState<string | null>(null);
  const [imported, setImported] = useState<ImportOutcome[]>([]);

  const scan = async () => {
    setLoading(true);
    try {
      setProfiles(await api.detectProfiles());
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void scan();
  }, []);

  const importFile = async (file: SessionFileInfo, browser: string, profile: string) => {
    setBusyFile(file.path);
    try {
      const outcome =
        file.kind === 'chromium_history' || file.kind === 'firefox_places'
          ? await api.importHistoryFile(file.path, browser, profile)
          : await api.importSessionFile(file.path, browser, profile);
      setImported((prev) => [outcome, ...prev]);
      notify(`Imported ${outcome.itemsImported} entr${outcome.itemsImported === 1 ? 'y' : 'ies'} from ${outcome.file}.`);
    } catch (e) {
      notify(`Import failed: ${String(e)}`);
    } finally {
      setBusyFile(null);
    }
  };

  const historyFiles = (p: DetectedProfileInfo) =>
    p.files.filter((f) => f.kind === 'chromium_history' || f.kind === 'firefox_places');
  const sessionFiles = (p: DetectedProfileInfo) =>
    p.files.filter((f) => f.kind !== 'chromium_history' && f.kind !== 'firefox_places');

  return (
    <div className="screen">
      <div className="screen-header" style={{ justifyContent: 'space-between' }}>
        <div>
          <h1>Recover from the past</h1>
          <div className="screen-sub">
            Reads the browser's own session files directly — no reinstall, no extension — and shows what
            they still hold. Files are copied before reading; the browser is never touched.
          </div>
        </div>
        <button className="btn" onClick={() => void scan()} disabled={loading}>
          {loading ? 'Scanning…' : 'Rescan'}
        </button>
      </div>

      {profiles.length === 0 && !loading ? (
        <div className="empty">
          <h3>No browser profiles found</h3>
          <p>TabVault looks in the standard Windows locations for Chrome, Edge and Firefox profiles.</p>
        </div>
      ) : (
        <div className="list" style={{ padding: 14, overflow: 'auto' }}>
          {profiles.map((profile) => (
            <section key={`${profile.browser}-${profile.path}`} className="card" style={{ marginBottom: 14 }}>
              <h2 style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                <span className="badge source">{profile.browser}</span>
                <span>{profile.displayName ?? profile.profileDir}</span>
                <span className="muted" style={{ textTransform: 'none', letterSpacing: 0, marginLeft: 'auto', fontSize: 11 }}>
                  {profile.files.length} file{profile.files.length === 1 ? '' : 's'}
                </span>
              </h2>

              <div className="muted" style={{ fontSize: 11, marginBottom: 10, wordBreak: 'break-all' }}>{profile.path}</div>

              {sessionFiles(profile).length > 0 && (
                <>
                  <div className="muted" style={{ fontSize: 11, marginBottom: 6, textTransform: 'uppercase', letterSpacing: '0.06em' }}>Session files</div>
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 6, marginBottom: 12 }}>
                    {sessionFiles(profile).map((file) => (
                      <div key={file.path} className="row" style={{ padding: '8px 12px', minHeight: 0, border: '1px solid var(--border)', borderRadius: 8 }}>
                        <div className="row-text" style={{ flex: 1 }}>
                          <div className="row-title" style={{ fontFamily: 'Consolas, monospace', fontSize: 12 }}>{file.name}</div>
                          <div className="row-meta">
                            <span>{kindLabel(file.kind)}</span>
                            <span>{fmtBytes(file.sizeBytes)}</span>
                            {file.modifiedMs && <span>{fmtDateTime(file.modifiedMs)}</span>}
                            {file.encrypted && <span className="badge low">encrypted (v20) — unreadable</span>}
                          </div>
                        </div>
                        <button
                          className="btn small primary"
                          disabled={file.encrypted || busyFile === file.path}
                          onClick={() => void importFile(file, profile.browser, profile.profileDir)}
                          title={file.encrypted ? 'This file is App-Bound Encrypted; pick the cleartext Sessions/ mirror instead.' : 'Import this file into TabVault'}
                        >
                          {busyFile === file.path ? 'Importing…' : 'Import'}
                        </button>
                      </div>
                    ))}
                  </div>
                </>
              )}

              {historyFiles(profile).length > 0 && (
                <>
                  <div className="muted" style={{ fontSize: 11, marginBottom: 6, textTransform: 'uppercase', letterSpacing: '0.06em' }}>History (seeds visit stats, not open tabs)</div>
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                    {historyFiles(profile).map((file) => (
                      <div key={file.path} className="row" style={{ padding: '8px 12px', minHeight: 0, border: '1px solid var(--border)', borderRadius: 8 }}>
                        <div className="row-text" style={{ flex: 1 }}>
                          <div className="row-title" style={{ fontFamily: 'Consolas, monospace', fontSize: 12 }}>{file.name}</div>
                          <div className="row-meta">
                            <span>{kindLabel(file.kind)}</span>
                            <span>{fmtBytes(file.sizeBytes)}</span>
                            {file.modifiedMs && <span>{fmtDateTime(file.modifiedMs)}</span>}
                          </div>
                        </div>
                        <button
                          className="btn small"
                          disabled={busyFile === file.path}
                          onClick={() => void importFile(file, profile.browser, profile.profileDir)}
                        >
                          {busyFile === file.path ? 'Seeding…' : 'Seed stats'}
                        </button>
                      </div>
                    ))}
                  </div>
                </>
              )}

              {profile.files.length === 0 && (
                <div className="muted" style={{ padding: 8 }}>No readable session or history files found for this profile.</div>
              )}
            </section>
          ))}

          {imported.length > 0 && (
            <div className="card">
              <h2>Import results</h2>
              {imported.map((outcome, i) => (
                <div key={i} className="row" style={{ minHeight: 0, padding: '6px 4px' }}>
                  <div className="row-text" style={{ flex: 1 }}>
                    <span style={{ fontFamily: 'Consolas, monospace' }}>{outcome.file}</span>
                    <span className="muted"> — {outcome.itemsImported} imported of {outcome.itemsFound} found</span>
                    {outcome.pinnedImported > 0 && (
                      <span className="badge pin" title="These pinned tabs were imported with their pin state and are restorable as pinned">
                        📌 {outcome.pinnedImported} pinned
                      </span>
                    )}
                  </div>
                  <span className="badge source">done</span>
                </div>
              ))}
              <p className="muted" style={{ marginBottom: 0 }}>Imported tabs appear in <b>Recently Closed</b> with a source badge.
                Files are filtered to skip this browser's unusable entries.</p>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
