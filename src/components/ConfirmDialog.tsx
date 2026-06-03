import { useEffect, useRef } from "react";
import type { InstalledApp } from "../types";

interface ConfirmDialogProps {
  app: InstalledApp;
  onCancel: () => void;
  onConfirm: () => void;
}

export function ConfirmDialog({ app, onCancel, onConfirm }: ConfirmDialogProps) {
  const confirmRef = useRef<HTMLButtonElement>(null);

  // 진입 시 확인 버튼에 포커스, ESC로 닫기(접근성).
  useEffect(() => {
    confirmRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return (
    <div className="dialog-backdrop" onClick={onCancel}>
      <div
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="dialog-title"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 id="dialog-title" className="dialog-title">
          앱을 삭제할까요?
        </h2>
        <p className="dialog-body">
          <strong>{app.name}</strong>을(를) 삭제합니다. 시스템 언인스톨러가
          실행되며, 안내에 따라 제거를 완료하세요.
        </p>
        <div className="dialog-actions">
          <button className="btn btn-ghost" onClick={onCancel}>
            취소
          </button>
          <button ref={confirmRef} className="btn btn-accent" onClick={onConfirm}>
            삭제 실행
          </button>
        </div>
      </div>
    </div>
  );
}
