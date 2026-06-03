use std::path::Path;

fn main() {
    // 설치 마법사는 메인 앱과 제거 마법사 실행 파일을 컴파일 타임에 임베드한다.
    // CI는 빌드 전에 실제 exe들을 payload/ 로 복사한다. 로컬/체크아웃 시 파일이
    // 없으면 include_bytes! 컴파일이 실패하므로, 없을 때만 빈 플레이스홀더를 만든다.
    let payload_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("payload");
    let _ = std::fs::create_dir_all(&payload_dir);
    for name in ["app-deleter.exe", "app-deleter-uninstaller.exe"] {
        let p = payload_dir.join(name);
        if !p.exists() {
            let _ = std::fs::write(&p, b"");
            println!(
                "cargo:warning=payload/{name} 가 없어 빈 플레이스홀더를 생성했습니다. \
                 실제 배포 빌드에서는 CI가 실제 exe를 복사합니다."
            );
        }
        println!("cargo:rerun-if-changed=payload/{name}");
    }

    tauri_build::build();
}
