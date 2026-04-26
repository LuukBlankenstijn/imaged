export type Tab = "hosts" | "groups" | "images" | "tasks";

const ENTRIES: { id: Tab; label: string; enabled: boolean }[] = [
  { id: "hosts", label: "Hosts", enabled: true },
  { id: "groups", label: "Groups", enabled: false },
  { id: "images", label: "Images", enabled: true },
  { id: "tasks", label: "Tasks", enabled: true },
];

interface SidebarProps {
  active: Tab;
  onSelect: (tab: Tab) => void;
}

export function Sidebar({ active, onSelect }: SidebarProps) {
  return (
    <aside className="sidebar">
      <div className="brand">
        <span className="brand-dot" />
        imaged
      </div>
      <div>
        <div className="nav-section">Workspace</div>
        <nav className="nav">
          {ENTRIES.map((entry) => (
            <button
              key={entry.id}
              className={`nav-item${active === entry.id ? " active" : ""}`}
              onClick={() => onSelect(entry.id)}
              disabled={!entry.enabled}
            >
              <span className="nav-bullet" />
              {entry.label}
            </button>
          ))}
        </nav>
      </div>
    </aside>
  );
}
