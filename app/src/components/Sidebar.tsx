// Sidebar navigation. The screens and their hotkeys are defined once here.

export type Screen =
  | 'recently'
  | 'timeline'
  | 'pinned'
  | 'mostused'
  | 'recover'
  | 'settings';

const ITEMS: { key: Screen; label: string; icon: string }[] = [
  { key: 'recently', label: 'Recently Closed', icon: '↺' },
  { key: 'timeline', label: 'Timeline', icon: '◷' },
  { key: 'pinned', label: 'Pinned Vault', icon: '📌' },
  { key: 'mostused', label: 'Most Used', icon: '▤' },
  { key: 'recover', label: 'Recover from the past', icon: '⬇' },
  { key: 'settings', label: 'Settings', icon: '⚙' },
];

export default function Sidebar({
  current,
  onSelect,
  onSearch,
  paused,
  stats,
}: {
  current: Screen;
  onSelect: (screen: Screen) => void;
  onSearch: () => void;
  paused: boolean;
  stats: { closedTabs: number; snapshots: number } | null;
}) {
  return (
    <nav className="sidebar" aria-label="TabVault navigation">
      <div className="logo">
        <span className="logo-mark">TV</span>
        <span>TabVault</span>
      </div>

      {ITEMS.map((item) => (
        <button
          key={item.key}
          className={`nav-item ${current === item.key ? 'active' : ''}`}
          onClick={() => onSelect(item.key)}
          aria-current={current === item.key ? 'page' : undefined}
        >
          <span aria-hidden>{item.icon}</span>
          <span>{item.label}</span>
          {item.key === 'recently' && stats && <span className="count">{stats.closedTabs.toLocaleString()}</span>}
        </button>
      ))}

      <div style={{ padding: '8px 4px 0' }}>
        <button className="nav-item" onClick={onSearch}>
          <span aria-hidden>⌕</span>
          <span>Search everything</span>
          <span className="count">Ctrl K</span>
        </button>
      </div>

      <div className="sidebar-footer">
        <div className="status-pill">
          <span className={`status-dot ${paused ? 'paused' : ''}`} />
          <span>{paused ? 'Recording paused' : 'Recording'}</span>
        </div>
        {stats && <div>{stats.snapshots.toLocaleString()} snapshots stored locally</div>}
      </div>
    </nav>
  );
}
