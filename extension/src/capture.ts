// Builds a BrowserSnapshot from the live browser state.
//
// One capture pass:
//   1. query all tabs (browser.tabs.query({}))          — every window
//   2. join the tab's group info (browser.tabGroups)    — when supported
//   3. join incognito flags (browser.windows)           — default-excluded
//   4. embed per-tab runtime counters (state.ts)
//   5. sanitize URLs: skip about:blank/newtab & extension internals
//
// The snapshot is sent verbatim to the host; ALL filtering (exclusions,
// incognito policy, retention) is applied there in the vault.

import browser from 'webextension-polyfill';
import { detectBrowser, profileLabel, nowMs } from './util';
import { getAllTabState } from './state';
import type { BrowserSnapshot, TabRecord, WindowSnapshot } from './types';

const SKIP_PREFIXES = [
  'about:blank',
  'about:newtab',
  'about:home',
  'about:privatebrowsing',
  'chrome://newtab',
  'chrome://new-tab-page',
  'edge://newtab',
  'devtools://',
  'chrome-extension://',
  'moz-extension://',
  'chrome-search://',
  'view-source:',
  'chrome-untrusted://',
];

export function shouldSkipUrl(url: string): boolean {
  if (!url) return true;
  const lower = url.trim().toLowerCase();
  return SKIP_PREFIXES.some((p) => lower.startsWith(p));
}

interface BrowserTabLike {
  id?: number;
  url?: string;
  pendingUrl?: string;
  title?: string;
  favIconUrl?: string;
  pinned?: boolean;
  index?: number;
  windowId?: number;
  groupId?: number;
  incognito?: boolean;
  active?: boolean;
  status?: string;
}

interface WindowLike {
  id?: number;
  focused?: boolean;
  incognito?: boolean;
}

interface GroupLike {
  id?: number;
  title?: string;
  color?: string;
  windowId?: number;
  collapsed?: boolean;
}

export async function captureSnapshot(): Promise<BrowserSnapshot> {
  const browserName = detectBrowser();
  const profile = profileLabel();
  const ts = nowMs();

  const [tabs, windows, groups] = await Promise.all([
    browser.tabs.query({}) as Promise<BrowserTabLike[]>,
    browser.windows.getAll({}) as Promise<WindowLike[]>,
    safeTabGroups(),
  ]);

  const windowById = new Map<number, WindowLike>();
  for (const w of windows) {
    if (w.id !== undefined) windowById.set(w.id, w);
  }

  const groupById = new Map<number, GroupLike>();
  for (const g of groups) {
    if (g.id !== undefined) groupById.set(g.id, g);
  }

  const runtime = await getAllTabState();

  const byWindow = new Map<number, TabRecord[]>();
  for (const tab of tabs) {
    if (tab.id === undefined) continue;
    const url = (tab.url ?? tab.pendingUrl ?? '').trim();
    if (!url || shouldSkipUrl(url)) continue;

    const w = tab.windowId !== undefined ? windowById.get(tab.windowId) : undefined;
    const incognito = tab.incognito ?? w?.incognito ?? false;
    const group = tab.groupId !== undefined && tab.groupId >= 0 ? groupById.get(tab.groupId) : undefined;

    const record: TabRecord = {
      url,
      title: tab.title ?? '',
      favicon: tab.favIconUrl ?? runtime[tab.id]?.favicon ?? null,
      pinned: tab.pinned ?? false,
      tabIndex: tab.index ?? 0,
      windowId: String(tab.windowId ?? 0),
      groupName: group?.title?.trim() ? group.title.trim() : null,
      groupColor: group?.color ?? null,
      browser: browserName,
      profile,
      openedMs: runtime[tab.id]?.openedMs ?? ts,
      closedMs: null,
      lastActiveMs: runtime[tab.id]?.lastActiveMs ?? null,
      activeMs: runtime[tab.id]?.activeMs ?? 0,
      visitCount: runtime[tab.id]?.visitCount ?? 0,
      partOfClosedWindow: false,
      incognito,
      source: 'extension',
      confidence: 'high',
    };

    const list = byWindow.get(tab.windowId ?? 0) ?? [];
    list.push(record);
    byWindow.set(tab.windowId ?? 0, list);
  }

  const focusedWindowId = windows.find((w) => w.focused)?.id;
  const snapshots: WindowSnapshot[] = [];
  for (const [windowId, records] of byWindow) {
    records.sort((a, b) => a.tabIndex - b.tabIndex);
    const w = windowById.get(windowId);
    snapshots.push({
      windowId: String(windowId),
      focused: windowId === focusedWindowId,
      incognito: w?.incognito ?? false,
      tabs: records,
    });
  }

  return { browser: browserName, profile, windows: snapshots };
}

/** tabGroups API does not exist on every browser/version; never let a
 * missing API break capture. */
async function safeTabGroups(): Promise<GroupLike[]> {
  try {
    const api = (browser as unknown as { tabGroups?: { query?: (q: object) => Promise<GroupLike[]> } }).tabGroups;
    if (api?.query) {
      return await api.query({});
    }
    return [];
  } catch (error) {
    console.warn('[tabvault] tabGroups query failed', error);
    return [];
  }
}
