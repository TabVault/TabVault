// Shared bits for the screens: formatting + favicon + badges.

import { useState } from 'react';

export function fmtAge(ms: number | null | undefined): string {
  if (!ms) return '—';
  const diff = Date.now() - ms;
  const abs = Math.abs(diff);
  if (abs < 60_000) return 'just now';
  if (abs < 3600_000) return `${Math.floor(abs / 60_000)}m ago`;
  if (abs < 86_400_000) return `${Math.floor(abs / 3600_000)}h ago`;
  const days = Math.floor(abs / 86_400_000);
  if (days < 30) return days === 1 ? 'yesterday' : `${days}d ago`;
  return new Date(ms).toLocaleDateString();
}

export function fmtDateTime(ms: number | null | undefined): string {
  if (!ms) return '—';
  return new Date(ms).toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

export function fmtDate(ms: number | null | undefined): string {
  if (!ms) return '—';
  return new Date(ms).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

export function fmtMs(ms: number | null | undefined): string {
  if (!ms) return '0s';
  const sec = Math.floor(ms / 1000);
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m`;
  const hours = Math.floor(min / 60);
  return `${hours}h ${min % 60}m`;
}

export function sourceLabel(source: string): string {
  switch (source) {
    case 'extension': return 'Live capture';
    case 'chromium_snss': return 'Chrome/Edge file';
    case 'firefox_sessionstore': return 'Firefox file';
    case 'firefox_places': return 'Firefox history';
    case 'chromium_history': return 'Chrome/Edge history';
    case 'import': return 'Imported';
    default: return source;
  }
}

export function confidenceBadge(c: string): string | null {
  if (c === 'low') return 'low confidence';
  if (c === 'medium') return null;
  return null;
}

export function Favicon({ url, title }: { url: string; title: string }) {
  const [broken, setBroken] = useState(false);
  let letter = '?';
  try {
    letter = new URL(url).hostname.replace(/^www\./, '').charAt(0).toUpperCase() || '?';
  } catch {
    letter = (title || '?').charAt(0).toUpperCase();
  }
  if (broken) {
    return <span className="favicon">{letter}</span>;
  }
  const favicon = (() => {
    try {
      return `${new URL(url).origin}/favicon.ico`;
    } catch {
      return null;
    }
  })();
  if (!favicon) return <span className="favicon">{letter}</span>;
  return (
    <span className="favicon">
      <img src={favicon} alt="" loading="lazy" referrerPolicy="no-referrer" onError={() => setBroken(true)} />
    </span>
  );
}

export function groupColor(color: string | null | undefined): string {
  switch (color) {
    case 'blue': return '#64b5f6';
    case 'red': return '#e57373';
    case 'yellow': return '#ffd54f';
    case 'green': return '#81c784';
    case 'pink': return '#f48fb1';
    case 'purple': return '#ba68c8';
    case 'cyan': return '#80deea';
    case 'orange': return '#ffb74d';
    default: return '#9e9e9e';
  }
}
