// Typed wrappers around every Tauri command the backend exposes.
import { invoke } from '@tauri-apps/api/core';

export interface DashboardStats {
  snapshots: number;
  closedTabs: number;
  closedWindows: number;
  pinnedSites: number;
  visitEst: number;
  firstSnapshotMs: number | null;
  lastSnapshotMs: number | null;
  recoveryPoints: number;
}

export interface BrowserProfileRow {
  browser: string;
  profile: string;
  snapshots: number;
  lastSnapshotMs: number | null;
}

export interface ClosedTabRow {
  id: number;
  url: string;
  title: string;
  favicon: string | null;
  domain: string;
  pinned: boolean;
  browser: string;
  profile: string;
  openedMs: number | null;
  closedMs: number;
  lastActiveMs: number | null;
  activeMs: number;
  visitCount: number;
  groupName: string | null;
  groupColor: string | null;
  partOfClosedWindow: boolean;
  source: string;
  confidence: string;
  windowEventId: number | null;
  sourceBatchId: number | null;
}

export interface ClosedTabPage {
  rows: ClosedTabRow[];
  total: number;
}

export interface ClosedTabFilter {
  q?: string;
  pinnedOnly?: boolean;
  browser?: string;
  source?: string;
  dateFrom?: number;
  dateTo?: number;
  minActiveMs?: number;
  batchId?: number;
  limit?: number;
  offset?: number;
}

export interface ClosedWindowRow {
  id: number;
  browser: string;
  profile: string;
  windowId: string;
  closedMs: number;
  tabCount: number;
  pinnedCount: number;
  source: string;
  snapshotId: number | null;
  sourceBatchId: number | null;
}

export interface SnapshotRow {
  id: number;
  takenMs: number;
  browser: string;
  profile: string;
  tabCount: number;
  windowCount: number;
  reason: string;
  recoveryPoint: boolean;
  droppedCount: number;
}

export interface TabRecord {
  url: string;
  title: string;
  favicon: string | null;
  pinned: boolean;
  tabIndex: number;
  windowId: string;
  groupName: string | null;
  groupColor: string | null;
  browser: string;
  profile: string;
  openedMs: number | null;
  closedMs: number | null;
  lastActiveMs: number | null;
  activeMs: number;
  visitCount: number;
  partOfClosedWindow: boolean;
  incognito: boolean;
  source: string;
  confidence: string;
}

export interface BrowserSnapshot {
  browser: string;
  profile: string;
  windows: { windowId: string; focused: boolean; incognito: boolean; tabs: TabRecord[] }[];
}

export interface SiteStatRow {
  id: number;
  url: string;
  urlKey: string;
  domain: string;
  title: string;
  favicon: string | null;
  visitCount: number;
  activeMs: number;
  lastSeenMs: number;
  firstSeenMs: number;
  pinned: boolean;
}

export interface PinnedSiteRow {
  id: number;
  url: string;
  domain: string;
  title: string;
  favicon: string | null;
  browser: string;
  profile: string;
  firstPinnedMs: number;
  lastPinnedMs: number;
  pinCount: number;
  currentlyPinned: boolean;
  groupName: string | null;
  groupColor: string | null;
}

export interface SearchHit {
  url: string;
  domain: string;
  title: string;
  kind: string;
  pinned: boolean;
  visitCount: number;
  activeMs: number;
  lastSeenMs: number;
}

export interface Settings {
  captureIntervalSecs: number;
  captureIncognito: boolean;
  exclusions: string[];
  retentionKeepAllHours: number;
  retentionHourlyDays: number;
  retentionDailyDays: number;
  keepRecoveryPointsForever: boolean;
  startWithWindows: boolean;
  minimiseToTray: boolean;
  theme: 'dark' | 'light' | 'system';
  recordingPaused: boolean;
  encryptionEnabled: boolean;
  dailyBackup: boolean;
  backupKeep: number;
  notifyOnMassClose: boolean;
  massCloseTabThreshold: number;
  massCloseWindowSecs: number;
}

export interface RestoreMode {
  newWindow: boolean;
  skipOpen: boolean;
}

export interface SessionFileInfo {
  path: string;
  name: string;
  sizeBytes: number;
  modifiedMs: number | null;
  kind: string;
  encrypted: boolean;
}

export interface DetectedProfileInfo {
  browser: string;
  profileDir: string;
  displayName: string | null;
  path: string;
  files: SessionFileInfo[];
}

export interface ImportBatchRow {
  id: number;
  startedMs: number;
  finishedMs: number | null;
  source: string;
  sourcePath: string | null;
  browser: string | null;
  profile: string | null;
  itemsFound: number;
  itemsImported: number;
  status: string;
  error: string | null;
  importedTabs: number;
  importedWindows: number;
}

export interface ImportOutcome {
  itemsFound: number;
  itemsImported: number;
  /** How many of the imported tabs were pinned (the lost-pinned-tabs case). */
  pinnedImported: number;
  batchId: number;
  file: string;
  kind: string;
  warnings: string[];
}

export const api = {
  dashboard: () => invoke<DashboardStats>('dashboard'),
  browserProfiles: () => invoke<BrowserProfileRow[]>('browser_profiles'),
  recentlyClosed: (filter: ClosedTabFilter) => invoke<ClosedTabPage>('recently_closed', { filter }),
  closedWindows: (browser?: string, limit = 100, offset = 0) =>
    invoke<ClosedWindowRow[]>('closed_windows', { browser, limit, offset }),
  closedWindowTabs: (windowEventId: number) =>
    invoke<ClosedTabRow[]>('closed_window_tabs', { windowEventId }),
  restoreTabs: (ids: number[], mode: RestoreMode) => invoke<number>('restore_tabs', { ids, mode }),
  restoreWindow: (windowEventId: number, mode: RestoreMode) =>
    invoke<number>('restore_window', { windowEventId, mode }),
  restoreSnapshot: (snapshotId: number, mode: RestoreMode) =>
    invoke<number>('restore_snapshot', { snapshotId, mode }),
  restoreLastClosedWindow: () => invoke<number | null>('restore_last_closed_window'),
  recentRestoreRequests: () => invoke<[number, string, string, string, number | null][]>('recent_restore_requests'),
  timeline: (from: number | undefined, to: number | undefined, browser: string | undefined, limit = 300, offset = 0) =>
    invoke<SnapshotRow[]>('timeline', { from, to, browser, limit, offset }),
  snapshotDetail: (snapshotId: number) => invoke<BrowserSnapshot | null>('snapshot_detail', { snapshotId }),
  topSites: (window: 'day' | 'week' | 'month' | 'all', metric: 'count' | 'active', limit = 30) =>
    invoke<SiteStatRow[]>('top_sites', { window, metric, limit }),
  pinnedVault: (browser?: string) => invoke<PinnedSiteRow[]>('pinned_vault', { browser }),
  repinAll: (mode: RestoreMode, scope: 'current' | 'all' = 'current') => invoke<number>('repin_all', { mode, scope }),
  restorePinnedSite: (id: number, mode: RestoreMode) => invoke<number>('restore_pinned_site', { id, mode }),
  globalSearch: (query: string, limit = 30) => invoke<SearchHit[]>('global_search', { query, limit }),
  settingsGet: () => invoke<Settings>('settings_get'),
  settingsSet: (settings: Settings) => invoke<void>('settings_set', { settings }),
  pauseToggle: () => invoke<boolean>('pause_toggle'),
  addSensitiveExclusions: () => invoke<string[]>('add_sensitive_exclusions'),
  retentionRun: () => invoke<Record<string, number>>('retention_run'),
  retentionPreview: () => invoke<[number, number, number, number]>('retention_preview'),
  backupNow: () => invoke<string>('backup_now'),
  importBatches: () => invoke<ImportBatchRow[]>('import_batches'),
  openDataFolder: () => invoke<void>('open_data_folder'),
  openUrl: (url: string) => invoke<void>('open_url', { url }),
  setEncryption: (enable: boolean) => invoke<boolean>('set_encryption', { enable }),
  exportJson: () => invoke<string>('export_json'),
  exportHtml: () => invoke<string>('export_html'),
  importSeed: () => invoke<{ itemsFound: number; itemsImported: number; batchId: number }>('import_seed'),
  exportOpenFolder: () => invoke<void>('export_open_folder'),
  detectProfiles: () => invoke<DetectedProfileInfo[]>('detect_profiles'),
  importSessionFile: (path: string, browser: string, profile: string) =>
    invoke<ImportOutcome>('import_session_file', { path, browser, profile }),
  importHistoryFile: (path: string, browser: string, profile: string) =>
    invoke<ImportOutcome>('import_history_file', { path, browser, profile }),
};
