import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AppSizeUpdate, InstalledApp, SortKey } from "./types";
import { Toolbar } from "./components/Toolbar";
import { AppList } from "./components/AppList";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { formatBytes } from "./utils";

export default function App() {
  const [apps, setApps] = useState<InstalledApp[]>([]);
  const [query, setQuery] = useState("");
  const [sortKey, setSortKey] = useState<SortKey>("size");
  const [loading, setLoading] = useState(true);
  const [calculating, setCalculating] = useState(false);
  const [target, setTarget] = useState<InstalledApp | null>(null);
  const [needElevation, setNeedElevation] = useState(false);
  const [toast, setToast] = useState<string | null>(null);

  // 가장 최근 refresh만 화면에 반영하기 위한 세대 토큰.
  const generation = useRef(0);

  const refresh = useCallback(async () => {
    const gen = ++generation.current;
    setLoading(true);
    setCalculating(true);
    try {
      // 1차: 레지스트리 메타데이터(EstimatedSize 기준). 이어서 백엔드가
      // 실제 폴더 용량을 계산하며 `app-size-updated` 이벤트를 스트리밍한다.
      const initial = await invoke<InstalledApp[]>("list_installed_apps");
      if (gen !== generation.current) return; // 더 새로운 refresh가 시작됨
      setApps(initial);
    } catch (e) {
      setToast(`앱 목록을 불러오지 못했습니다: ${String(e)}`);
    } finally {
      if (gen === generation.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 백엔드의 용량 계산 진행 이벤트 구독.
  useEffect(() => {
    const unlistenSize = listen<AppSizeUpdate>("app-size-updated", (event) => {
      const { id, size_bytes, size_estimated } = event.payload;
      setApps((prev) =>
        prev.map((a) =>
          a.id === id ? { ...a, size_bytes, size_estimated } : a,
        ),
      );
    });
    const unlistenDone = listen("app-sizes-complete", () => {
      setCalculating(false);
    });
    return () => {
      void unlistenSize.then((fn) => fn());
      void unlistenDone.then((fn) => fn());
    };
  }, []);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = q
      ? apps.filter(
          (a) =>
            a.name.toLowerCase().includes(q) ||
            (a.publisher ?? "").toLowerCase().includes(q),
        )
      : apps;
    const sorted = [...filtered].sort((a, b) =>
      sortKey === "size"
        ? b.size_bytes - a.size_bytes
        : a.name.localeCompare(b.name),
    );
    return sorted;
  }, [apps, query, sortKey]);

  const totalBytes = useMemo(
    () => apps.reduce((sum, a) => sum + a.size_bytes, 0),
    [apps],
  );
  const maxBytes = useMemo(
    () => apps.reduce((m, a) => Math.max(m, a.size_bytes), 0),
    [apps],
  );

  const handleUninstall = useCallback(async (app: InstalledApp) => {
    setTarget(null);
    if (!app.uninstall_string) return;
    try {
      await invoke("uninstall_app", { uninstallString: app.uninstall_string });
      setToast(`${app.name} 언인스톨러를 실행했습니다.`);
      // 언인스톨러는 별도 창에서 진행되므로 잠시 후 목록을 갱신한다.
      window.setTimeout(() => void refresh(), 1500);
    } catch (e) {
      // 권한 상승이 필요하면(최초 삭제 시) 관리자 재실행을 안내한다.
      if (String(e).includes("ELEVATION_REQUIRED")) {
        setNeedElevation(true);
      } else {
        setToast(`언인스톨러 실행 실패: ${String(e)}`);
      }
    }
  }, [refresh]);

  const elevate = useCallback(async () => {
    try {
      // 성공 시 백엔드가 관리자 권한으로 재실행하고 현재 인스턴스를 종료한다.
      await invoke("relaunch_as_admin");
    } catch (e) {
      setNeedElevation(false);
      setToast(`권한 상승 실패: ${String(e)}`);
    }
  }, []);

  useEffect(() => {
    if (!toast) return;
    const t = window.setTimeout(() => setToast(null), 4000);
    return () => window.clearTimeout(t);
  }, [toast]);

  return (
    <div className="app">
      <header className="app-header">
        <div className="app-title">
          <span className="app-title-icon" aria-hidden>
            🗑
          </span>
          앱 삭제
        </div>
        <div className="app-summary">
          총 {apps.length}개
          <span className="app-summary-dot">·</span>
          합계 {formatBytes(totalBytes)}
          {calculating && (
            <span className="app-summary-calc"> · 용량 계산 중…</span>
          )}
        </div>
      </header>

      <Toolbar
        query={query}
        onQueryChange={setQuery}
        sortKey={sortKey}
        onSortChange={setSortKey}
        onRefresh={() => void refresh()}
        loading={loading}
      />

      <AppList
        apps={visible}
        maxBytes={maxBytes}
        loading={loading}
        onUninstall={setTarget}
      />

      {target && (
        <ConfirmDialog
          app={target}
          onCancel={() => setTarget(null)}
          onConfirm={() => void handleUninstall(target)}
        />
      )}

      {needElevation && (
        <div className="dialog-backdrop" onClick={() => setNeedElevation(false)}>
          <div
            className="dialog"
            role="dialog"
            aria-modal="true"
            onClick={(e) => e.stopPropagation()}
          >
            <h2 className="dialog-title">관리자 권한이 필요합니다</h2>
            <p className="dialog-body">
              앱을 삭제하려면 관리자 권한이 필요합니다. 권한을 상승하고 앱을 다시
              시작할까요? (이번 세션에서 한 번만 요청합니다.)
            </p>
            <div className="dialog-actions">
              <button className="btn btn-ghost" onClick={() => setNeedElevation(false)}>
                취소
              </button>
              <button className="btn btn-accent" onClick={() => void elevate()}>
                관리자로 다시 시작
              </button>
            </div>
          </div>
        </div>
      )}

      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}
