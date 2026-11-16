// Small shared helpers used across the service worker and pages.

/** Detects the browser this extension runs in. */
export function detectBrowser(): 'chrome' | 'edge' | 'firefox' {
  const ua = navigator.userAgent;
  if (ua.includes('Firefox/')) return 'firefox';
  if (ua.includes('Edg/')) return 'edge';
  return 'chrome';
}

/** Stable per-installation profile label. Chromium exposes no profile-name
 * API, so live captures are labelled `default`; the desktop app attributes
 * real profile names when importing browser session files. */
export function profileLabel(): string {
  return 'default';
}

export function nowMs(): number {
  return Date.now();
}

/** Clamp a number to [min, max]. */
export function clamp(n: number, min: number, max: number): number {
  return Math.min(Math.max(n, min), max);
}

/** Debounce: calls the function at most once per `delayMs`, always firing
 * the trailing call so the last write wins. */
export function debounce<A extends unknown[]>(
  fn: (...args: A) => void,
  delayMs: number,
): (...args: A) => void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  return (...args: A) => {
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      fn(...args);
    }, delayMs);
  };
}

/** Retries an async operation with linear backoff. Used for the initial
 * native-host handshake; a browser may start before the host is installed. */
export async function retry<T>(
  op: () => Promise<T>,
  attempts: number,
  delayMs: number,
): Promise<T> {
  let lastError: unknown;
  for (let i = 0; i < attempts; i++) {
    try {
      return await op();
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, delayMs * (i + 1)));
    }
  }
  throw lastError;
}

export function log(...args: unknown[]): void {
  console.log('[tabvault]', ...args);
}

export function warn(...args: unknown[]): void {
  console.warn('[tabvault]', ...args);
}

export function error(...args: unknown[]): void {
  console.error('[tabvault]', ...args);
}
