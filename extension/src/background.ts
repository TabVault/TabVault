// TabVault service worker: orchestrates capture, host messaging, mass-close
// detection and active-time accounting.
//
// Message flow:
//   browser event → debounced capture → full BrowserSnapshot → host
//   host poll reply → restore jobs → we execute them → restoreDone
//
// The host, not this worker, decides what to store — we ship state, it diffs,
// dedupes, retains and notifies.

import browser from 'webextension-polyfill';
import { captureSnapshot, shouldSkipUrl } from './capture';
import { host } from './host';
import { addActiveMs, getLiveSettings, markVisit, recordClose, recentCloseCount, setFavicon, forgetTab, setLiveSettings } from './state';
import { runRestoreJob } from './restore';
import { debounce, log, nowMs, warn } from './util';
import type { SnapshotReason } from './types';

const CAPTURE_ALARM = 'tabvault.capture';
const POLL_ALARM = 'tabvault.poll';
const FLUSH_ALARM = 'tabvault.flush';

// ---------------------------------------------------------------------------
// Capture scheduling
// ---------------------------------------------------------------------------

let pendingCloses = 0;

async function capture(reason: SnapshotReason): Promise<void> {
  const settings = await getLiveSettings();
  if (settings.paused) {
    log('recording paused; skipping capture');
    return;
  }
  try {
    const snapshot = await captureSnapshot();
    const sent = await host.send({ type: 'state', snapshot, reason });
    if (!sent) {
      log('host down; snapshot queued');
    }
    if (reason === 'tab_closed' || reason === 'mass_close') {
      pendingCloses = 0;
    }
  } catch (error) {
    warn('capture failed', error);
  }
}

const scheduleCapture = debounce(() => {
  const reason: SnapshotReason = pendingCloses > 0 ? 'tab_closed' : 'interval';
  void capture(reason);
}, 1500);

// ---------------------------------------------------------------------------
// Active time accounting
// ---------------------------------------------------------------------------

let activeTabId: number | null = null;
let activeSince: number | null = null;

async function rollActiveClock(): Promise<void> {
  if (activeTabId !== null && activeSince !== null) {
    const now = nowMs();
    const delta = now - activeSince;
    if (delta > 0) {
      await addActiveMs(activeTabId, delta);
    }
    activeSince = now;
  }
}

async function setActiveTab(tabId: number | null): Promise<void> {
  await rollActiveClock();
  activeTabId = tabId;
  activeSince = tabId !== null ? nowMs() : null;
}

// ---------------------------------------------------------------------------
// Event wiring
// ---------------------------------------------------------------------------

browser.tabs.onCreated.addListener(() => {
  scheduleCapture();
});

browser.tabs.onUpdated.addListener(
  (tabId, changeInfo) => {
    const url = changeInfo.url;
    if (url) {
      if (!shouldSkipUrl(url)) {
        void markVisit(tabId);
      }
    }
    if (changeInfo.title || changeInfo.favIconUrl) {
      if (changeInfo.favIconUrl) {
        void setFavicon(tabId, changeInfo.favIconUrl);
      }
      scheduleCapture();
    }
  },
);

browser.tabs.onRemoved.addListener((tabId) => {
  pendingCloses += 1;
  void recordClose();
  void forgetTab(tabId);
  void setActiveTab(null);
  void massCloseCheck();
  scheduleCapture();
});

browser.tabs.onMoved.addListener(() => {
  scheduleCapture();
});

browser.tabs.onAttached.addListener(() => {
  scheduleCapture();
});

browser.tabs.onDetached.addListener(() => {
  scheduleCapture();
});

browser.tabs.onActivated.addListener((activeInfo) => {
  void setActiveTab(activeInfo.tabId);
});

browser.windows.onFocusChanged.addListener((windowId) => {
  if (windowId === browser.windows.WINDOW_ID_NONE) {
    void rollActiveClock();
    activeTabId = null;
    activeSince = null;
    return;
  }
  // Focus moved to another window: the previously focused tab is no longer active.
  void setActiveTab(null);
  void (async () => {
    try {
      const [tab] = (await browser.tabs.query({ active: true, windowId })) as { id?: number }[];
      await setActiveTab(tab?.id ?? null);
    } catch {
      await setActiveTab(null);
    }
  })();
  scheduleCapture();
});

browser.windows.onCreated.addListener(() => {
  scheduleCapture();
});

browser.windows.onRemoved.addListener((_windowId) => {
  // A window closing usually implies many tab closes; the mass-close check
  // will see them via onRemoved for each tab.
  void recordClose();
  void massCloseCheck();
  scheduleCapture();
});

const tabGroupsApi = (browser as unknown as {
  tabGroups?: {
    onCreated?: { addListener: (cb: (g: unknown) => void) => void };
    onRemoved?: { addListener: (cb: (g: unknown) => void) => void };
    onUpdated?: { addListener: (cb: (g: unknown) => void) => void };
    onMoved?: { addListener: (cb: (g: unknown) => void) => void };
  };
}).tabGroups;
if (tabGroupsApi) {
  for (const event of [tabGroupsApi.onCreated, tabGroupsApi.onRemoved, tabGroupsApi.onUpdated, tabGroupsApi.onMoved]) {
    event?.addListener(() => scheduleCapture());
  }
}

// ---------------------------------------------------------------------------
// Mass-close detection (in addition to the host's)
// ---------------------------------------------------------------------------

async function massCloseCheck(): Promise<void> {
  const settings = await getLiveSettings();
  const threshold = settings.massCloseTabThreshold;
  const windowMs = settings.massCloseWindowSecs * 1000;
  const count = await recentCloseCount(windowMs);
  if (count >= threshold) {
    // Tell the host this is a mass close; it flags a recovery point and
    // notifies the user. We also notify ourselves so it works pre-host-install.
    pendingCloses = Math.max(pendingCloses, count);
    await capture('mass_close');
    if (settings.notifyOnMassClose) {
      try {
        await browser.notifications.create({
          type: 'basic',
          iconUrl: browser.runtime.getURL('icons/icon-128.png'),
          title: `${count} tabs just closed`,
          message: 'TabVault flagged this as a recovery point. Open TabVault to restore?',
          priority: 2,
        });
      } catch (error) {
        warn('notification failed', error);
      }
    }
  }
}

// ---------------------------------------------------------------------------
// Host messages: restore jobs and notifications
// ---------------------------------------------------------------------------

host.onMessage(async (message) => {
  switch (message.type) {
    case 'restore': {
      if (message.settings) {
        // The vault is the single source of truth for live settings; apply the
        // pushed copy so capture decisions match the desktop app instantly.
        void setLiveSettings({
          paused: message.settings.paused,
          captureIntervalSecs: message.settings.captureIntervalSecs,
          captureIncognito: message.settings.captureIncognito,
          notifyOnMassClose: message.settings.notifyOnMassClose,
          massCloseTabThreshold: message.settings.massCloseTabThreshold,
          massCloseWindowSecs: message.settings.massCloseWindowSecs,
        });
      }
      for (const job of message.jobs) {
        const error = await runRestoreJob(job);
        await host.send({ type: 'restoreDone', jobId: job.id, error: error ?? null });
      }
      break;
    }
    case 'notify': {
      try {
        await browser.notifications.create(message.tag, {
          type: 'basic',
          iconUrl: browser.runtime.getURL('icons/icon-128.png'),
          title: message.title,
          message: message.body,
          priority: 2,
        });
      } catch (error) {
        warn('notification failed', error);
      }
      break;
    }
    case 'host_error':
      warn('host reported error', message.code, message.message);
      break;
    default:
      break;
  }
});

// Notification click: tell the host to launch the desktop app.
browser.notifications.onClicked.addListener(() => {
  void host.send({ type: 'openApp' });
});

// ---------------------------------------------------------------------------
// Alarms
// ---------------------------------------------------------------------------

browser.alarms.onAlarm.addListener((alarm) => {
  switch (alarm.name) {
    case CAPTURE_ALARM:
      void capture('interval');
      break;
    case POLL_ALARM:
      void pollRestore();
      break;
    case FLUSH_ALARM:
      void host.flushQueue();
      break;
    default:
      break;
  }
});

async function pollRestore(): Promise<void> {
  try {
    await host.send({ type: 'pollRestore', limit: 10 });
  } catch (error) {
    warn('restore poll failed', error);
  }
}

async function configureAlarms(): Promise<void> {
  const settings = await getLiveSettings();
  const intervalMinutes = Math.max(0.5, Math.min(60, settings.captureIntervalSecs / 60));
  await browser.alarms.create(CAPTURE_ALARM, { periodInMinutes: intervalMinutes });
  await browser.alarms.create(POLL_ALARM, { periodInMinutes: 0.5 });
  await browser.alarms.create(FLUSH_ALARM, { periodInMinutes: 1 });
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

browser.runtime.onInstalled.addListener(() => {
  void (async () => {
    await configureAlarms();
    await host.hello();
    await capture('startup');
  })();
});

browser.runtime.onStartup.addListener(() => {
  void (async () => {
    await configureAlarms();
    await host.hello();
    // On browser startup, the extension sees only the tabs the browser re-opened;
    // the vault's history is the source of truth for everything else.
    await capture('startup');
  })();
});

// When Chrome kills the service worker, a bare boot must restore state.
void (async () => {
  await configureAlarms();
  await host.ensureConnected();
})();
