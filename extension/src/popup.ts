// Popup: connection status, quick pause toggle, snapshot on demand.

import browser from 'webextension-polyfill';
import { host } from './host';
import { captureSnapshot } from './capture';
import { setLiveSettings, getLiveSettings } from './state';

const $ = (id: string): HTMLElement => {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el;
};

function fmtLastSeen(ms: number | null): string {
  if (!ms) return 'never';
  const diff = Date.now() - ms;
  if (diff < 60_000) return `${Math.round(diff / 1000)}s ago`;
  if (diff < 3600_000) return `${Math.round(diff / 60_000)}m ago`;
  const hours = Math.floor(diff / 3600_000);
  return `${hours}h ago`;
}

async function refresh(): Promise<void> {
  const info = await host.getStatus();
  const settings = await getLiveSettings();

  const dot = $('dot');
  const status = $('statusText');
  dot.classList.remove('up', 'down');
  if (info.status === 'connected') {
    dot.classList.add('up');
    status.textContent = `host connected (v${info.appVersion ?? '?'})`;
  } else {
    dot.classList.add('down');
    status.textContent = info.lastError ? `host offline — ${info.lastError}` : 'host offline';
  }
  $('queueDepth').textContent = info.queueDepth > 0 ? `${info.queueDepth} queued` : '';
  $('lastSeen').textContent = fmtLastSeen(info.lastContactMs);
  $('recState').textContent = settings.paused ? 'paused' : 'active';
  ($('togglePause') as HTMLButtonElement).textContent = settings.paused
    ? 'Resume recording'
    : 'Pause recording';
  $('version').textContent = `v${browser.runtime.getManifest().version}`;
}

$('snapshotNow').addEventListener('click', () => {
  void (async () => {
    const snapshot = await captureSnapshot();
    await host.send({ type: 'state', snapshot, reason: 'manual' });
    await refresh();
  })();
});

$('togglePause').addEventListener('click', () => {
  void (async () => {
    const settings = await getLiveSettings();
    await setLiveSettings({ paused: !settings.paused });
    await host.send({ type: 'setPaused', paused: !settings.paused });
    await refresh();
  })();
});

$('openApp').addEventListener('click', () => {
  void host.send({ type: 'openApp' });
});

void refresh();
