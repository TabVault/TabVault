// Global search overlay: Ctrl+K everywhere, results from the FTS index.

import { useEffect, useRef, useState } from 'react';
import { api, type SearchHit } from '../api';
import { Favicon, fmtAge } from './Shared';

export default function GlobalSearch({ open, onClose, notify }: { open: boolean; onClose: () => void; notify: (msg: string) => void }) {
  const [query, setQuery] = useState('');
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setQuery('');
      setHits([]);
      setIndex(0);
      // Focus after the overlay mounts.
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const t = setTimeout(() => {
      const q = query.trim();
      if (!q) {
        setHits([]);
        return;
      }
      void api.globalSearch(q, 30).then((rows) => {
        setHits(rows);
        setIndex(0);
      }).catch(() => setHits([]));
    }, 120);
    return () => clearTimeout(t);
  }, [query, open]);

  if (!open) return null;

  const activate = (hit: SearchHit) => {
    void api.openUrl(hit.url);
    notify(`Opening ${hit.domain}…`);
    onClose();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setIndex((i) => Math.min(hits.length - 1, i + 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setIndex((i) => Math.max(0, i - 1));
    } else if (e.key === 'Enter' && hits[index]) {
      e.preventDefault();
      activate(hits[index]);
    } else if (e.key === 'Escape') {
      onClose();
    }
  };

  return (
    <div className="overlay" role="dialog" aria-modal="true" aria-label="Global search" onClick={onClose}>
      <div className="search-box" onClick={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          type="text"
          placeholder="Search your entire browser history…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKeyDown}
          aria-label="Global search"
          autoComplete="off"
          spellCheck={false}
        />
        <div className="search-results" role="listbox">
          {hits.length === 0 && (
            <div className="empty" style={{ padding: 30 }}>
              <p>{query.trim() ? 'No matches. (Search matches titles, domains and URL words.)' : 'Type to search everything you have ever visited.'}</p>
            </div>
          )}
          {hits.map((hit, i) => (
            <button
              key={`${hit.url}-${i}`}
              role="option"
              aria-selected={i === index}
              className="row"
              style={{ width: '100%', border: 'none', borderRadius: 0, background: i === index ? 'var(--accent-soft)' : undefined, textAlign: 'left' }}
              onMouseEnter={() => setIndex(i)}
              onClick={() => activate(hit)}
            >
              <Favicon url={hit.url} title={hit.title} />
              <div className="row-text">
                <div className="row-title">{hit.title || hit.domain}</div>
                <div className="row-meta">
                  <span className="domain">{hit.domain}</span>
                  <span className="badge">{hit.kind}</span>
                  {hit.pinned && <span className="badge pin">pinned</span>}
                  <span>{hit.visitCount.toLocaleString()} visits · {fmtAge(hit.lastSeenMs)}</span>
                </div>
              </div>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
