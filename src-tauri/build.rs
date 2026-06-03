fn main() {
    let mut attributes = tauri_build::Attributes::new();

    // Windows에서는 관리자 권한(UAC 상승)을 요구하는 커스텀 매니페스트를 임베드한다.
    // 레지스트리 전체 열거·설치 폴더 스캔·언인스톨러 실행을 안정적으로 수행하기 위함.
    #[cfg(windows)]
    {
        attributes = attributes.windows_attributes(
            tauri_build::WindowsAttributes::new()
                .app_manifest(include_str!("app.manifest")),
        );
    }

    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
