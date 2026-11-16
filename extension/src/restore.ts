// Executes restore jobs pushed by the host via pollRestore.
//
// Actions:
//   open_tab     — open one URL (optionally pinned, optionally in new window)
//   open_window  — open a batch of tabs into one window (new or current)
//   repin_all    — pin every URL that is not already pinned/open
//   group_open   — like open_window, but wraps the batch in a named group
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

async function openTab(tab: RestoreTab, windowId: number | undefined, skipOpen: boolean): Promise<string | null> {
  const createProperties: Record<string, unknown> = {
    url: tab.url,
    pinned: tab.pinned,
    active: tab.active ?? false,
  };
  if (windowId !== undefined) createProperties.windowId = windowId;
  if (tab.index !== undefined && tab.index >= 0) createProperties.index = tab.index;

  if (skipOpen) {
    const existing = (await browser.tabs.query({})) as { url?: string }[];
    const key = normalizeUrlKey(tab.url);
    if (existing.some((t) => normalizeUrlKey(t.url ?? '') === key)) {
      return null; // already open — nothing to do
    }
  }

  try {
    await (browser.tabs.create(createProperties as Parameters<typeof browser.tabs.create>[0]) as Promise<unknown>);
    return null;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return `${tab.url}: ${message}`;
  }
}

interface OpenPlanResult {
  errors: string[];
  createdTabIds: number[];
}

async function openBatchIntoWindow(
  tabs: RestoreTab[],
  newWindow: boolean,
  skipOpen: boolean,
): Promise<OpenPlanResult> {
  const errors: string[] = [];
  const createdTabIds: number[] = [];
  if (tabs.length === 0) return { errors, createdTabIds };

  let targetWindowId: number | undefined;
  if (newWindow) {
    const first = tabs[0];
    try {
      const createdWindow = (await browser.windows.create({
        url: first.url,
        focused: true,
      })) as { id?: number };
      targetWindowId = createdWindow?.id;
      if (first.pinned || first.index !== undefined) {
        // The window was created with the first URL un-pinned; fix it right away.
        const createdTabs = (await browser.tabs.query({
          windowId: targetWindowId,
          url: first.url,
        })) as { id?: number }[];
        for (const t of createdTabs) {
          if (t.id !== undefined) {
            await browser.tabs.update(t.id, { pinned: first.pinned });
          }
        }
      }
      tabs = tabs.slice(1);
    } catch (error) {
      errors.push(`new window: ${error instanceof Error ? error.message : String(error)}`);
      return { errors, createdTabIds };
    }
  }

  for (const tab of tabs) {
    const before = createdTabIds.length;
    const error = await openTab(tab, targetWindowId, skipOpen);
    if (error) {
      errors.push(error);
      continue;
    }
    // Best effort: track the created tab for group assignment.
    if (createdTabIds.length === before) {
      // tab was created (or skipped for skipOpen); find it by URL for grouping.
      const found = (await browser.tabs.query({
        url: tab.url,
        ...(targetWindowId !== undefined ? { windowId: targetWindowId } : {}),
      })) as { id?: number }[];
      for (const t of found) {
        if (t.id !== undefined && !createdTabIds.includes(t.id)) createdTabIds.push(t.id);
      }
    }
  }

  return { errors, createdTabIds };
}

/** Group created tabs by their groupName+groupColor and apply both. */
async function applyGroups(tabs: RestoreTab[], createdIds: number[], windowId?: number): Promise<void> {
  const byGroup = new Map<string, { color: string; ids: number[] }>();
  for (let i = 0; i < tabs.length && i < createdIds.length; i++) {
    const tab = tabs[i];
    const id = createdIds[i];
    if (id === undefined) continue;
    const name = tab.groupName?.trim();
    if (!name) continue;
    const color = tab.groupColor ?? 'grey';
    const entry = byGroup.get(name) ?? { color, ids: [] };
    entry.ids.push(id);
    byGroup.set(name, entry);
  }
  if (byGroup.size === 0) return;

  const api = (browser as unknown as {
    tabGroups?: {
      group: (options: { tabIds: number[]; createProperties?: { windowId?: number } }) => Promise<number>;
      update: (
        groupId: number,
        options: { title?: string; color?: string },
      ) => Promise<unknown>;
    };
  }).tabGroups;
  if (!api) return;

  for (const [name, { color, ids }] of byGroup) {
    try {
      const groupId = await api.group({ tabIds: ids, createProperties: windowId !== undefined ? { windowId } : undefined });
      await api.update(groupId, { title: name, color: color as 'grey' | 'blue' | 'red' | 'yellow' | 'green' | 'pink' | 'purple' | 'cyan' | 'orange' });
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
      return openTab(
        { url: payload.url, pinned: payload.pinned ?? false, active: payload.active },
        undefined,
        skipOpen,
      );
    }
    case 'open_window': {
      const tabs = payload.tabs ?? [];
      if (tabs.length === 0) return 'open_window: no tabs requested';
      const result = await openBatchIntoWindow(tabs, payload.newWindow ?? true, skipOpen);
      const windowId = undefined; // grouping handles its own window lookup
      await applyGroups(payload.tabs ?? [], result.createdTabIds, windowId);
      return result.errors.length ? result.errors.join('; ') : null;
    }
    case 'group_open': {
      const tabs = payload.tabs ?? [];
      if (tabs.length === 0) return 'group_open: no tabs requested';
      const result = await openBatchIntoWindow(tabs, payload.newWindow ?? true, skipOpen);
      await applyGroups(tabs, result.createdTabIds);
      return result.errors.length ? result.errors.join('; ') : null;
    }
    case 'repin_all': {
      const tabs = payload.tabs ?? [];
      if (tabs.length === 0) return 'repin_all: nothing to pin';
      // Pin everything into the current (or a new) window.
      const result = await openBatchIntoWindow(tabs.map((t) => ({ ...t, pinned: true })), payload.newWindow ?? true, skipOpen);
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
