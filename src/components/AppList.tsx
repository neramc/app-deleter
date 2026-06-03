import type { InstalledApp } from "../types";
import { AppRow } from "./AppRow";

interface AppListProps {
  apps: InstalledApp[];
  maxBytes: number;
  loading: boolean;
  onUninstall: (app: InstalledApp) => void;
}

export function AppList({ apps, maxBytes, loading, onUninstall }: AppListProps) {
  if (loading) {
    return (
      <div className="app-list">
        {Array.from({ length: 8 }).map((_, i) => (
          <div className="row skeleton" key={i} aria-hidden>
            <div className="skeleton-block" />
          </div>
        ))}
      </div>
    );
  }

  if (apps.length === 0) {
    return (
      <div className="empty-state">
        <div className="empty-icon" aria-hidden>
          📭
        </div>
        <p>표시할 앱이 없습니다.</p>
      </div>
    );
  }

  return (
    <div className="app-list" role="list">
      {apps.map((app) => (
        <AppRow
          key={app.id}
          app={app}
          maxBytes={maxBytes}
          onUninstall={onUninstall}
        />
      ))}
    </div>
  );
}
