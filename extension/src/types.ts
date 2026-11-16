// Wire types mirroring the host's Rust protocol (crates/tv-host/src/protocol.rs
// and crates/tv-core/src/model.rs). Field names are camelCase on the wire.

export type Source =
  | 'extension'
  | 'chromium_snss'
  | 'firefox_sessionstore'
  | 'firefox_places'
  | 'chromium_history'
  | 'import';

export type Confidence = 'high' | 'medium' | 'low';

export type SnapshotReason =
  | 'interval'
  | 'tab_closed'
  | 'startup'
  | 'manual'
  | 'mass_close';

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
  source: Source;
  confidence: Confidence;
  browserTabId?: string | null;
}

export interface WindowSnapshot {
  windowId: string;
  focused: boolean;
  incognito: boolean;
  tabs: TabRecord[];
}

export interface BrowserSnapshot {
  browser: string;
  profile: string;
  windows: WindowSnapshot[];
}

export type ExtensionMessage =
  | {
      type: 'hello';
      browser: string;
      profile: string;
      extensionVersion: string;
    }
  | { type: 'state'; snapshot: BrowserSnapshot; reason: SnapshotReason }
  | { type: 'pollRestore'; limit?: number }
  | { type: 'restoreDone'; jobId: number; error?: string | null }
  | { type: 'setPaused'; paused: boolean }
  | { type: 'importSessions'; entries: ImportSessionEntry[] }
  | { type: 'importHistory'; entries: ImportHistoryEntry[] }
  | { type: 'ping' }
  | { type: 'openApp' };

/** One tab entry reported by the browser's own sessions API. */
export interface ImportSessionEntry {
  url: string;
  title: string;
  pinned: boolean;
  favicon: string | null;
  /** Seconds since the Unix epoch, as returned by browser.sessions. */
  lastActiveMs: number | null;
}

/** Aggregated visit count for one URL (from browser.history). */
export interface ImportHistoryEntry {
  url: string;
  title: string;
  visitCount: number;
  lastVisitTime: number | null;
}

/** Live settings pushed from the vault (single source of truth lives there). */
export interface LiveAppSettings {
  paused: boolean;
  captureIntervalSecs: number;
  captureIncognito: boolean;
  notifyOnMassClose: boolean;
  massCloseTabThreshold: number;
  massCloseWindowSecs: number;
}

export interface RestoreJob {
  id: number;
  action: string;
  payload: RestorePayload;
}

export interface RestoreTab {
  url: string;
  pinned: boolean;
  groupName?: string | null;
  groupColor?: string | null;
  index?: number;
  active?: boolean;
}

export interface RestorePayload {
  // Common fields
  url?: string;
  pinned?: boolean;
  newWindow?: boolean;
  active?: boolean;
  // Bulk actions
  tabs?: RestoreTab[];
  groupName?: string | null;
  groupColor?: string | null;
  // Skip URLs that are already open (restore option)
  skipOpen?: boolean;
  // Focus the app window instead of the browser
  appPath?: string;
}

export type HostMessage =
  | {
      type: 'hello_ack';
      appVersion: string;
      capabilities: string[];
    }
  | {
      type: 'ack';
      snapshotId: number | null;
      unchanged: boolean;
      recoveryPoint: boolean;
    }
  | {
      type: 'restore';
      jobs: RestoreJob[];
      /** Live settings from the vault (authoritative copy). */
      settings?: LiveAppSettings;
    }
  | {
      type: 'ack_settings';
      settings: LiveAppSettings;
    }
  | {
      type: 'notify';
      title: string;
      body: string;
      tag: string;
    }
  | { type: 'pong'; appVersion: string }
  | { type: 'host_error'; code: string; message: string };


