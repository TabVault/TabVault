// Executes restore jobs pushed by the host via pollRestore.
//
// Actions:
//   open_tab     — open one URL (optionally pinned, optionally in new window)
//   open_window  — open a batch of tabs into one window (new or current)
//   repin_all    — pin every URL that is not already pinned/open
//   group_open   — like open_window, but wraps the batch in named tab groups
//
// Every job is best-effort per tab: one failing URL does not stop the rest;
// the error is accumulated and reported in restoreDone.

import browser from 'webextension-polyfill';
import type { RestoreJob, RestorePayload, RestoreTab } from './types';
import { warn } from './util';

/** Same normalization the vault uses for URL keys (fragment stripped, scheme/
 * host lower-cased, root-path trailing slash dropped). */
export function normalizeUrlKey(url: string): string {
  const trimmed = url.trim();
  if (!trimmed) return '';
  const hashCut = trimmed.indexOf('#');
  const noFragment = hashCut === -1 ? trimmed : trimmed.slice(0, hashCut);
  const sep = noFragment.indexOf('://');
  if (sep === -1) return noFragment.toLowerCase();
  const scheme = noFragment.slice(0, sep).toLowerCase();
  const rest = noFragment.slice(sep + 3);
  const cut = rest.search(/[/?]/);
  const authority = (cut === -1 ? rest : rest.slice(0, cut)).toLowerCase();
  const path = cut === -1 ? '' : rest.slice(cut);
  const normalizedPath = path === '/' ? '' : path;
  return `${scheme}://${authority}${normalizedPath}`;
}

/** Pair of a requested tab and the id the browser assigned to its new tab. */
interface CreatedPair {
  tab: RestoreTab;
  id: number;
}

interface OpenResult {
  errors: string[];
  created: CreatedPair[];
  windowId?: number;
}

interface CreatedTab {
  id?: number;
}

async function openTab(
  tab: RestoreTab,
  windowId: number | undefined,
  skipOpen: boolean,
): Promise<{ id: number | null; error: string | null }> {
  if (skipOpen) {
    const existing = (await browser.tabs.query({})) as { url?: string }[];
    const key = normalizeUrlKey(tab.url);
    if (existing.some((t) => normalizeUrlKey(t.url ?? '') === key)) {
      return { id: null, error: null }; // already open — nothing to do
    }
  }

  const createProperties: Record<string, unknown> = {
    url: tab.url,
    pinned: tab.pinned,
    active: tab.active ?? false,
  };
  if (windowId !== undefined) createProperties.windowId = windowId;
  if (tab.index !== undefined && tab.index >= 0) createProperties.index = tab.index;

  try {
    const created = (await browser.tabs.create(
      createProperties as Parameters<typeof browser.tabs.create>[0],
    )) as CreatedTab;
    return { id: created?.id ?? null, error: null };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return { id: null, error: `${tab.url}: ${message}` };
  }
}

/** Pins and activates the tab that windows.create made for `tab`. */
async function fixUpCreatedTab(
  tab: RestoreTab,
  windowId: number,
): Promise<CreatedPair | null> {
  // The window we just created holds exactly the one tab we opened, so query
  // that window directly — this survives URL redirects, which a URL-matched
  // query might miss.
  const found = (await browser.tabs.query({ windowId })) as {
    id?: number;
  }[];
  const id = found[0]?.id;
  if (id === undefined) return null;
  try {
    if (tab.pinned) {
      await browser.tabs.update(id, { pinned: true });
    }
    if (tab.active) {
      await browser.tabs.update(id, { active: true }).catch(() => undefined);
    }
  } catch {
    // A pin failure must not lose the tab from the group.
  }
  return { tab, id };
}

async function openBatchIntoWindow(
  tabs: RestoreTab[],
  newWindow: boolean,
  skipOpen: boolean,
): Promise<OpenResult> {
  const errors: string[] = [];
  const created: CreatedPair[] = [];
  if (tabs.length === 0) return { errors, created };

  let targetWindowId: number | undefined;
  let remaining = tabs;

  if (newWindow) {
    const first = tabs[0];
    try {
      const createdWindow = (await browser.windows.create({
        url: first.url,
        focused: true,
      })) as { id?: number };
      targetWindowId = createdWindow?.id;
      if (targetWindowId !== undefined) {
        const pair = await fixUpCreatedTab(first, targetWindowId);
        if (pair) created.push(pair);
      }
      remaining = tabs.slice(1);
    } catch (error) {
      errors.push(`new window: ${error instanceof Error ? error.message : String(error)}`);
      return { errors, created };
    }
  }

  for (const tab of remaining) {
    const outcome = await openTab(tab, targetWindowId, skipOpen);
    if (outcome.error) {
      errors.push(outcome.error);
      continue;
    }
    if (outcome.id !== null) {
      created.push({ tab, id: outcome.id });
    }
  }

  return { errors, created, windowId: targetWindowId };
}

/** Group created tabs by their groupName+groupColor and apply both. */
async function applyGroups(
  pairs: CreatedPair[],
  windowId: number | undefined,
): Promise<void> {
  // Chrome and Edge refuse to group pinned tabs; groups always consist of
  // unpinned tabs only.
  const byGroup = new Map<string, { color: string; ids: number[] }>();
  for (const { tab, id } of pairs) {
    if (tab.pinned) continue;
    const name = tab.groupName?.trim();
    if (!name) continue;
    const color = tab.groupColor ?? 'grey';
    const entry = byGroup.get(name) ?? { color, ids: [] };
    entry.ids.push(id);
    byGroup.set(name, entry);
  }
  if (byGroup.size === 0) return;

  // Two different APIs cooperate here:
  //   browser.tabs.group()    creates the group (chrome/edge 88+, firefox 143+)
  //   browser.tabGroups.update() names/colors it
  // Both are guarded — a missing API skips grouping without failing the job.
  const tabs = browser.tabs as unknown as {
    group?: (options: { tabIds: number[]; createProperties?: { windowId?: number } }) => Promise<number>;
  };
  const tabGroups = (browser as unknown as {
    tabGroups?: {
      update: (
        groupId: number,
        options: { title?: string; color?: string },
      ) => Promise<unknown>;
    };
  }).tabGroups;
  if (!tabs.group || !tabGroups?.update) {
    warn('tab grouping API unavailable; leaving restored tabs ungrouped');
    return;
  }

  for (const [name, { color, ids }] of byGroup) {
    try {
      const groupId = await tabs.group({
        tabIds: ids,
        createProperties: windowId !== undefined ? { windowId } : undefined,
      });
      await tabGroups.update(groupId, {
        title: name,
        color: color as 'grey' | 'blue' | 'red' | 'yellow' | 'green' | 'pink' | 'purple' | 'cyan' | 'orange',
      });
    } catch (error) {
      warn('failed to create tab group', name, error);
    }
  }
}

async function executeJob(job: RestoreJob): Promise<string | null> {
  const payload = job.payload as RestorePayload & { tabs?: RestoreTab[] };
  const skipOpen = payload.skipOpen ?? false;

  switch (job.action) {
    case 'open_tab': {
      if (!payload.url) return 'open_tab: missing url';
      const outcome = await openTab(
        { url: payload.url, pinned: payload.pinned ?? false, active: payload.active },
        undefined,
        skipOpen,
      );
      return outcome.error;
    }
    case 'open_window': {
      const tabs = payload.tabs ?? [];
      if (tabs.length === 0) return 'open_window: no tabs requested';
      const result = await openBatchIntoWindow(tabs, payload.newWindow ?? true, skipOpen);
      await applyGroups(result.created, result.windowId);
      return result.errors.length ? result.errors.join('; ') : null;
    }
    case 'group_open': {
      const tabs = payload.tabs ?? [];
      if (tabs.length === 0) return 'group_open: no tabs requested';
      const result = await openBatchIntoWindow(tabs, payload.newWindow ?? true, skipOpen);
      await applyGroups(result.created, result.windowId);
      return result.errors.length ? result.errors.join('; ') : null;
    }
    case 'repin_all': {
      const tabs = payload.tabs ?? [];
      if (tabs.length === 0) return 'repin_all: nothing to pin';
      // Pin everything into a new window; recreate the groups too.
      const pinnedTabs = tabs.map((t) => ({ ...t, pinned: true }));
      const result = await openBatchIntoWindow(pinnedTabs, payload.newWindow ?? true, skipOpen);
      await applyGroups(result.created, result.windowId);
      return result.errors.length ? result.errors.join('; ') : null;
    }
    default:
      return `unknown restore action: ${job.action}`;
  }
}

export async function runRestoreJob(job: RestoreJob): Promise<string | null> {
  try {
    return await executeJob(job);
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
}
