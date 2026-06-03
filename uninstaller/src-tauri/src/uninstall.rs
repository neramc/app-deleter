use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct AppInfo {
    pub dir: String,
    pub version: String,
}

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\AppDeleter";

/// 설치 위치(현재 실행 파일이 있는 폴더)와 버전 정보를 반환한다.
#[tauri::command]
pub fn app_info() -> AppInfo {
    let dir = install_dir().unwrap_or_default();
    #[cfg(windows)]
    let version = read_version().unwrap_or_default();
    #[cfg(not(windows))]
    let version = String::new();
    AppInfo { dir, version }
}

/// App Deleter를 제거한다: 레지스트리 항목·바로가기 삭제 후, 현재 프로세스가
/// 종료되면 설치 폴더 전체를 지우는 작업을 백그라운드로 예약한다.
#[tauri::command]
pub fn perform_uninstall() -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_impl::perform()
    }
    #[cfg(not(windows))]
    {
        Err("제거는 Windows에서만 지원됩니다.".into())
    }
}

/// 제거 마법사를 종료한다(예약된 폴더 삭제가 진행될 수 있도록).
#[tauri::command]
pub fn exit_app(app: tauri::AppHandle) {
    app.exit(0);
}

/// 설치 폴더 = 현재 실행 파일(uninstall.exe)이 위치한 폴더.
fn install_dir() -> Option<String> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn read_version() -> Option<String> {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags(UNINSTALL_KEY, KEY_READ).ok()?;
    key.get_value::<String, _>("DisplayVersion").ok()
}

#[cfg(windows)]
mod windows_impl {
    use super::{install_dir, UNINSTALL_KEY};
    use std::path::PathBuf;
    use std::process::Command;
    use winreg::enums::*;
    use winreg::RegKey;

    pub fn perform() -> Result<(), String> {
        let dir = install_dir().ok_or("설치 위치를 확인할 수 없습니다.")?;

        // 1) 레지스트리 제거 항목 삭제.
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let _ = hkcu.delete_subkey_all(UNINSTALL_KEY);

        // 2) 시작 메뉴 / 바탕화면 바로가기 삭제.
        for lnk in shortcut_paths() {
            let _ = std::fs::remove_file(&lnk);
        }

        // 3) 설치 폴더(실행 중인 uninstall.exe 포함)는 자기 자신을 지울 수 없으므로,
        //    현재 프로세스가 종료될 때까지 기다렸다가 삭제하는 detached 프로세스를 띄운다.
        let pid = std::process::id();
        let script = format!(
            "try {{ Wait-Process -Id {pid} -Timeout 120 -ErrorAction SilentlyContinue }} catch {{}}; \
             Start-Sleep -Milliseconds 400; \
             Remove-Item -LiteralPath '{dir}' -Recurse -Force -ErrorAction SilentlyContinue"
        );
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-WindowStyle",
                "Hidden",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .spawn()
            .map_err(|e| format!("정리 작업 예약 실패: {e}"))?;

        Ok(())
    }

    fn shortcut_paths() -> Vec<PathBuf> {
        let mut v = Vec::new();
        if let Ok(appdata) = std::env::var("APPDATA") {
            v.push(
                PathBuf::from(appdata)
                    .join(r"Microsoft\Windows\Start Menu\Programs")
                    .join("App Deleter.lnk"),
            );
        }
        if let Ok(profile) = std::env::var("USERPROFILE") {
            v.push(PathBuf::from(profile).join("Desktop").join("App Deleter.lnk"));
        }
        v
    }
}
