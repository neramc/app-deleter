fn main() {
    // Tauri 기본 매니페스트(asInvoker + Common-Controls v6)를 사용한다.
    // 관리자 권한은 앱 시작 시가 아니라, 실제 삭제(언인스톨) 시점에 한 번만
    // 상승시킨다(uninstall.rs의 relaunch_as_admin 참고).
    tauri_build::build();
}
