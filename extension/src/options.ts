// Options page: native host status, recording toggles, first-run import.

import browser from 'webextension-polyfill';
import { host } from './host';
import { getLiveSettings, setLiveSettings } from './state';

const $ = (id: string): HTMLElement => {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el;
};

const logEl = () => $('log');

function log(line: string): void {
  const prev = logEl().textContent ?? '';
  const stamp = new Date().toLocaleTimeString();
  logEl().textContent = `${prev}\n[${stamp}] ${line}`.trim();
}

async function refresh(): Promise<void> {
  const info = await host.getStatus();
  const settings = await getLiveSettings();
  $('hostStatus').textContent =
    info.status === 'connected' ? `connected (v${info.appVersion ?? '?'})` : 'offline';
  $('hostError').textContent = info.lastError ?? 'none';
  $('queueDepth').textContent = String(info.queueDepth);
  $('appVersion').textContent = info.appVersion ?? '—';
  $('interval').textContent = `every ${Math.round(settings.captureIntervalSecs)}s`;
  $('incognito').textContent = settings.captureIncognito ? 'enabled' : 'disabled';
  $('notify').textContent = settings.notifyOnMassClose ? 'enabled' : 'disabled';
  ($('pause') as HTMLButtonElement).textContent = settings.paused ? 'Resume' : 'Pause';
}

$('ping').addEventListener('click', () => {
  void (async () => {
    log('sending ping…');
    await host.ensureConnected();
    await host.send({ type: 'ping' });
    // The pong lands in the background; show liveness instead of a round trip.
    setTimeout(() => void refresh(), 300);
  })();
});

$('flush').addEventListener('click', () => {
  void (async () => {
    const sent = await host.flushQueue();
    log(`flushed ${sent} queued message(s)`);
    await refresh();
  })();
});

$('pause').addEventListener('click', () => {
  void (async () => {
    const settings = await getLiveSettings();
    const next = !settings.paused;
    await setLiveSettings({ paused: next });
    await host.send({ type: 'setPaused', paused: next });
    log(next ? 'recording paused' : 'recording resumed');
    await refresh();
  })();
});

$('openApp').addEventListener('click', () => {
  void (async () => {
    await host.send({ type: 'openApp' });
    log('asked the host to launch TabVault');
  })();
});

// ---------------------------------------------------------------------------
// First-run import via the browser's own APIs
// ---------------------------------------------------------------------------

interface SessionsApi {
  getRecentlyClosed(filter?: { maxResults?: number }): Promise<
    Array<{
      lastModified?: number;
      tab?: { url?: string; title?: string; pinned?: boolean; favIconUrl?: string };
      window?: { tabs?: Array<{ url?: string; title?: string; pinned?: boolean; favIconUrl?: string }> };
    }>
  >;
}

$('importSessions').addEventListener('click', () => {
  void (async () => {
    try {
      const sessions = (browser as unknown as { sessions?: SessionsApi }).sessions;
      if (!sessions) {
        log('this browser does not expose the sessions API');
        return;
      }
      const recently = await sessions.getRecentlyClosed({ maxResults: 25 });
      const entries = recently.flatMap((entry) => {
        const favicon = entry.tab?.favIconUrl ?? null;
        if (entry.tab?.url) {
          return [
            {
              url: entry.tab.url,
              title: entry.tab.title ?? '',
              pinned: entry.tab.pinned ?? false,
              favicon,
              lastActiveMs: entry.lastModified ? entry.lastModified * 1000 : null,
            },
          ];
        }
        if (entry.window?.tabs) {
          return entry.window.tabs
            .filter((t) => !!t.url)
            .map((t) => ({
              url: t.url!,
              title: t.title ?? '',
              pinned: t.pinned ?? false,
              favicon: t.favIconUrl ?? null,
              lastActiveMs: entry.lastModified ? entry.lastModified * 1000 : null,
            }));
        }
        return [];
      });
      if (entries.length === 0) {
        log('nothing to import (no recently closed entries with URLs)');
        return;
      }
      log(`importing ${entries.length} entr${entries.length === 1 ? 'y' : 'ies'}…`);
      await host.send({ type: 'importSessions', entries });
      log('import requested; watch the desktop app');
    } catch (error) {
      log(`import failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  })();
});

interface HistoryApi {
  search(query: {
    text: string;
    startTime?: number;
    maxResults?: number;
  }): Promise<Array<{ url?: string; title?: string; lastVisitTime?: number; visitCount?: number }>>;
}

$('importHistory').addEventListener('click', () => {
  void (async () => {
    try {
      const history = (browser as unknown as { history?: HistoryApi }).history;
      if (!history) {
        log('this browser does not expose the history API');
        return;
      }
      const startTime = Date.now() - 90 * 86_400_000;
      const results = await history.search({ text: '', startTime, maxResults: 500 });
      const entries = results
        .filter((r) => !!r.url && (r.visitCount ?? 0) > 0)
        .map((r) => ({
          url: r.url!,
          title: r.title ?? '',
          visitCount: r.visitCount ?? 1,
          lastVisitTime: r.lastVisitTime ?? null,
        }));
      if (entries.length === 0) {
        log('nothing to seed from history');
        return;
      }
      log(`seeding ${entries.length} URLs…`);
      await host.send({ type: 'importHistory', entries });
      log('seed requested');
    } catch (error) {
      log(`seed failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  })();
});

void refresh();
