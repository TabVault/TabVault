// Native messaging client with an offline queue.
//
// Lifecycle:
//   1. `ensureConnected()` opens a long-lived port to `com.tabvault.host`.
//   2. While connected, messages stream over it.
//   3. When disconnected (browser start, host crash/update, port error),
//      every outgoing message is queued in storage.local.
//   4. An alarm (`tabvault.flush`) re-attempts the connection and drains the
//      queue in FIFO order, batching to bound memory.
//
// The service worker may be killed at any time, so ALL durable state lives in
// storage.local — never in module-level variables alone.

import browser from 'webextension-polyfill';
import type { ExtensionMessage, HostMessage } from './types';
import { detectBrowser, log, nowMs, profileLabel, warn } from './util';

const HOST_NAME = 'com.tabvault.host';
const QUEUE_KEY = 'tabvault:queue';
const STATUS_KEY = 'tabvault:hostStatus';
const MAX_QUEUE = 300;
const hostVersionHint = '1.0.0';

export type HostStatus = 'unknown' | 'connected' | 'disconnected';

export interface HostInfo {
  status: HostStatus;
  lastError: string | null;
  queueDepth: number;
  lastContactMs: number | null;
  appVersion: string | null;
}

interface QueuedMessage {
  message: ExtensionMessage;
  queuedMs: number;
}

type Handler = (message: HostMessage) => void | Promise<void>;

class HostClient {
  private port: { name?: string } | null = null;
  private handlers = new Set<Handler>();
  private connecting: Promise<boolean> | null = null;

  async hello(): Promise<boolean> {
    // Read current status, reconnect if needed, and say hello.
    const ok = await this.ensureConnected();
    if (!ok) return false;
    return this.send({
      type: 'hello',
      browser: detectBrowser(),
      profile: profileLabel(),
      extensionVersion: browser.runtime.getManifest().version,
    });
  }

  /** Register a listener for host messages (restore jobs, notifications). */
  onMessage(handler: Handler): void {
    this.handlers.add(handler);
  }

  async getStatus(): Promise<HostInfo> {
    const [status, queue] = await Promise.all([
      browser.storage.local.get(STATUS_KEY),
      this.queueDepth(),
    ]);
    const info = (status[STATUS_KEY] as Partial<HostInfo> | undefined) ?? {};
    return {
      status: info.status ?? 'unknown',
      lastError: info.lastError ?? null,
      queueDepth: queue,
      lastContactMs: info.lastContactMs ?? null,
      appVersion: info.appVersion ?? null,
    };
  }

  /** Send a message; queues it when the host is down. Returns true when the
   * message reached the host (or was accepted into the port). */
  async send(message: ExtensionMessage): Promise<boolean> {
    const connected = await this.ensureConnected();
    if (connected && this.port) {
      try {
        (this.port as unknown as { postMessage: (msg: ExtensionMessage) => void }).postMessage(message);
        return true;
      } catch (error) {
        warn('host port dropped during send, queuing instead', error);
        this.teardown();
      }
    }
    await this.enqueue(message);
    return false;
  }

  /** Open the native port if not already connected. */
  async ensureConnected(): Promise<boolean> {
    if (this.port) return true;
    if (this.connecting) return this.connecting;
    this.connecting = this.doConnect();
    const result = await this.connecting;
    this.connecting = null;
    return result;
  }

  private async doConnect(): Promise<boolean> {
    try {
      const port = (browser.runtime as unknown as {
        connectNative: (name: string) => {
          onMessage: { addListener: (cb: (msg: unknown) => void) => void };
          onDisconnect: { addListener: (cb: () => void) => void };
          postMessage: (msg: unknown) => void;
          name?: string;
        };
      }).connectNative(HOST_NAME);

      port.onMessage.addListener((message: unknown) => {
        void this.handleHostMessage(message as HostMessage);
      });
      port.onDisconnect.addListener(() => {
        const lastError = browser.runtime.lastError?.message ?? 'disconnected';
        void this.noteDisconnected(lastError);
      });

      this.port = port;
      await this.writeStatus({
        status: 'connected',
        lastError: null,
        lastContactMs: nowMs(),
      });
      // Greet and drain anything queued while we were away.
      this.postToPort({
        type: 'hello',
        browser: detectBrowser(),
        profile: profileLabel(),
        extensionVersion: browser.runtime.getManifest().version,
      });
      await this.flushQueue();
      return true;
    } catch (error) {
      await this.writeStatus({
        status: 'disconnected',
        lastError: error instanceof Error ? error.message : String(error),
        lastContactMs: null,
      });
      return false;
    }
  }

  private teardown(): void {
    this.port = null;
  }

  private async noteDisconnected(message: string): Promise<void> {
    this.teardown();
    await this.writeStatus({ status: 'disconnected', lastError: message });
  }

  private postToPort(message: ExtensionMessage): void {
    if (!this.port) return;
    try {
      (this.port as unknown as { postMessage: (msg: ExtensionMessage) => void }).postMessage(message);
    } catch (error) {
      warn('postMessage failed', error);
      this.teardown();
    }
  }

  private async handleHostMessage(message: HostMessage): Promise<void> {
    await this.writeStatus({ lastContactMs: nowMs(), lastError: null });
    if (message.type === 'host_error') {
      warn('host error', message.code, message.message);
      await this.writeStatus({ lastError: message.message });
    }
    for (const handler of this.handlers) {
      try {
        await handler(message);
      } catch (error) {
        warn('message handler failed', error);
      }
    }
  }

  // -------------------------------------------------------------------------
  // Offline queue
  // -------------------------------------------------------------------------

  async queueDepth(): Promise<number> {
    const data = await browser.storage.local.get(QUEUE_KEY);
    const queue = (data[QUEUE_KEY] as QueuedMessage[] | undefined) ?? [];
    return queue.length;
  }

  async enqueue(message: ExtensionMessage): Promise<void> {
    const data = await browser.storage.local.get(QUEUE_KEY);
    const queue = (data[QUEUE_KEY] as QueuedMessage[] | undefined) ?? [];
    queue.push({ message, queuedMs: nowMs() });
    // Drop oldest beyond the cap; the alternative is unbounded storage growth.
    while (queue.length > MAX_QUEUE) queue.shift();
    await browser.storage.local.set({ [QUEUE_KEY]: queue });
  }

  async flushQueue(): Promise<number> {
    if (!this.port) return 0;
    const data = await browser.storage.local.get(QUEUE_KEY);
    const queue = (data[QUEUE_KEY] as QueuedMessage[] | undefined) ?? [];
    if (queue.length === 0) return 0;
    let sent = 0;
    const remaining: QueuedMessage[] = [];
    for (const item of queue) {
      // Never re-send a hello out of order; hello always comes first.
      if (this.port) {
        this.postToPort(item.message);
        sent += 1;
      } else {
        remaining.push(item);
      }
    }
    await browser.storage.local.set({ [QUEUE_KEY]: remaining });
    log(`flushed ${sent} queued message(s); ${remaining.length} remain`);
    return sent;
  }

  private async writeStatus(patch: Partial<HostInfo>): Promise<void> {
    const data = await browser.storage.local.get(STATUS_KEY);
    const current = (data[STATUS_KEY] as Partial<HostInfo> | undefined) ?? {};
    await browser.storage.local.set({
      [STATUS_KEY]: { ...current, ...patch },
    });
  }
}

export const host = new HostClient();
export { HOST_NAME, hostVersionHint };
