// Settings: capture cadence, retention tiers, exclusions, theme, backup.
// The vault stays the single source of truth; the extension reads these via poll.

import { useEffect, useState } from 'react';
import { api, type Settings as SettingsData } from '../api';

export default function Settings({ notify }: { notify: (msg: string) => void }) {
  const [settings, setSettings] = useState<SettingsData | null>(null);
  const [exclusionsText, setExclusionsText] = useState('');
  const [preview, setPreview] = useState<[number, number, number, number] | null>(null);

  useEffect(() => {
    void api.settingsGet().then((s) => {
      setSettings(s);
      setExclusionsText(s.exclusions.join('\n'));
    });
    void api.retentionPreview().then(setPreview).catch(() => setPreview(null));
  }, []);

  const update = (patch: Partial<SettingsData>) => {
    setSettings((prev) => {
      if (!prev) return prev;
      const next = { ...prev, ...patch };
      void api.settingsSet(next).then(() => {
        void api.retentionPreview().then(setPreview).catch(() => undefined);
      });
      return next;
    });
  };

  const saveExclusions = () => {
    const list = exclusionsText
      .split('\n')
      .map((l) => l.trim())
      .filter((l) => l && !l.startsWith('#'));
    update({ exclusions: list });
    notify('Exclusion list saved.');
  };

  if (!settings) {
    return <div className="screen"><div className="muted">Loading…</div></div>;
  }

  return (
    <div className="screen" style={{ overflow: 'hidden' }}>
      <div className="screen-header">
        <h1>Settings</h1>
      </div>
      <div className="settings-grid">
        <section className="card">
          <h2>Recording</h2>
          <div className="form-row">
            <div>
              <div className="label">Pause recording</div>
              <div className="help">Stops all capture. Stored history is untouched. Toggle from the tray anytime.</div>
            </div>
            <button className={`btn ${settings.recordingPaused ? 'primary' : 'danger'}`} onClick={() => update({ recordingPaused: !settings.recordingPaused })}>
              {settings.recordingPaused ? 'Paused — resume' : 'Pause now'}
            </button>
          </div>
          <div className="form-row">
            <div>
              <div className="label">Capture interval</div>
              <div className="help">Full-state snapshot every N seconds (deduplicated — idle browser writes nothing).</div>
            </div>
            <select
              value={settings.captureIntervalSecs}
              onChange={(e) => update({ captureIntervalSecs: Number(e.target.value) })}
            >
              {[15, 30, 60, 120, 300, 600].map((s) => (
                <option key={s} value={s}>{s < 60 ? `${s}s` : `${s / 60} min`}</option>
              ))}
            </select>
          </div>
          <div className="form-row">
            <div>
              <div className="label">Private / incognito windows</div>
              <div className="help">Capture them too. Off by default — private means private.</div>
            </div>
            <button className={`chip ${settings.captureIncognito ? 'active' : ''}`} onClick={() => update({ captureIncognito: !settings.captureIncognito })}>
              {settings.captureIncognito ? 'Captured' : 'Skipped'}
            </button>
          </div>
          <div className="form-row">
            <div>
              <div className="label">Domain exclusions</div>
              <div className="help">One per line. These never get recorded — banking, password managers, mail. Suffix match: <code>bank.com</code> covers <code>www.bank.com</code>.</div>
            </div>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 8, minWidth: 320 }}>
              <textarea
                rows={5}
                value={exclusionsText}
                onChange={(e) => setExclusionsText(e.target.value)}
                spellCheck={false}
                placeholder={'bank.com\nchase.com\n1password.com'}
              />
              <button className="btn small" onClick={saveExclusions}>Save exclusions</button>
            </div>
          </div>
        </section>

        <section className="card">
          <h2>Retention (snapshots only — closed-tab history is permanent)</h2>
          <div className="form-row">
            <div>
              <div className="label">Keep everything for …</div>
              <div className="help">The rolling recent window, every minute, untouched.</div>
            </div>
            <select value={settings.retentionKeepAllHours} onChange={(e) => update({ retentionKeepAllHours: Number(e.target.value) })}>
              {[6, 12, 24, 48, 72, 168].map((h) => <option key={h} value={h}>{h < 48 ? `${h}h` : `${h / 24} days`}</option>)}
            </select>
          </div>
          <div className="form-row">
            <div>
              <div className="label">…then hourly for …</div>
              <div className="help">One snapshot per hour past the recent window.</div>
            </div>
            <select value={settings.retentionHourlyDays} onChange={(e) => update({ retentionHourlyDays: Number(e.target.value) })}>
              {[7, 14, 30, 90, 180].map((d) => <option key={d} value={d}>{d} days</option>)}
            </select>
          </div>
          <div className="form-row">
            <div>
              <div className="label">…then daily for …</div>
              <div className="help">One snapshot per day, then deletion. {preview ? `Now keeping ${preview[0]} recent + ${preview[1]} hourly + ${preview[2]} daily, deleting ${preview[3].toLocaleString()}.` : ''}</div>
            </div>
            <select value={settings.retentionDailyDays} onChange={(e) => update({ retentionDailyDays: Number(e.target.value) })}>
              {[90, 180, 365, 730, 1825].map((d) => <option key={d} value={d}>{d === 1825 ? '5 years' : `${d} days`}</option>)}
            </select>
          </div>
          <div className="form-row">
            <div>
              <div className="label">Recovery points live forever</div>
              <div className="help">Mass-close snapshots (5+ tabs or a whole window at once) are never auto-deleted.</div>
            </div>
            <button className={`chip ${settings.keepRecoveryPointsForever ? 'active' : ''}`} onClick={() => update({ keepRecoveryPointsForever: !settings.keepRecoveryPointsForever })}>
              {settings.keepRecoveryPointsForever ? 'Kept forever' : 'Deletable'}
            </button>
          </div>
          <div className="form-row">
            <div>
              <div className="label">Prune old snapshots now</div>
              <div className="help">Applies the tiers above immediately.</div>
            </div>
            <button className="btn" onClick={() => void api.retentionRun().then((r) => notify(`Pruned: ${Object.entries(r).map(([k, v]) => `${v} ${k}`).join(', ') || 'nothing to prune'}.`))}>
              Prune now
            </button>
          </div>
        </section>

        <section className="card">
          <h2>Notifications</h2>
          <div className="form-row">
            <div>
              <div className="label">Mass-close toast</div>
              <div className="help">Windows toast when 5+ tabs or a whole window close at once. Button: restore.</div>
            </div>
            <button className={`chip ${settings.notifyOnMassClose ? 'active' : ''}`} onClick={() => update({ notifyOnMassClose: !settings.notifyOnMassClose })}>
              {settings.notifyOnMassClose ? 'On' : 'Off'}
            </button>
          </div>
          <div className="form-row">
            <div>
              <div className="label">Mass-close threshold</div>
              <div className="help">Tabs within {settings.massCloseWindowSecs}s that trigger a recovery point.</div>
            </div>
            <select value={settings.massCloseTabThreshold} onChange={(e) => update({ massCloseTabThreshold: Number(e.target.value) })}>
              {[3, 5, 8, 10, 15, 20].map((n) => <option key={n} value={n}>{n} tabs</option>)}
            </select>
          </div>
        </section>

        <section className="card">
          <h2>Appearance &amp; startup</h2>
          <div className="form-row">
            <div>
              <div className="label">Theme</div>
            </div>
            <select value={settings.theme} onChange={(e) => update({ theme: e.target.value as SettingsData['theme'] })}>
              <option value="system">System</option>
              <option value="dark">Dark</option>
              <option value="light">Light</option>
            </select>
          </div>
          <div className="form-row">
            <div>
              <div className="label">Start with Windows</div>
              <div className="help">Launches minimized to the tray at login.</div>
            </div>
            <button className={`chip ${settings.startWithWindows ? 'active' : ''}`} onClick={() => update({ startWithWindows: !settings.startWithWindows })}>
              {settings.startWithWindows ? 'Enabled' : 'Disabled'}
            </button>
          </div>
          <div className="form-row">
            <div>
              <div className="label">Daily backup</div>
              <div className="help">A consistent copy of the vault into backups/, kept for {settings.backupKeep} days.</div>
            </div>
            <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <button className={`chip ${settings.dailyBackup ? 'active' : ''}`} onClick={() => update({ dailyBackup: !settings.dailyBackup })}>
                {settings.dailyBackup ? 'On' : 'Off'}
              </button>
              <button className="btn small" onClick={() => void api.backupNow().then((p) => notify(`Backup written to ${p}`))}>
                Back up now
              </button>
            </div>
          </div>
          <div className="form-row">
            <div>
              <div className="label">Database folder</div>
              <div className="help">Everything stays on this machine — %APPDATA%\TabVault. No network, no cloud, no telemetry.</div>
            </div>
            <button className="btn" onClick={() => void api.openDataFolder()}>Open folder</button>
          </div>
        </section>
      </div>
    </div>
  );
}
