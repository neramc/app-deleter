import { convertFileSrc } from "@tauri-apps/api/core";
import type { InstalledApp } from "../types";
import { formatBytes } from "../utils";

interface AppRowProps {
  app: InstalledApp;
  maxBytes: number;
  onUninstall: (app: InstalledApp) => void;
}

export function AppRow({ app, maxBytes, onUninstall }: AppRowProps) {
  const ratio = maxBytes > 0 ? Math.max(app.size_bytes / maxBytes, 0.02) : 0;
  const canUninstall = !!app.uninstall_string;
  const iconSrc = app.icon_path ? convertFileSrc(app.icon_path) : null;

  return (
    <div className="row" role="listitem">
      <div className="row-icon" aria-hidden>
        {iconSrc ? (
          <img src={iconSrc} alt="" onError={(e) => (e.currentTarget.style.display = "none")} />
        ) : (
          <span className="row-icon-fallback">📦</span>
        )}
      </div>

      <div className="row-main">
        <div className="row-name" title={app.name}>
          {app.name}
        </div>
        <div className="row-meta">
          {app.publisher && <span>{app.publisher}</span>}
          {app.version && (
            <span className="row-version">v{app.version}</span>
          )}
        </div>
        <div className="size-bar" aria-hidden>
          <div className="size-bar-fill" style={{ width: `${ratio * 100}%` }} />
        </div>
      </div>

      <div className="row-size">
        {app.size_estimated && <span className="approx" title="추정 용량">~</span>}
        {formatBytes(app.size_bytes)}
      </div>

      <button
        className="btn btn-accent"
        onClick={() => onUninstall(app)}
        disabled={!canUninstall}
        title={canUninstall ? "앱 삭제" : "언인스톨 정보가 없어 삭제할 수 없습니다"}
        aria-label={`${app.name} 삭제`}
      >
        삭제
      </button>
    </div>
  );
}
