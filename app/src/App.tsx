// TabVault desktop shell: sidebar + screens, global hotkeys, pause state.

import { useCallback, useEffect, useState } from 'react';
import Sidebar, { type Screen } from './components/Sidebar';
import RecentlyClosed from './components/RecentlyClosed';
import Timeline from './components/Timeline';
import PinnedVault from './components/PinnedVault';
import MostUsed from './components/MostUsed';
import Settings from './components/Settings';
import Recover from './components/Recover';
import GlobalSearch from './components/GlobalSearch';
import { api } from './api';

export default function App() {
  const [screen, setScreen] = useState<Screen>('recently');
  const [searchOpen, setSearchOpen] = useState(false);
  const [paused, setPaused] = useState(false);
  const [stats, setStats] = useState<{ closedTabs: number; snapshots: number } | null>(null);
  const [toast, setToast] = useState<string | null>(null);

  const notify = useCallback((message: string) => {
    setToast(message);
    window.clearTimeout((notify as { _t?: number })._t);
    (notify as { _t?: number })._t = window.setTimeout(() => setToast(null), 6000);
  }, []);

  // Load settings + dashboard stats once at boot, then refresh stats on focus.
  useEffect(() => {
    void api.settingsGet().then((s) => {
      setPaused(s.recordingPaused);
      document.documentElement.dataset.theme =
        s.theme === 'system' ? (window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark') : s.theme;
      if (s.theme === 'system') {
        const media = window.matchMedia('(prefers-color-scheme: light)');
        const listener = () => {
          document.documentElement.dataset.theme = media.matches ? 'light' : 'dark';
        };
        media.addEventListener('change', listener);
        return () => media.removeEventListener('change', listener);
      }
    });
    void api.dashboard().then((d) => setStats({ closedTabs: d.closedTabs, snapshots: d.snapshots }));
  }, []);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      const inField = tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || (e.target as HTMLElement)?.isContentEditable;
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        setSearchOpen((v) => !v);
        return;
      }
      if (e.ctrlKey && e.altKey && e.key.toLowerCase() === 'r') {
        e.preventDefault();
        void api.restoreLastClosedWindow().then((job) => {
          notify(job ? 'Restoring your most recently closed window…' : 'No closed window on record yet.');
        });
        return;
      }
      if (inField) return;
      // Alt+1..6 jumps straight to a screen.
      if (e.altKey && ['1', '2', '3', '4', '5', '6'].includes(e.key)) {
        const screens: Screen[] = ['recently', 'timeline', 'pinned', 'mostused', 'recover', 'settings'];
        setScreen(screens[Number(e.key) - 1]);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [notify]);

  return (
    <div className="app-shell">
      <Sidebar
        current={screen}
        onSelect={setScreen}
        onSearch={() => setSearchOpen(true)}
        paused={paused}
        stats={stats}
      />
      <main className="main">
        {screen === 'recently' && <RecentlyClosed notify={notify} />}
        {screen === 'timeline' && <Timeline notify={notify} />}
        {screen === 'pinned' && <PinnedVault notify={notify} />}
        {screen === 'mostused' && <MostUsed />}
        {screen === 'recover' && <Recover notify={notify} />}
        {screen === 'settings' && <Settings notify={notify} />}
      </main>

      <GlobalSearch open={searchOpen} onClose={() => setSearchOpen(false)} notify={notify} />

      {toast && (
        <div className="toast" role="status">
          {toast}
        </div>
      )}
    </div>
  );
}
