// Per-tab runtime bookkeeping kept in storage.session.
//
// The browser gives us no creation time, no visit count and no active time,
// so we track all three ourselves. storage.session survives service-worker
// restarts within the same browser run but is wiped when the browser exits —
// which is the correct lifetime for this data (a restarted browser opens
// tabs we never saw opened).

import browser from 'webextension-polyfill';

export interface TabRuntimeState {
  visitCount: number;
  activeMs: number;
  openedMs: number;
  lastActiveMs: number | null;
  favicon: string | null;
}

const STATE_KEY = 'tabvault:tabstate';
const CLOSES_KEY = 'tabvault:recentCloses';
const SETTINGS_KEY = 'tabvault:settings';

export interface LiveSettings {
  paused: boolean;
  captureIntervalSecs: number;
  captureIncognito: boolean;
  notifyOnMassClose: boolean;
  massCloseTabThreshold: number;
  massCloseWindowSecs: number;
}

export const DEFAULT_SETTINGS: LiveSettings = {
  paused: false,
  captureIntervalSecs: 60,
  captureIncognito: false,
  notifyOnMassClose: true,
  massCloseTabThreshold: 5,
  massCloseWindowSecs: 5,
};

async function readMap(): Promise<Record<string, TabRuntimeState>> {
  const data = await browser.storage.session.get(STATE_KEY);
  return (data[STATE_KEY] as Record<string, TabRuntimeState> | undefined) ?? {};
}

async function writeMap(map: Record<string, TabRuntimeState>): Promise<void> {
  await browser.storage.session.set({ [STATE_KEY]: map });
}

/** Record that a tab finished loading a new page: count it as a visit. */
export async function markVisit(tabId: number): Promise<void> {
  const map = await readMap();
  const key = String(tabId);
  const entry = map[key] ?? {
    visitCount: 0,
    activeMs: 0,
    openedMs: Date.now(),
    lastActiveMs: Date.now(),
    favicon: null,
  };
  entry.visitCount += 1;
  entry.lastActiveMs = Date.now();
  await writeMap(map);
}

/** Mark a tab as active right now. Callers run through TabGovernor instead,
 * which keeps the in-memory clock; this persists the totals. */
export async function addActiveMs(tabId: number, ms: number): Promise<void> {
  if (ms <= 0) return;
  const map = await readMap();
  const key = String(tabId);
  const entry = map[key] ?? {
    visitCount: 0,
    activeMs: 0,
    openedMs: Date.now(),
    lastActiveMs: Date.now(),
    favicon: null,
  };
  entry.activeMs += ms;
  entry.lastActiveMs = Date.now();
  await writeMap(map);
}

export async function setFavicon(tabId: number, url: string): Promise<void> {
  const map = await readMap();
  const key = String(tabId);
  const entry = map[key] ?? {
    visitCount: 0,
    activeMs: 0,
    openedMs: Date.now(),
    lastActiveMs: null,
    favicon: null,
  };
  entry.favicon = url;
  await writeMap(map);
}

export async function getTabState(tabId: number): Promise<TabRuntimeState> {
  const map = await readMap();
  return (
    map[String(tabId)] ?? {
      visitCount: 0,
      activeMs: 0,
      openedMs: Date.now(),
      lastActiveMs: null,
      favicon: null,
    }
  );
}

export async function forgetTab(tabId: number): Promise<void> {
  const map = await readMap();
  delete map[String(tabId)];
  await writeMap(map);
}

/** Snapshot the per-tab counters so a capture can embed them. */
export async function getAllTabState(): Promise<Record<number, TabRuntimeState>> {
  const map = await readMap();
  const out: Record<number, TabRuntimeState> = {};
  for (const [k, v] of Object.entries(map)) {
    out[Number(k)] = v;
  }
  return out;
}

// ---------------------------------------------------------------------------
// Recent closes (for client-side mass-close detection)
// ---------------------------------------------------------------------------

export async function recordClose(at: number = Date.now()): Promise<void> {
  const data = await browser.storage.session.get(CLOSES_KEY);
  const list = (data[CLOSES_KEY] as number[] | undefined) ?? [];
  list.push(at);
  // Never grow unboundedly; we only need the last few seconds.
  while (list.length > 200) list.shift();
  await browser.storage.session.set({ [CLOSES_KEY]: list });
}

export async function recentCloseCount(windowMs: number): Promise<number> {
  const data = await browser.storage.session.get(CLOSES_KEY);
  const list = (data[CLOSES_KEY] as number[] | undefined) ?? [];
  const cutoff = Date.now() - windowMs;
  return list.filter((t) => t >= cutoff).length;
}

// ---------------------------------------------------------------------------
// Settings cached locally (authoritative copy lives in the vault database and
// is pushed down by the desktop app via the host's poll reply)
// ---------------------------------------------------------------------------

export async function getLiveSettings(): Promise<LiveSettings> {
  const data = await browser.storage.local.get(SETTINGS_KEY);
  return { ...DEFAULT_SETTINGS, ...(data[SETTINGS_KEY] as Partial<LiveSettings> | undefined) };
}

export async function setLiveSettings(patch: Partial<LiveSettings>): Promise<LiveSettings> {
  const current = await getLiveSettings();
  const merged = { ...current, ...patch };
  await browser.storage.local.set({ [SETTINGS_KEY]: merged });
  return merged;
}
