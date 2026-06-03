import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import iconUrl from "./assets/icon.png";

interface AppInfo {
  dir: string;
  version: string;
}

type Phase = "confirm" | "removing" | "done" | "error";

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [phase, setPhase] = useState<Phase>("confirm");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<AppInfo>("app_info").then(setInfo).catch(() => setInfo(null));
  }, []);

  const remove = useCallback(async () => {
    setPhase("removing");
    try {
      await invoke("perform_uninstall");
      setPhase("done");
    } catch (e) {
      setError(String(e));
      setPhase("error");
    }
  }, []);

  const close = useCallback(() => void invoke("exit_app"), []);

  return (
    <div className="uninstaller">
      <header className="hero">
        <img className="logo" src={iconUrl} alt="" />
        <div className="hero-text">
          <h1>App Deleter 제거</h1>
          {info?.version && <span className="ver">버전 {info.version}</span>}
        </div>
      </header>

      <main className="body">
        {phase === "confirm" && (
          <>
            <p className="lead">App Deleter를 컴퓨터에서 제거합니다.</p>
            {info?.dir && (
              <p className="path">
                설치 위치: <code>{info.dir}</code>
              </p>
            )}
            <p className="hint">
              앱 파일, 시작 메뉴·바탕화면 바로가기, 제거 정보가 모두 삭제됩니다.
            </p>
          </>
        )}

        {phase === "removing" && (
          <div className="center">
            <div className="spinner" aria-hidden />
            <p>제거하는 중…</p>
          </div>
        )}

        {phase === "done" && (
          <div className="center">
            <div className="badge ok" aria-hidden>
              ✓
            </div>
            <h2>제거 완료</h2>
            <p className="hint">App Deleter가 제거되었습니다.</p>
          </div>
        )}

        {phase === "error" && (
          <div className="center">
            <div className="badge err" aria-hidden>
              ✕
            </div>
            <h2>제거 실패</h2>
            <p className="err-text">{error}</p>
          </div>
        )}
      </main>

      <footer className="actions">
        {phase === "confirm" && (
          <>
            <button className="btn btn-ghost" onClick={close}>
              취소
            </button>
            <button className="btn btn-danger" onClick={() => void remove()}>
              제거
            </button>
          </>
        )}
        {phase === "removing" && (
          <button className="btn btn-ghost" disabled>
            제거 중…
          </button>
        )}
        {(phase === "done" || phase === "error") && (
          <button className="btn btn-accent" onClick={close}>
            닫기
          </button>
        )}
      </footer>
    </div>
  );
}
