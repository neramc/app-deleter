import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface Progress {
  step: number;
  total: number;
  message: string;
}

type Phase = "idle" | "installing" | "done" | "error";

export default function App() {
  const [installDir, setInstallDir] = useState("");
  const [phase, setPhase] = useState<Phase>("idle");
  const [progress, setProgress] = useState<Progress | null>(null);
  const [error, setError] = useState<string | null>(null);

  // 기본 설치 경로(%LOCALAPPDATA%\AppDeleter, ASCII) 조회.
  useEffect(() => {
    invoke<string>("default_install_dir")
      .then(setInstallDir)
      .catch(() => setInstallDir("C:\\Program Files\\AppDeleter"));
  }, []);

  // 설치 진행 이벤트 구독.
  useEffect(() => {
    const un = listen<Progress>("install-progress", (e) => setProgress(e.payload));
    return () => {
      void un.then((fn) => fn());
    };
  }, []);

  const install = useCallback(async () => {
    setPhase("installing");
    setError(null);
    try {
      await invoke("install", { installDir });
      setPhase("done");
    } catch (e) {
      setError(String(e));
      setPhase("error");
    }
  }, [installDir]);

  const launch = useCallback(async () => {
    try {
      await invoke("launch_app", { installDir });
      await invoke("exit_installer");
    } catch (e) {
      setError(String(e));
    }
  }, [installDir]);

  const close = useCallback(() => void invoke("exit_installer"), []);

  const pct = progress ? Math.round((progress.step / progress.total) * 100) : 0;

  return (
    <div className="installer">
      <header className="hero">
        <div className="logo" aria-hidden>
          🗑
        </div>
        <div className="hero-text">
          <h1>App Deleter</h1>
          <p>설치된 앱을 실제 용량 기준으로 정리하는 도구</p>
        </div>
      </header>

      <main className="body">
        {phase === "idle" && (
          <>
            <label className="field-label" htmlFor="dir">
              설치 위치
            </label>
            <input
              id="dir"
              className="field"
              value={installDir}
              onChange={(e) => setInstallDir(e.target.value)}
              spellCheck={false}
            />
            <p className="hint">
              관리자 권한 없이 현재 사용자 계정에 설치됩니다. 설치 후 시작 메뉴와
              바탕화면에 바로가기가 생성됩니다.
            </p>
          </>
        )}

        {phase === "installing" && (
          <div className="progress-wrap">
            <div className="progress-track">
              <div className="progress-fill" style={{ width: `${pct}%` }} />
            </div>
            <p className="progress-msg">
              {progress?.message ?? "설치 준비 중…"} ({pct}%)
            </p>
          </div>
        )}

        {phase === "done" && (
          <div className="result success">
            <div className="result-icon" aria-hidden>
              ✓
            </div>
            <h2>설치 완료</h2>
            <p>{installDir} 에 설치되었습니다.</p>
          </div>
        )}

        {phase === "error" && (
          <div className="result error">
            <div className="result-icon" aria-hidden>
              ✕
            </div>
            <h2>설치 실패</h2>
            <p className="error-text">{error}</p>
          </div>
        )}
      </main>

      <footer className="actions">
        {phase === "idle" && (
          <>
            <button className="btn btn-ghost" onClick={close}>
              취소
            </button>
            <button className="btn btn-accent" onClick={() => void install()}>
              설치
            </button>
          </>
        )}
        {phase === "installing" && (
          <button className="btn btn-ghost" disabled>
            설치 중…
          </button>
        )}
        {phase === "done" && (
          <>
            <button className="btn btn-ghost" onClick={close}>
              닫기
            </button>
            <button className="btn btn-accent" onClick={() => void launch()}>
              실행하기
            </button>
          </>
        )}
        {phase === "error" && (
          <>
            <button className="btn btn-ghost" onClick={close}>
              닫기
            </button>
            <button
              className="btn btn-accent"
              onClick={() => {
                setPhase("idle");
                setProgress(null);
              }}
            >
              다시 시도
            </button>
          </>
        )}
      </footer>
    </div>
  );
}
