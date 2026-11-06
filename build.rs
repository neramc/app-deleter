//! 빌드 스크립트
//!
//! 1. `ui/app.slint`를 Fluent 스타일로 컴파일한다.
//! 2. Windows 대상일 때 아이콘·버전 정보·매니페스트(관리자 권한, PerMonitorV2 DPI)를
//!    리소스로 실행 파일에 포함한다.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=app.manifest");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        // Windows 11 전용 앱이다. 다른 OS에서는 순수 로직(단위 테스트)만 빌드한다.
        return;
    }

    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI 컴파일 실패");

    embed_windows_resources();
}

fn embed_windows_resources() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    // rc 파일 문자열 안에서는 역슬래시가 이스케이프 문자이므로 슬래시로 통일한다.
    let as_rc_path = |p: PathBuf| p.to_string_lossy().replace('\\', "/");
    let icon = as_rc_path(manifest_dir.join("assets").join("app.ico"));
    let manifest = as_rc_path(manifest_dir.join("app.manifest"));

    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let mut nums: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    nums.resize(4, 0);
    let ver_comma = format!("{},{},{},{}", nums[0], nums[1], nums[2], nums[3]);

    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 24 "{manifest}"

1 VERSIONINFO
FILEVERSION {ver_comma}
PRODUCTVERSION {ver_comma}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "041204B0"
    BEGIN
      VALUE "CompanyName", "App Deleter"
      VALUE "FileDescription", "App Deleter - 앱 완전 삭제"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "app-deleter"
      VALUE "LegalCopyright", "Apache-2.0"
      VALUE "OriginalFilename", "app-deleter.exe"
      VALUE "ProductName", "App Deleter"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0412, 1200
  END
END
"#
    );
    let rc_path = out_dir.join("app.rc");
    fs::write(&rc_path, rc).expect("app.rc 쓰기 실패");

    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_required()
        .expect("Windows 리소스(아이콘/매니페스트) 컴파일 실패");
}
